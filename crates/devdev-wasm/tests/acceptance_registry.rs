//! Acceptance tests for Cap 04 — Tool Registry & Dispatch.
//!
//! Each test maps to one bullet in `capabilities/04-tool-registry.md`'s
//! Acceptance Criteria block. WASM-backed tests are gated behind `#[ignore]`
//! because the `.wasm` artifacts are not checked into git; run them with
//! `cargo test -p devdev-wasm --test acceptance_registry -- --ignored`.

use std::collections::HashMap;
use std::path::Path;

use devdev_vfs::MemFs;
use devdev_wasm::{ToolEngine, WasmToolRegistry};

fn env() -> HashMap<String, String> {
    HashMap::new()
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

fn fs_with(files: &[(&str, &str)]) -> MemFs {
    let mut fs = MemFs::new();
    for (path, body) in files {
        // Create parent directories on demand.
        if let Some(parent) = Path::new(path).parent() {
            let _ = fs.mkdir_p(parent);
        }
        fs.write(Path::new(path), body.as_bytes()).unwrap();
    }
    fs
}

// ── Native dispatch ──────────────────────────────────────────────

/// AC: `execute("grep", &["-rn", "TODO", "src/"])` searches the VFS tree.
#[test]
fn grep_recursive_matches_tree() {
    let fs = fs_with(&[
        ("/src/a.rs", "fn main() {\n    // TODO first\n}\n"),
        ("/src/sub/b.rs", "// nothing here\n"),
        ("/src/sub/c.rs", "let x = 1; // TODO second\n"),
    ]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "grep",
        &args(&["-rn", "TODO", "src/"]),
        &[],
        &env(),
        "/",
        &fs,
    );
    assert_eq!(r.exit_code, 0);
    let stdout = String::from_utf8(r.stdout).unwrap();
    assert!(stdout.contains("src/a.rs:2:"), "got: {stdout}");
    assert!(stdout.contains("src/sub/c.rs:1:"), "got: {stdout}");
    assert!(!stdout.contains("src/sub/b.rs"));
}

/// AC: `grep -n foo file.txt` output format is `path:line:content`.
#[test]
fn grep_line_number_format() {
    let fs = fs_with(&[("/x.txt", "one\nfoo line\nthree\nfoo again\n")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "grep",
        &args(&["-n", "foo", "x.txt"]),
        &[],
        &env(),
        "/",
        &fs,
    );
    assert_eq!(r.exit_code, 0);
    let stdout = String::from_utf8(r.stdout).unwrap();
    // Single file → no filename prefix, only lineno prefix.
    assert_eq!(stdout, "2:foo line\n4:foo again\n");
}

/// AC: `grep` exits 1 when pattern not found.
#[test]
fn grep_no_match_exit_one() {
    let fs = fs_with(&[("/x.txt", "just some text\n")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "grep",
        &args(&["missing", "x.txt"]),
        &[],
        &env(),
        "/",
        &fs,
    );
    assert_eq!(r.exit_code, 1);
    assert!(r.stdout.is_empty());
}

/// AC: `find . -name '*.rs' -type f` lists matching files.
#[test]
fn find_name_type_file() {
    let fs = fs_with(&[
        ("/a.rs", "x"),
        ("/b.txt", "x"),
        ("/sub/c.rs", "x"),
        ("/sub/d.md", "x"),
    ]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "find",
        &args(&[".", "-name", "*.rs", "-type", "f"]),
        &[],
        &env(),
        "/",
        &fs,
    );
    assert_eq!(r.exit_code, 0);
    let lines: Vec<&str> = std::str::from_utf8(&r.stdout)
        .unwrap()
        .lines()
        .collect();
    assert!(lines.contains(&"./a.rs"), "lines: {lines:?}");
    assert!(lines.contains(&"./sub/c.rs"), "lines: {lines:?}");
    assert!(!lines.contains(&"./b.txt"));
    assert!(!lines.contains(&"./sub/d.md"));
}

/// AC: find respects `-maxdepth`.
#[test]
fn find_maxdepth_limits_recursion() {
    let fs = fs_with(&[("/a.txt", "x"), ("/sub/b.txt", "x"), ("/sub/deep/c.txt", "x")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "find",
        &args(&[".", "-maxdepth", "1"]),
        &[],
        &env(),
        "/",
        &fs,
    );
    assert_eq!(r.exit_code, 0);
    let out = String::from_utf8(r.stdout).unwrap();
    assert!(out.contains("./a.txt"));
    assert!(out.contains("./sub"));
    assert!(!out.contains("./sub/b.txt"), "depth-2 leaked: {out}");
}

/// AC: `diff a b` produces a unified diff.
#[test]
fn diff_unified_output() {
    let fs = fs_with(&[
        ("/a.txt", "one\ntwo\nthree\n"),
        ("/b.txt", "one\nTWO\nthree\n"),
    ]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "diff",
        &args(&["a.txt", "b.txt"]),
        &[],
        &env(),
        "/",
        &fs,
    );
    assert_eq!(r.exit_code, 1);
    let out = String::from_utf8(r.stdout).unwrap();
    assert!(out.starts_with("--- a/a.txt\n+++ b/b.txt\n"), "got: {out}");
    assert!(out.contains("-two"), "got: {out}");
    assert!(out.contains("+TWO"), "got: {out}");
}

/// AC: `diff` returns 0 on identical files.
#[test]
fn diff_identical_exit_zero() {
    let fs = fs_with(&[("/a", "same\n"), ("/b", "same\n")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute("diff", &args(&["a", "b"]), &[], &env(), "/", &fs);
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.is_empty());
}

// ── Dispatch semantics ──────────────────────────────────────────

/// AC: unknown command returns exit 127 with "command not found" on stderr.
#[test]
fn unknown_command_exit_127() {
    let fs = MemFs::new();
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute("nonexistent", &[], &[], &env(), "/", &fs);
    assert_eq!(r.exit_code, 127);
    assert!(r.stdout.is_empty());
    let stderr = String::from_utf8(r.stderr).unwrap();
    assert_eq!(stderr, "command not found: nonexistent\n");
}

/// AC: awk falls through to 127 (deferred to P2).
#[test]
fn awk_not_implemented_exit_127() {
    let fs = MemFs::new();
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute("awk", &args(&["{print}"]), &[], &env(), "/", &fs);
    assert_eq!(r.exit_code, 127);
    assert_eq!(
        String::from_utf8(r.stderr).unwrap(),
        "command not found: awk\n"
    );
}

/// AC: `has_tool` reports both WASM and native backends.
#[test]
fn has_tool_reports_both_backends() {
    let reg = WasmToolRegistry::new().unwrap();
    assert!(reg.has_tool("cat"), "WASM tool missing");
    assert!(reg.has_tool("grep"), "native tool missing");
    assert!(reg.has_tool("find"));
    assert!(reg.has_tool("diff"));
    assert!(!reg.has_tool("vim"));
}

/// AC: `available_tools()` returns a merged list.
#[test]
fn available_tools_merged() {
    let reg = WasmToolRegistry::new().unwrap();
    let tools = reg.available_tools();
    // Native tools must be present even though they aren't WASM-backed.
    assert!(tools.contains(&"grep"));
    assert!(tools.contains(&"find"));
    assert!(tools.contains(&"diff"));
    // WASM tools present.
    assert!(tools.contains(&"cat"));
    assert!(tools.contains(&"wc"));
}

// ── WASM backend smoke test ─────────────────────────────────────

/// AC: `execute("cat", &["-"], stdin=b"hello\n")` routes through the WASM
/// backend and reproduces stdin on stdout. `#[ignore]` because `.wasm`
/// artifacts aren't in the repo.
#[test]
#[ignore]
fn cat_wasm_dispatch() {
    let fs = MemFs::new();
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "cat",
        &[],
        b"hello from cat\n",
        &env(),
        "/",
        &fs,
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, b"hello from cat\n");
}
