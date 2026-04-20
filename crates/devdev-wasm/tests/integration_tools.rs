//! Integration tests for Cap 02 — WASM Coreutils Build Pipeline.
//!
//! These tests load the real `.wasm` binaries produced by the build pipeline
//! and run them through the WasmEngine (Cap 03) to prove end-to-end viability.
//!
//! Each test is `#[ignore]` by default because the `.wasm` files are build
//! artifacts not checked into git. Run with:
//!     cargo test -p devdev-wasm --test integration_tools -- --ignored

use std::collections::HashMap;
use std::path::Path;

use devdev_wasm::{WasmEngine, WasmRunConfig};

fn tools_dir() -> &'static Path {
    // Relative to workspace root
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/wasm"))
}

fn load_tool(engine: &mut WasmEngine, name: &str) {
    let path = tools_dir().join(format!("{name}.wasm"));
    assert!(path.exists(), "{name}.wasm not found at {path:?} — run build-tools first");
    let bytes = std::fs::read(&path).unwrap();
    engine.load_module(name, &bytes).unwrap();
}

fn empty_env() -> HashMap<String, String> {
    HashMap::new()
}

/// AC: cat.wasm is a valid WASI module that reads stdin and writes to stdout.
#[test]
#[ignore]
fn cat_wasm_pipes_stdin_to_stdout() {
    let mut engine = WasmEngine::new().unwrap();
    load_tool(&mut engine, "cat");

    let env = empty_env();
    let input = b"hello from cat\n";
    let args = vec!["cat".to_string()];
    let result = engine
        .run(WasmRunConfig {
            module_name: "cat",
            args: &args,
            stdin: input,
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, input);
}

/// AC: echo.wasm produces correct output for given arguments.
#[test]
#[ignore]
fn echo_wasm_prints_args() {
    let mut engine = WasmEngine::new().unwrap();
    load_tool(&mut engine, "echo");

    let env = empty_env();
    let args = vec!["echo".to_string(), "hello".to_string(), "world".to_string()];
    let result = engine
        .run(WasmRunConfig {
            module_name: "echo",
            args: &args,
            stdin: &[],
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(String::from_utf8(result.stdout).unwrap(), "hello world\n");
}

/// AC: grep.wasm (ripgrep) filters stdin lines matching a pattern.
#[test]
#[ignore]
fn grep_wasm_filters_stdin() {
    let mut engine = WasmEngine::new().unwrap();
    load_tool(&mut engine, "grep");

    let env = empty_env();
    let input = b"apple\nbanana\napricot\ncherry\n";
    let args = vec!["rg".to_string(), "ap".to_string(), "-".to_string()];
    let result = engine
        .run(WasmRunConfig {
            module_name: "grep",
            args: &args,
            stdin: input,
            env: &env,
            cwd: "/",
            preopened_dir: None,
        })
        .unwrap();

    assert_eq!(result.exit_code, 0);
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(stdout.contains("apple"), "stdout: {stdout}");
    assert!(stdout.contains("apricot"), "stdout: {stdout}");
    assert!(!stdout.contains("banana"), "stdout: {stdout}");
    assert!(!stdout.contains("cherry"), "stdout: {stdout}");
}

/// AC: diff.wasm produces unified diff output for two files.
#[test]
#[ignore]
fn diff_wasm_unified_output() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "line1\nline2\nline3\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "line1\nchanged\nline3\n").unwrap();

    let mut engine = WasmEngine::new().unwrap();
    load_tool(&mut engine, "diff");

    let env = empty_env();
    let args = vec![
        "diff".to_string(),
        "-u".to_string(),
        "a.txt".to_string(),
        "b.txt".to_string(),
    ];
    let result = engine
        .run(WasmRunConfig {
            module_name: "diff",
            args: &args,
            stdin: &[],
            env: &env,
            cwd: "/",
            preopened_dir: Some(dir.path()),
        })
        .unwrap();

    assert_eq!(result.exit_code, 1, "exit 1 = files differ");
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(stdout.contains("--- a.txt"), "stdout: {stdout}");
    assert!(stdout.contains("+++ b.txt"), "stdout: {stdout}");
    assert!(stdout.contains("-line2"), "stdout: {stdout}");
    assert!(stdout.contains("+changed"), "stdout: {stdout}");
    assert!(!stdout.contains("-line1"), "unchanged line1 should not be a deletion: {stdout}");
    assert!(!stdout.contains("-line3"), "unchanged line3 should not be a deletion: {stdout}");
}

/// AC: Each built `.wasm` binary loads without error (valid WASI module).
#[test]
#[ignore]
fn all_built_tools_load_successfully() {
    let dir = tools_dir();
    if !dir.exists() {
        eprintln!("skipping: tools/wasm/ not found");
        return;
    }

    let mut engine = WasmEngine::new().unwrap();
    let mut loaded = 0;

    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "wasm") {
            let name = path.file_stem().unwrap().to_str().unwrap();
            let bytes = std::fs::read(&path).unwrap();
            engine
                .load_module(name, &bytes)
                .unwrap_or_else(|e| panic!("failed to load {name}.wasm: {e}"));
            loaded += 1;
        }
    }

    assert!(loaded > 0, "no .wasm files found in {dir:?}");
    println!("{loaded} tools loaded successfully");
}
