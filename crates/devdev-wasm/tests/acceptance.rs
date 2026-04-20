//! Acceptance tests for Cap 03 — WASM Runtime & WASI Wiring.
//!
//! Tests use WAT (WebAssembly Text format) for inline test modules.
//! Wasmtime can compile WAT directly, so no external .wasm files needed.

use std::collections::HashMap;

use devdev_wasm::{WasmEngine, WasmRunConfig};
use tempfile::TempDir;

/// Minimal WASI program that writes "hello\n" to stdout via fd_write.
const HELLO_WAT: &str = r#"
(module
  ;; Import WASI fd_write: (fd, iovs_ptr, iovs_len, nwritten_ptr) -> errno
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  ;; Import WASI proc_exit
  (import "wasi_snapshot_preview1" "proc_exit"
    (func $proc_exit (param i32)))

  (memory (export "memory") 1)

  ;; Data: "hello\n" at offset 8
  (data (i32.const 8) "hello\n")

  ;; iov: {buf_ptr=8, buf_len=6} at offset 0
  (data (i32.const 0) "\08\00\00\00\06\00\00\00")

  (func (export "_start")
    ;; fd_write(stdout=1, iovs=0, iovs_len=1, nwritten=100)
    (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 100))
    drop
    ;; proc_exit(0)
    (call $proc_exit (i32.const 0))
  )
)
"#;

/// Minimal WASI program that exits with code 42.
const EXIT42_WAT: &str = r#"
(module
  (import "wasi_snapshot_preview1" "proc_exit"
    (func $proc_exit (param i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    (call $proc_exit (i32.const 42))
  )
)
"#;

/// Minimal WASI program that reads args and writes them to stdout.
const ECHO_ARGS_WAT: &str = r#"
(module
  (import "wasi_snapshot_preview1" "args_sizes_get"
    (func $args_sizes_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "args_get"
    (func $args_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit"
    (func $proc_exit (param i32)))
  (memory (export "memory") 1)

  (func (export "_start")
    (local $argc i32)
    (local $argv_buf_size i32)
    (local $i i32)
    (local $ptr i32)
    (local $len i32)

    ;; Get args sizes: argc at offset 0, buf_size at offset 4
    (drop (call $args_sizes_get (i32.const 0) (i32.const 4)))
    (local.set $argc (i32.load (i32.const 0)))
    (local.set $argv_buf_size (i32.load (i32.const 4)))

    ;; args_get: ptrs at 100, buf at 1000
    (drop (call $args_get (i32.const 100) (i32.const 1000)))

    ;; Skip argv[0], print argv[1..] separated by spaces
    (local.set $i (i32.const 1))
    (block $break
      (loop $loop
        (br_if $break (i32.ge_u (local.get $i) (local.get $argc)))

        ;; Get pointer to argv[i]
        (local.set $ptr
          (i32.load (i32.add (i32.const 100) (i32.mul (local.get $i) (i32.const 4)))))

        ;; Calculate length by finding null terminator
        (local.set $len (i32.const 0))
        (block $found
          (loop $scan
            (br_if $found (i32.eqz (i32.load8_u (i32.add (local.get $ptr) (local.get $len)))))
            (local.set $len (i32.add (local.get $len) (i32.const 1)))
            (br $scan)
          )
        )

        ;; Write arg: set up iov at offset 2000
        (i32.store (i32.const 2000) (local.get $ptr))
        (i32.store (i32.const 2004) (local.get $len))
        (drop (call $fd_write (i32.const 1) (i32.const 2000) (i32.const 1) (i32.const 2100)))

        ;; Write space if not last
        (if (i32.lt_u (local.get $i) (i32.sub (local.get $argc) (i32.const 1)))
          (then
            (i32.store8 (i32.const 2200) (i32.const 32)) ;; space
            (i32.store (i32.const 2000) (i32.const 2200))
            (i32.store (i32.const 2004) (i32.const 1))
            (drop (call $fd_write (i32.const 1) (i32.const 2000) (i32.const 1) (i32.const 2100)))
          )
        )

        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)
      )
    )

    (call $proc_exit (i32.const 0))
  )
)
"#;

/// Minimal WASI program that reads stdin and writes it to stdout.
const CAT_WAT: &str = r#"
(module
  (import "wasi_snapshot_preview1" "fd_read"
    (func $fd_read (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit"
    (func $proc_exit (param i32)))
  (memory (export "memory") 1)

  (func (export "_start")
    (local $nread i32)

    ;; Read from stdin into buffer at 100, up to 4096 bytes
    ;; iov: {buf=100, len=4096} at offset 0
    (i32.store (i32.const 0) (i32.const 100))
    (i32.store (i32.const 4) (i32.const 4096))
    (drop (call $fd_read (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 48)))
    (local.set $nread (i32.load (i32.const 48)))

    ;; Write nread bytes to stdout
    (if (i32.gt_u (local.get $nread) (i32.const 0))
      (then
        (i32.store (i32.const 0) (i32.const 100))
        (i32.store (i32.const 4) (local.get $nread))
        (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 48)))
      )
    )

    (call $proc_exit (i32.const 0))
  )
)
"#;

/// Minimal WASI program that reads env var FOO.
const ENV_WAT: &str = r#"
(module
  (import "wasi_snapshot_preview1" "environ_sizes_get"
    (func $environ_sizes_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "environ_get"
    (func $environ_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit"
    (func $proc_exit (param i32)))
  (memory (export "memory") 1)

  (func (export "_start")
    (local $env_count i32)
    (local $env_buf_size i32)
    (local $i i32)
    (local $ptr i32)
    (local $len i32)

    ;; Get env sizes
    (drop (call $environ_sizes_get (i32.const 0) (i32.const 4)))
    (local.set $env_count (i32.load (i32.const 0)))
    (local.set $env_buf_size (i32.load (i32.const 4)))

    ;; Get env: ptrs at 100, buf at 1000
    (drop (call $environ_get (i32.const 100) (i32.const 1000)))

    ;; Print all env vars (each is "KEY=VALUE\0")
    (local.set $i (i32.const 0))
    (block $break
      (loop $loop
        (br_if $break (i32.ge_u (local.get $i) (local.get $env_count)))

        (local.set $ptr
          (i32.load (i32.add (i32.const 100) (i32.mul (local.get $i) (i32.const 4)))))

        ;; Find length (null terminator)
        (local.set $len (i32.const 0))
        (block $found
          (loop $scan
            (br_if $found (i32.eqz (i32.load8_u (i32.add (local.get $ptr) (local.get $len)))))
            (local.set $len (i32.add (local.get $len) (i32.const 1)))
            (br $scan)
          )
        )

        ;; Write to stdout
        (i32.store (i32.const 2000) (local.get $ptr))
        (i32.store (i32.const 2004) (local.get $len))
        (drop (call $fd_write (i32.const 1) (i32.const 2000) (i32.const 1) (i32.const 2100)))

        ;; newline
        (i32.store8 (i32.const 2200) (i32.const 10))
        (i32.store (i32.const 2000) (i32.const 2200))
        (i32.store (i32.const 2004) (i32.const 1))
        (drop (call $fd_write (i32.const 1) (i32.const 2000) (i32.const 1) (i32.const 2100)))

        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop)
      )
    )

    (call $proc_exit (i32.const 0))
  )
)
"#;

/// Infinite loop module (for fuel test).
const INFINITE_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "_start")
    (loop $inf (br $inf))
  )
)
"#;

/// WASI module that exits 0 if fd 3 (preopened dir) exists, else non-zero errno.
const PRESTAT_WAT: &str = r#"
(module
  (import "wasi_snapshot_preview1" "fd_prestat_get"
    (func $fd_prestat_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit"
    (func $proc_exit (param i32)))
  (memory (export "memory") 1)
  (func (export "_start")
    ;; fd_prestat_get(fd=3, buf=0): returns 0 if preopened dir exists, EBADF otherwise
    (call $proc_exit (call $fd_prestat_get (i32.const 3) (i32.const 0)))
  )
)
"#;

fn empty_env() -> HashMap<String, String> {
    HashMap::new()
}

fn no_args() -> Vec<String> {
    vec!["test".to_owned()]
}

/// AC: Load a .wasm module, run with args, capture stdout.
#[test]
fn run_hello_capture_stdout() {
    let mut engine = WasmEngine::new().unwrap();
    engine.load_module("hello", HELLO_WAT.as_bytes()).unwrap();

    let env = empty_env();
    let result = engine
        .run(WasmRunConfig {
            module_name: "hello",
            args: &no_args(),
            stdin: &[],
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"hello\n");
}

/// AC: Module caching — loading the same module twice doesn't recompile.
#[test]
fn module_caching() {
    let mut engine = WasmEngine::new().unwrap();
    engine.load_module("hello", HELLO_WAT.as_bytes()).unwrap();
    assert!(engine.has_module("hello"));

    // Loading again should succeed (overwrites cache)
    engine.load_module("hello", HELLO_WAT.as_bytes()).unwrap();
    assert!(engine.has_module("hello"));
    assert_eq!(engine.loaded_modules().len(), 1);
}

/// AC: stdin piping — pass bytes as stdin, WASM module reads them.
#[test]
fn stdin_piping() {
    let mut engine = WasmEngine::new().unwrap();
    engine.load_module("cat", CAT_WAT.as_bytes()).unwrap();

    let env = empty_env();
    let input = b"piped data";
    let result = engine
        .run(WasmRunConfig {
            module_name: "cat",
            args: &no_args(),
            stdin: input,
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"piped data");
}

/// AC: Environment variables — WASM module reads $FOO.
#[test]
fn env_vars() {
    let mut engine = WasmEngine::new().unwrap();
    engine.load_module("env", ENV_WAT.as_bytes()).unwrap();

    let mut env = HashMap::new();
    env.insert("FOO".into(), "bar".into());

    let result = engine
        .run(WasmRunConfig {
            module_name: "env",
            args: &no_args(),
            stdin: &[],
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_eq!(result.exit_code, 0);
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("FOO=bar"), "output: {output}");
}

/// AC: Fuel limit — infinite loop in WASM → error, not hang.
#[test]
fn fuel_limit_prevents_hang() {
    let mut engine = WasmEngine::new().unwrap();
    engine.set_fuel_limit(1000); // Very low fuel
    engine
        .load_module("infinite", INFINITE_WAT.as_bytes())
        .unwrap();

    let env = empty_env();
    let result = engine.run(WasmRunConfig {
        module_name: "infinite",
        args: &no_args(),
        stdin: &[],
        env: &env,
        cwd: "/",
        preopened_dir: None,
    });

    // Should be a trap (out of fuel) resulting in exit_code 139
    // or a WasmError
    match result {
        Ok(r) => {
            assert_eq!(r.exit_code, 139, "expected trap exit code");
            let stderr = String::from_utf8_lossy(&r.stderr);
            assert!(
                stderr.contains("trap") || stderr.contains("fuel"),
                "stderr: {stderr}"
            );
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("fuel") || msg.contains("trap"),
                "error: {msg}"
            );
        }
    }
}

/// AC: WASM proc_exit(42) → exit_code 42.
#[test]
fn proc_exit_code() {
    let mut engine = WasmEngine::new().unwrap();
    engine
        .load_module("exit42", EXIT42_WAT.as_bytes())
        .unwrap();

    let env = empty_env();
    let result = engine
        .run(WasmRunConfig {
            module_name: "exit42",
            args: &no_args(),
            stdin: &[],
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_eq!(result.exit_code, 42);
}

/// AC: VFS integration — preopened dir is visible to WASM module as fd 3.
#[test]
fn preopened_dir_visible() {
    let dir = TempDir::new().unwrap();
    let mut engine = WasmEngine::new().unwrap();
    engine.load_module("prestat", PRESTAT_WAT.as_bytes()).unwrap();

    let env = empty_env();
    let result = engine
        .run(WasmRunConfig {
            module_name: "prestat",
            args: &no_args(),
            stdin: &[],
            env: &env,
            cwd: "/",
            preopened_dir: Some(dir.path()),
        })
        .unwrap();

    assert_eq!(result.exit_code, 0, "fd 3 prestat should succeed");
}

/// Negative: without a preopened dir, fd 3 prestat fails (non-zero exit).
#[test]
fn no_preopened_dir_no_fd3() {
    let mut engine = WasmEngine::new().unwrap();
    engine.load_module("prestat", PRESTAT_WAT.as_bytes()).unwrap();

    let env = empty_env();
    let result = engine
        .run(WasmRunConfig {
            module_name: "prestat",
            args: &no_args(),
            stdin: &[],
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_ne!(result.exit_code, 0, "fd 3 prestat should fail without preopened dir");
}

/// AC: Module not found → error.
#[test]
fn module_not_found() {
    let engine = WasmEngine::new().unwrap();
    let env = empty_env();
    let result = engine.run(WasmRunConfig {
        module_name: "nonexistent",
        args: &no_args(),
        stdin: &[],
        env: &env,
        cwd: "/",
        preopened_dir: None,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

/// AC: args are passed correctly to WASM module.
#[test]
fn args_passed() {
    let mut engine = WasmEngine::new().unwrap();
    engine
        .load_module("echo_args", ECHO_ARGS_WAT.as_bytes())
        .unwrap();

    let env = empty_env();
    let args: Vec<String> = vec!["echo".into(), "hello".into(), "world".into()];
    let result = engine
        .run(WasmRunConfig {
            module_name: "echo_args",
            args: &args,
            stdin: &[],
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "hello world");
}
