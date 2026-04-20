//! `WasmToolRegistry` — the public face of the tool engine.
//!
//! Implements the dispatch chain `shim → native → WASM → 127` described in
//! `capabilities/04-tool-registry.md`. The shell executor holds a
//! `Arc<dyn ToolEngine>` and calls `execute(name, args, stdin, env, cwd, fs)`;
//! this layer figures out which backend actually runs the command and
//! returns a uniform `ToolResult`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use devdev_vfs::MemFs;
use thiserror::Error;

use crate::engine::{WasmEngine, WasmError, WasmRunConfig};
use crate::native::{self, NativeTool};

/// Result type returned to the shell executor. Opaque to the backend.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

/// Errors raised during registry construction (module loading).
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("failed to load WASM module '{name}': {source}")]
    Load {
        name: &'static str,
        #[source]
        source: WasmError,
    },

    #[error(transparent)]
    Engine(#[from] WasmError),
}

/// Trait consumed by `09-shell-executor`. Backend-agnostic.
pub trait ToolEngine: Send + Sync {
    fn execute(
        &self,
        command: &str,
        args: &[String],
        stdin: &[u8],
        env: &HashMap<String, String>,
        cwd: &str,
        fs: &MemFs,
    ) -> ToolResult;

    fn available_tools(&self) -> Vec<&str>;
    fn has_tool(&self, name: &str) -> bool;
}

/// Embedded WASM tool bytes. Names here must match [`WASM_TOOLS`].
///
/// `include_bytes!` grows the binary by the sum of every `.wasm` file, but
/// construction of a registry is now free — actual Cranelift compilation
/// is deferred until the tool is first invoked (see
/// [`WasmState::ensure_loaded`]). This keeps test startup cheap even though
/// every test builds its own registry.
const EMBEDDED_WASM: &[(&str, &[u8])] = &[
    ("cat", include_bytes!("../../../tools/wasm/cat.wasm")),
    ("cp", include_bytes!("../../../tools/wasm/cp.wasm")),
    ("echo", include_bytes!("../../../tools/wasm/echo.wasm")),
    ("head", include_bytes!("../../../tools/wasm/head.wasm")),
    ("ls", include_bytes!("../../../tools/wasm/ls.wasm")),
    ("mkdir", include_bytes!("../../../tools/wasm/mkdir.wasm")),
    ("mv", include_bytes!("../../../tools/wasm/mv.wasm")),
    ("rm", include_bytes!("../../../tools/wasm/rm.wasm")),
    ("sort", include_bytes!("../../../tools/wasm/sort.wasm")),
    ("tail", include_bytes!("../../../tools/wasm/tail.wasm")),
    ("touch", include_bytes!("../../../tools/wasm/touch.wasm")),
    ("uniq", include_bytes!("../../../tools/wasm/uniq.wasm")),
    ("wc", include_bytes!("../../../tools/wasm/wc.wasm")),
];

/// Names of WASM tools baked into the binary, derived from [`EMBEDDED_WASM`].
const WASM_TOOLS: &[&str] = &[
    "cat", "cp", "echo", "head", "ls", "mkdir", "mv", "rm", "sort", "tail", "touch", "uniq", "wc",
];

/// Shared mutable state protected by a single `Mutex`: both the Wasmtime
/// engine (Store creation wants exclusive access anyway) and the set of
/// modules that have been compiled so far.
struct WasmState {
    engine: WasmEngine,
    loaded: HashSet<&'static str>,
}

impl WasmState {
    /// Compile `name` into the engine if it hasn't been compiled yet.
    fn ensure_loaded(&mut self, name: &'static str) -> Result<(), WasmError> {
        if self.loaded.contains(name) {
            return Ok(());
        }
        let bytes = EMBEDDED_WASM
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| *b)
            .ok_or_else(|| WasmError::ModuleNotFound(name.to_owned()))?;
        self.engine.load_module(name, bytes)?;
        self.loaded.insert(name);
        Ok(())
    }
}

/// The WASM + native tool registry.
///
/// Construct once per sandbox session with [`WasmToolRegistry::new`]. WASM
/// modules compile lazily on first use; construction is cheap. The engine
/// is wrapped in a `Mutex` because WASM execution currently wants
/// `&mut self` for store creation; callers see only the `&self`
/// `ToolEngine` facade.
pub struct WasmToolRegistry {
    state: Mutex<WasmState>,
    wasm_modules: HashSet<&'static str>,
    native: HashMap<&'static str, Arc<dyn NativeTool>>,
    shims: HashMap<&'static str, &'static str>,
}

impl WasmToolRegistry {
    /// Build the registry with a custom fuel and output-byte cap
    /// applied to every subsequent invocation. See [`WasmEngine`] for
    /// what each knob bounds.
    pub fn new_with_limits(fuel: u64, output_limit: usize) -> Result<Self, RegistryError> {
        let this = Self::new()?;
        this.set_fuel_limit(fuel);
        this.set_output_limit(output_limit);
        Ok(this)
    }

    /// Update the per-invocation fuel budget. Takes effect on the next
    /// tool call.
    pub fn set_fuel_limit(&self, fuel: u64) {
        self.state.lock().expect("wasm state poisoned").engine.set_fuel_limit(fuel);
    }

    /// Update the per-invocation stdout/stderr byte cap. Takes effect on
    /// the next tool call.
    pub fn set_output_limit(&self, bytes: usize) {
        self.state.lock().expect("wasm state poisoned").engine.set_output_limit(bytes);
    }

    /// Build the registry and register native fallbacks. WASM modules are
    /// NOT compiled here — they are compiled on first use (see
    /// [`WasmState::ensure_loaded`]).
    ///
    /// `diff.wasm` and `grep.wasm` are intentionally not embedded: they are
    /// served from the native backend. `sd.wasm` is not yet built by
    /// `tools/build-tools.*`; once produced, add it to [`EMBEDDED_WASM`] and
    /// enable the `sed → sd` shim.
    pub fn new() -> Result<Self, RegistryError> {
        let engine = WasmEngine::new()?;

        let mut native: HashMap<&'static str, Arc<dyn NativeTool>> = HashMap::new();
        native.insert("grep", Arc::new(native::grep::Grep));
        native.insert("find", Arc::new(native::find::Find));
        native.insert("diff", Arc::new(native::diff::Diff));

        // Shims are deliberately empty until `sd.wasm` exists. Adding an entry
        // `("sed", "sd")` here will light up the GNU-sed shim transparently.
        let shims: HashMap<&'static str, &'static str> = HashMap::new();

        Ok(Self {
            state: Mutex::new(WasmState {
                engine,
                loaded: HashSet::new(),
            }),
            wasm_modules: WASM_TOOLS.iter().copied().collect(),
            native,
            shims,
        })
    }

    /// List of tool names this registry recognises. Includes shim source
    /// names so agents get an accurate view via `available_tools()`.
    fn names(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.wasm_modules.iter().copied().collect();
        out.extend(self.native.keys().copied());
        out.extend(self.shims.keys().copied());
        out.sort_unstable();
        out.dedup();
        out
    }
}

impl ToolEngine for WasmToolRegistry {
    fn execute(
        &self,
        command: &str,
        args: &[String],
        stdin: &[u8],
        env: &HashMap<String, String>,
        cwd: &str,
        fs: &MemFs,
    ) -> ToolResult {
        // 1. Shim? Translate and recurse as the target tool.
        if let Some(&target) = self.shims.get(command) {
            let (shim_args, err) = translate_shim(command, args);
            if let Some(e) = err {
                return ToolResult {
                    stdout: Vec::new(),
                    stderr: format!("{e}\n").into_bytes(),
                    exit_code: 2,
                };
            }
            return self.execute(target, &shim_args, stdin, env, cwd, fs);
        }

        // 2. Native backend?
        if let Some(tool) = self.native.get(command) {
            return tool.execute(args, stdin, env, cwd, fs);
        }

        // 3. WASM backend? Resolve to the `&'static str` name first so
        //    downstream errors/logging reference the canonical identifier.
        if let Some(canonical) = WASM_TOOLS.iter().copied().find(|n| *n == command) {
            return run_wasm(&self.state, canonical, args, stdin, env, cwd);
        }

        // 4. Command not found.
        ToolResult {
            stdout: Vec::new(),
            stderr: format!("command not found: {command}\n").into_bytes(),
            exit_code: 127,
        }
    }

    fn available_tools(&self) -> Vec<&str> {
        self.names()
    }

    fn has_tool(&self, name: &str) -> bool {
        self.shims.contains_key(name)
            || self.native.contains_key(name)
            || self.wasm_modules.contains(name)
    }
}

/// Execute a WASM tool. Compiles the module on first invocation (see
/// [`WasmState::ensure_loaded`]).
///
/// For P0 the registry does **not** bridge the VFS into the sandbox: tools
/// see an empty preopen, so they operate on stdin/stdout only. VFS-aware
/// file arguments will be added alongside the shell executor work in
/// capability 09, once the executor knows which paths a command touches.
fn run_wasm(
    state: &Mutex<WasmState>,
    command: &'static str,
    args: &[String],
    stdin: &[u8],
    env: &HashMap<String, String>,
    cwd: &str,
) -> ToolResult {
    let mut guard = match state.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Err(e) = guard.ensure_loaded(command) {
        return ToolResult {
            stdout: Vec::new(),
            stderr: format!("{command}: {e}\n").into_bytes(),
            exit_code: 1,
        };
    }
    let cfg = WasmRunConfig {
        module_name: command,
        args,
        stdin,
        env,
        cwd,
        preopened_dir: None,
    };
    match guard.engine.run(cfg) {
        Ok(r) => ToolResult {
            stdout: r.stdout,
            stderr: r.stderr,
            exit_code: r.exit_code,
        },
        Err(WasmError::ModuleNotFound(_)) => ToolResult {
            stdout: Vec::new(),
            stderr: format!("command not found: {command}\n").into_bytes(),
            exit_code: 127,
        },
        Err(e) => ToolResult {
            stdout: Vec::new(),
            stderr: format!("{command}: {e}\n").into_bytes(),
            exit_code: 1,
        },
    }
}

/// Translate a shim invocation into the target tool's argv. Returns
/// `(translated_args, Some(err))` on untranslatable input.
fn translate_shim(command: &str, _args: &[String]) -> (Vec<String>, Option<String>) {
    // No shims are active yet (see `new()` above). This function exists so
    // enabling one becomes a single-line change in the shims table.
    (
        Vec::new(),
        Some(format!("{command}: shim not implemented yet")),
    )
}
