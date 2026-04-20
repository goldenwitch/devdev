---
id: wasm-engine
title: "WASM Runtime & WASI Wiring"
status: done
type: leaf
phase: 2
crate: devdev-wasm
priority: P0
depends-on: [vfs-core]
effort: M
---

# 03 — WASM Runtime & WASI Wiring

Set up Wasmtime as the WASM execution engine. Handle module loading, AOT compilation caching, WASI configuration, and VFS-backed filesystem mounting. This is the execution substrate — it runs WASM modules but doesn't know which tools exist.

## Scope

**In:**
- Wasmtime engine configuration (AOT compilation, fuel/resource limits)
- WASM module loading from embedded bytes
- Module caching: compile `.wasm` → native once, reuse across invocations
- WASI context setup: mount VFS as filesystem preopen, wire stdin/stdout/stderr, set env vars and cwd
- Per-invocation isolation: fresh `Store` + `Instance` per tool run
- Capture stdout, stderr, and exit code after execution

**Out:**
- Knowledge of which tools exist (that's `04-tool-registry`)
- The `.wasm` binaries themselves (that's `02-wasm-toolchain`)
- Sed shim or tool-specific logic

## Interface

```rust
pub struct WasmEngine {
    engine: wasmtime::Engine,
    // Module cache: name → compiled module
    modules: HashMap<String, wasmtime::Module>,
}

pub struct WasmRunConfig<'a> {
    pub module_name: &'a str,
    pub args: &'a [String],     // argv (including argv[0] = command name)
    pub stdin: &'a [u8],        // piped input
    pub env: &'a HashMap<String, String>,
    pub cwd: &'a str,           // working directory relative to VFS root
    pub fs: &'a dyn VirtualFilesystem,
}

pub struct WasmRunResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

impl WasmEngine {
    /// Create engine with AOT compilation enabled.
    pub fn new() -> Result<Self>;
    
    /// Load a WASM module from bytes and cache the compiled form.
    pub fn load_module(&mut self, name: &str, wasm_bytes: &[u8]) -> Result<()>;
    
    /// Run a loaded module with the given configuration.
    /// Creates a fresh Store + Instance, wires WASI, runs to completion.
    pub fn run(&self, config: WasmRunConfig) -> Result<WasmRunResult>;
    
    /// Check if a module is loaded.
    pub fn has_module(&self, name: &str) -> bool;
    
    /// List all loaded module names.
    pub fn loaded_modules(&self) -> Vec<&str>;
}
```

## Implementation Notes

### Wasmtime Setup

```rust
let mut config = wasmtime::Config::new();
config.cranelift_opt_level(OptLevel::Speed);  // AOT compile for speed
config.consume_fuel(true);                     // resource limiting
config.wasm_component_model(false);            // we use core WASM, not components

let engine = wasmtime::Engine::new(&config)?;
```

### WASI Wiring

The critical piece: mounting the VFS as the WASM module's filesystem.

Wasmtime's `wasmtime-wasi` crate provides `WasiCtxBuilder` with filesystem preopen support. The VFS must implement the appropriate trait (`wasmtime_wasi::filesystem::WasiFilesystem` or equivalent) so that WASM modules' WASI calls (`fd_read`, `fd_write`, `path_open`, etc.) route to VFS operations.

```rust
// Conceptual per-invocation setup:
let mut store = Store::new(&engine, ());
let wasi = WasiCtxBuilder::new()
    .preopened_dir(vfs_adapter, "/")     // VFS as root filesystem
    .stdin(stdin_pipe)                    // piped input
    .stdout(stdout_capture)               // capture output
    .stderr(stderr_capture)               // capture errors
    .env(&env_vars)                       // virtual environment
    .args(&args)                          // command-line arguments
    .build();
```

**VFS ↔ WASI adapter:** Either Wasmtime's built-in `MemoryDir`/`mem_fs` (if the API matches), or a thin adapter that implements Wasmtime's filesystem trait by delegating to `VirtualFilesystem`. Investigate Wasmtime's `preview2` WASI implementation — it has built-in in-memory filesystem support.

### Performance

- **Module caching:** `wasmtime::Module::new()` compiles WASM → native code. This is expensive (~50-200ms per module). Cache the `Module` and reuse it. Only `Store` + `Instance` are created per invocation (~1-5ms).
- **Fuel:** Set a fuel limit per invocation to prevent infinite loops. A tool that runs out of fuel returns an error.
- **Instance overhead target:** < 10ms from `run()` call to first WASM instruction executing.

### Error Handling

- WASM module traps (out-of-bounds, stack overflow) → `exit_code: 139` (SIGSEGV equivalent), trap message in stderr
- WASI `proc_exit(code)` → capture the exit code
- Module not found → `Result::Err` (not a WASM error — the registry handles this)

## Files

```
crates/devdev-wasm/Cargo.toml
crates/devdev-wasm/src/lib.rs       — WasmEngine, WasmRunConfig, WasmRunResult
crates/devdev-wasm/src/engine.rs    — Wasmtime setup, module caching
crates/devdev-wasm/src/wasi.rs      — VFS ↔ WASI adapter, WasiCtx construction
```

## Acceptance Criteria

- [ ] Load a `.wasm` module, run with args, capture stdout — output matches expected
- [ ] Module caching: loading the same module twice doesn't recompile
- [ ] VFS integration: WASM module can `path_open` and `fd_read` a file from VFS
- [ ] WASM module can `fd_write` a new file → visible in VFS afterward
- [ ] stdin piping: pass bytes as stdin, WASM module reads them via `fd_read(0)`
- [ ] Environment variables: WASM module can read `$FOO` from configured env
- [ ] cwd: WASM module sees correct working directory
- [ ] Fuel limit: infinite loop in WASM → error, not hang
- [ ] Instance creation overhead < 10ms (benchmark test)
- [ ] WASM trap → exit_code 139 + stderr message
