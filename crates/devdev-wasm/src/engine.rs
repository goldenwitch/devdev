//! WASM engine — module loading, caching, and WASI execution.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::preview1::WasiP1Ctx;
use wasmtime_wasi::WasiCtxBuilder;

/// Errors from WASM engine operations.
#[derive(Debug, Error)]
pub enum WasmError {
    #[error("module not found: {0}")]
    ModuleNotFound(String),

    #[error("wasmtime error: {0}")]
    Wasmtime(#[from] wasmtime::Error),

    #[error("wasi exit: {0}")]
    WasiExit(i32),
}

/// Configuration for a single WASM module invocation.
pub struct WasmRunConfig<'a> {
    pub module_name: &'a str,
    pub args: &'a [String],
    pub stdin: &'a [u8],
    pub env: &'a HashMap<String, String>,
    pub cwd: &'a str,
    pub preopened_dir: Option<&'a std::path::Path>,
}

/// Result of a WASM module invocation.
#[derive(Debug)]
pub struct WasmRunResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

/// Default cap on captured stdout / stderr per invocation. Beyond this,
/// the WASI pipe returns `BrokenPipe` to the guest and the tool exits
/// non-zero — preferable to letting a rogue module allocate host memory
/// until OOM. 32 MiB is generous for legitimate output and bounded
/// enough to prevent DoS.
pub const DEFAULT_OUTPUT_LIMIT: usize = 32 * 1024 * 1024;

/// Default fuel per invocation (≈100M wasm instructions). Bounds CPU,
/// not memory.
pub const DEFAULT_FUEL: u64 = 100_000_000;

/// WASM execution engine with module caching.
pub struct WasmEngine {
    engine: Engine,
    modules: HashMap<String, Arc<Module>>,
    default_fuel: u64,
    output_limit: usize,
}

impl WasmEngine {
    /// Create a new engine with AOT compilation and fuel metering.
    pub fn new() -> Result<Self, WasmError> {
        let mut config = wasmtime::Config::new();
        config.cranelift_opt_level(wasmtime::OptLevel::Speed);
        config.consume_fuel(true);

        let engine = Engine::new(&config)?;
        Ok(Self {
            engine,
            modules: HashMap::new(),
            default_fuel: DEFAULT_FUEL,
            output_limit: DEFAULT_OUTPUT_LIMIT,
        })
    }

    /// Set the default fuel limit for invocations.
    pub fn set_fuel_limit(&mut self, fuel: u64) {
        self.default_fuel = fuel;
    }

    /// Set the per-invocation stdout/stderr cap in bytes. Applies to
    /// both streams independently.
    pub fn set_output_limit(&mut self, bytes: usize) {
        self.output_limit = bytes;
    }

    /// Current per-invocation stdout/stderr cap in bytes.
    pub fn output_limit(&self) -> usize {
        self.output_limit
    }

    /// Load a WASM module from bytes and cache the compiled form.
    /// Accepts both `.wasm` binary and `.wat` text format.
    pub fn load_module(&mut self, name: &str, wasm_bytes: &[u8]) -> Result<(), WasmError> {
        let module = Module::new(&self.engine, wasm_bytes)?;
        self.modules.insert(name.to_owned(), Arc::new(module));
        Ok(())
    }

    /// Check if a module is already loaded (cached).
    pub fn has_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// List all loaded module names.
    pub fn loaded_modules(&self) -> Vec<&str> {
        self.modules.keys().map(|k| k.as_str()).collect()
    }

    /// Run a loaded module with the given configuration.
    pub fn run(&self, config: WasmRunConfig) -> Result<WasmRunResult, WasmError> {
        let module = self
            .modules
            .get(config.module_name)
            .ok_or_else(|| WasmError::ModuleNotFound(config.module_name.to_owned()))?
            .clone();

        // Set up stdout/stderr capture pipes with a hard cap — beyond
        // this, MemoryOutputPipe returns BrokenPipe to the guest.
        let stdout_pipe = MemoryOutputPipe::new(self.output_limit);
        let stderr_pipe = MemoryOutputPipe::new(self.output_limit);
        let stdout_clone = stdout_pipe.clone();
        let stderr_clone = stderr_pipe.clone();

        // Build WASI context
        let mut wasi_builder = WasiCtxBuilder::new();
        wasi_builder
            .args(config.args)
            .stdin(MemoryInputPipe::new(config.stdin.to_vec()))
            .stdout(stdout_pipe)
            .stderr(stderr_pipe)
            .allow_blocking_current_thread(true);

        // Set environment variables
        let env_pairs: Vec<(&str, &str)> = config
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        wasi_builder.envs(&env_pairs);

        // Preopened directory for filesystem access
        if let Some(dir_path) = config.preopened_dir {
            wasi_builder
                .preopened_dir(
                    dir_path,
                    "/",
                    wasmtime_wasi::DirPerms::all(),
                    wasmtime_wasi::FilePerms::all(),
                )
                .map_err(WasmError::Wasmtime)?;
        }

        let wasi_ctx = wasi_builder.build_p1();

        // Run the module in a block so the store is dropped before we read pipes
        let run_result = {
            let mut store = Store::new(&self.engine, wasi_ctx);
            store.set_fuel(self.default_fuel)?;

            let mut linker = Linker::new(&self.engine);
            wasmtime_wasi::preview1::add_to_linker_sync(&mut linker, |t: &mut WasiP1Ctx| t)?;

            let instance = linker.instantiate(&mut store, &module)?;
            let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
            start.call(&mut store, ())
        };

        let exit_code = match run_result {
            Ok(()) => 0,
            Err(err) => {
                if let Some(exit) = err.downcast_ref::<wasmtime_wasi::I32Exit>() {
                    exit.0
                } else if err.is::<wasmtime::Trap>() {
                    let trap_msg = format!("WASM trap: {err}");
                    let mut stderr_bytes = stderr_clone.contents().to_vec();
                    stderr_bytes.extend_from_slice(trap_msg.as_bytes());
                    return Ok(WasmRunResult {
                        stdout: stdout_clone.contents().to_vec(),
                        stderr: stderr_bytes,
                        exit_code: 139,
                    });
                } else {
                    return Err(WasmError::Wasmtime(err));
                }
            }
        };

        Ok(WasmRunResult {
            stdout: stdout_clone.try_into_inner().unwrap_or_default().to_vec(),
            stderr: stderr_clone.try_into_inner().unwrap_or_default().to_vec(),
            exit_code,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_finite() {
        let engine = WasmEngine::new().unwrap();
        assert_eq!(engine.output_limit(), DEFAULT_OUTPUT_LIMIT);
        assert!(engine.output_limit() < usize::MAX);
    }

    #[test]
    fn set_output_limit_round_trips() {
        let mut engine = WasmEngine::new().unwrap();
        engine.set_output_limit(1024);
        assert_eq!(engine.output_limit(), 1024);
        engine.set_output_limit(0);
        assert_eq!(engine.output_limit(), 0);
    }

    #[test]
    fn set_fuel_limit_does_not_panic() {
        let mut engine = WasmEngine::new().unwrap();
        engine.set_fuel_limit(1);
        engine.set_fuel_limit(u64::MAX);
    }
}