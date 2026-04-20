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
    let mut fs = fs_with(&[
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
        &mut fs,
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
    let mut fs = fs_with(&[("/x.txt", "one\nfoo line\nthree\nfoo again\n")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "grep",
        &args(&["-n", "foo", "x.txt"]),
        &[],
        &env(),
        "/",
        &mut fs,
    );
    assert_eq!(r.exit_code, 0);
    let stdout = String::from_utf8(r.stdout).unwrap();
    // Single file → no filename prefix, only lineno prefix.
    assert_eq!(stdout, "2:foo line\n4:foo again\n");
}

/// AC: `grep` exits 1 when pattern not found.
#[test]
fn grep_no_match_exit_one() {
    let mut fs = fs_with(&[("/x.txt", "just some text\n")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "grep",
        &args(&["missing", "x.txt"]),
        &[],
        &env(),
        "/",
        &mut fs,
    );
    assert_eq!(r.exit_code, 1);
    assert!(r.stdout.is_empty());
}

/// AC: `find . -name '*.rs' -type f` lists matching files.
#[test]
fn find_name_type_file() {
    let mut fs = fs_with(&[
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
        &mut fs,
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
    let mut fs = fs_with(&[("/a.txt", "x"), ("/sub/b.txt", "x"), ("/sub/deep/c.txt", "x")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "find",
        &args(&[".", "-maxdepth", "1"]),
        &[],
        &env(),
        "/",
        &mut fs,
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
    let mut fs = fs_with(&[
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
        &mut fs,
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
    let mut fs = fs_with(&[("/a", "same\n"), ("/b", "same\n")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute("diff", &args(&["a", "b"]), &[], &env(), "/", &mut fs);
    assert_eq!(r.exit_code, 0);
    assert!(r.stdout.is_empty());
}

// ── Dispatch semantics ──────────────────────────────────────────

/// AC: unknown command returns exit 127 with "command not found" on stderr.
#[test]
fn unknown_command_exit_127() {
    let mut fs = MemFs::new();
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute("nonexistent", &[], &[], &env(), "/", &mut fs);
    assert_eq!(r.exit_code, 127);
    assert!(r.stdout.is_empty());
    let stderr = String::from_utf8(r.stderr).unwrap();
    assert_eq!(stderr, "command not found: nonexistent\n");
}

/// AC: awk falls through to 127 (deferred to P2).
#[test]
fn awk_not_implemented_exit_127() {
    let mut fs = MemFs::new();
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute("awk", &args(&["{print}"]), &[], &env(), "/", &mut fs);
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
    let mut fs = MemFs::new();
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "cat",
        &[],
        b"hello from cat\n",
        &env(),
        "/",
        &mut fs,
    );
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout, b"hello from cat\n");
}

// ── VFS bridge tests (require .wasm artifacts) ─────────────────

/// AC: `cat file.txt` reads a file from the VFS via the WASI preopen bridge.
/// This is the core proof that the VFS→tempdir→WASI preopens path works.
#[test]
#[ignore]
fn cat_reads_vfs_file_via_bridge() {
    let mut fs = fs_with(&[("/hello.txt", "bridged content\n")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "cat",
        &args(&["cat", "/hello.txt"]),
        &[],
        &env(),
        "/",
        &mut fs,
    );
    assert_eq!(r.exit_code, 0, "stderr: {}", String::from_utf8_lossy(&r.stderr));
    assert_eq!(String::from_utf8(r.stdout).unwrap(), "bridged content\n");
}

/// AC: `ls /src` lists VFS directory contents via the WASI preopen bridge.
#[test]
#[ignore]
fn ls_lists_vfs_directory_via_bridge() {
    let mut fs = fs_with(&[
        ("/src/main.rs", "fn main() {}"),
        ("/src/lib.rs", "// lib"),
    ]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "ls",
        &args(&["ls", "/src"]),
        &[],
        &env(),
        "/",
        &mut fs,
    );
    assert_eq!(r.exit_code, 0, "stderr: {}", String::from_utf8_lossy(&r.stderr));
    let out = String::from_utf8(r.stdout).unwrap();
    assert!(out.contains("main.rs"), "got: {out}");
    assert!(out.contains("lib.rs"), "got: {out}");
}

/// AC: `touch /new.txt` creates a file that is synced back to the VFS.
#[test]
#[ignore]
fn touch_creates_file_synced_to_vfs() {
    let mut fs = MemFs::new();
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "touch",
        &args(&["touch", "/created.txt"]),
        &[],
        &env(),
        "/",
        &mut fs,
    );
    assert_eq!(r.exit_code, 0, "stderr: {}", String::from_utf8_lossy(&r.stderr));
    // The file should now exist in the VFS after sync-back.
    assert!(fs.exists(Path::new("/created.txt")), "touch did not sync back to VFS");
}

/// AC: `cp src dst` copies a VFS file and syncs the copy back.
#[test]
#[ignore]
fn cp_copies_file_synced_to_vfs() {
    let mut fs = fs_with(&[("/original.txt", "payload\n")]);
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "cp",
        &args(&["cp", "/original.txt", "/copy.txt"]),
        &[],
        &env(),
        "/",
        &mut fs,
    );
    assert_eq!(r.exit_code, 0, "stderr: {}", String::from_utf8_lossy(&r.stderr));
    assert!(fs.exists(Path::new("/copy.txt")), "cp did not sync back to VFS");
    assert_eq!(
        fs.read(Path::new("/copy.txt")).unwrap(),
        b"payload\n",
        "copied content mismatch",
    );
    // Original must still exist.
    assert!(fs.exists(Path::new("/original.txt")));
}

/// AC: `rm /file.txt` deletes a VFS file and the deletion syncs back.
#[test]
#[ignore]
fn rm_deletes_file_synced_to_vfs() {
    let mut fs = fs_with(&[("/doomed.txt", "goodbye\n")]);
    assert!(fs.exists(Path::new("/doomed.txt")));
    let reg = WasmToolRegistry::new().unwrap();
    let r = reg.execute(
        "rm",
        &args(&["rm", "/doomed.txt"]),
        &[],
        &env(),
        "/",
        &mut fs,
    );
    assert_eq!(r.exit_code, 0, "stderr: {}", String::from_utf8_lossy(&r.stderr));
    assert!(!fs.exists(Path::new("/doomed.txt")), "rm did not sync deletion to VFS");
}
