//! Acceptance tests for Cap 08 — Shell Builtins.
//!
//! Each test maps to one acceptance criterion from capabilities/08-shell-builtins.md.

use std::path::Path;

use devdev_shell::{try_builtin, BuiltinResult, ShellState};
use devdev_vfs::MemFs;

fn args(strs: &[&str]) -> Vec<String> {
    strs.iter().map(|s| (*s).to_owned()).collect()
}

/// AC: `cd /some/path` then `pwd` → prints `/some/path`.
#[test]
fn cd_then_pwd() {
    let mut vfs = MemFs::new();
    vfs.mkdir_p(Path::new("/some/path")).unwrap();
    let mut state = ShellState::new();

    let result = try_builtin("cd", &args(&["/some/path"]), &mut state, &vfs);
    assert!(matches!(result, BuiltinResult::Ok { exit_code: 0, .. }));

    let result = try_builtin("pwd", &[], &mut state, &vfs);
    if let BuiltinResult::Ok { stdout, exit_code, .. } = result {
        assert_eq!(exit_code, 0);
        assert_eq!(String::from_utf8(stdout).unwrap(), "/some/path\n");
    } else {
        panic!("expected Ok result");
    }
}

/// AC: `cd` (no args) → cwd is `/`.
#[test]
fn cd_no_args() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();
    state.cwd = std::path::PathBuf::from("/somewhere");

    let result = try_builtin("cd", &[], &mut state, &vfs);
    assert!(matches!(result, BuiltinResult::Ok { exit_code: 0, .. }));
    assert_eq!(state.cwd, Path::new("/"));
}

/// AC: `cd nonexistent` → error message, exit code 1, cwd unchanged.
#[test]
fn cd_nonexistent() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();
    let original_cwd = state.cwd.clone();

    let result = try_builtin("cd", &args(&["/nonexistent"]), &mut state, &vfs);
    if let BuiltinResult::Ok { stderr, exit_code, .. } = result {
        assert_eq!(exit_code, 1);
        let msg = String::from_utf8(stderr).unwrap();
        assert!(msg.contains("no such file or directory"), "got: {msg}");
    } else {
        panic!("expected Ok result with error");
    }
    // cwd unchanged
    assert_eq!(state.cwd, original_cwd);
}

/// AC: `export FOO=bar` then env contains `FOO=bar`.
#[test]
fn export_sets_env() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();

    let result = try_builtin("export", &args(&["FOO=bar"]), &mut state, &vfs);
    assert!(matches!(result, BuiltinResult::Ok { exit_code: 0, .. }));
    assert_eq!(state.env.get("FOO"), Some(&"bar".to_owned()));
}

/// AC: `unset FOO` then env no longer contains `FOO`.
#[test]
fn unset_removes_env() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();
    state.env.insert("FOO".into(), "bar".into());

    let result = try_builtin("unset", &args(&["FOO"]), &mut state, &vfs);
    assert!(matches!(result, BuiltinResult::Ok { exit_code: 0, .. }));
    assert!(!state.env.contains_key("FOO"));
}

/// AC: `echo hello world` → stdout is `"hello world\n"`.
#[test]
fn echo_basic() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();

    let result = try_builtin("echo", &args(&["hello", "world"]), &mut state, &vfs);
    if let BuiltinResult::Ok { stdout, exit_code, .. } = result {
        assert_eq!(exit_code, 0);
        assert_eq!(stdout, b"hello world\n");
    } else {
        panic!("expected Ok result");
    }
}

/// AC: `echo -n hello` → stdout is `"hello"` (no trailing newline).
#[test]
fn echo_no_newline() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();

    let result = try_builtin("echo", &args(&["-n", "hello"]), &mut state, &vfs);
    if let BuiltinResult::Ok { stdout, exit_code, .. } = result {
        assert_eq!(exit_code, 0);
        assert_eq!(stdout, b"hello");
    } else {
        panic!("expected Ok result");
    }
}

/// AC: `exit 42` returns `BuiltinResult::Exit(42)`.
#[test]
fn exit_with_code() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();

    let result = try_builtin("exit", &args(&["42"]), &mut state, &vfs);
    assert_eq!(result, BuiltinResult::Exit(42));
}

/// AC: `try_builtin("grep", ...)` returns `NotBuiltin`.
#[test]
fn non_builtin_falls_through() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();

    let result = try_builtin("grep", &args(&["-rn", "TODO"]), &mut state, &vfs);
    assert_eq!(result, BuiltinResult::NotBuiltin);
}

// ── Additional coverage ─────────────────────────────────────────

/// Verify `exit` with no args defaults to code 0.
#[test]
fn exit_default_zero() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();

    let result = try_builtin("exit", &[], &mut state, &vfs);
    assert_eq!(result, BuiltinResult::Exit(0));
}

/// Verify `cd -` goes to OLDPWD.
#[test]
fn cd_dash_oldpwd() {
    let mut vfs = MemFs::new();
    vfs.mkdir_p(Path::new("/a")).unwrap();
    vfs.mkdir_p(Path::new("/b")).unwrap();
    let mut state = ShellState::new();

    let r = try_builtin("cd", &args(&["/a"]), &mut state, &vfs);
    assert!(matches!(r, BuiltinResult::Ok { exit_code: 0, .. }));
    let r = try_builtin("cd", &args(&["/b"]), &mut state, &vfs);
    assert!(matches!(r, BuiltinResult::Ok { exit_code: 0, .. }));
    assert_eq!(state.cwd, Path::new("/b"));

    let r = try_builtin("cd", &args(&["-"]), &mut state, &vfs);
    assert!(matches!(r, BuiltinResult::Ok { exit_code: 0, .. }));
    assert_eq!(state.cwd, Path::new("/a"));
}

/// Verify `echo` with no args prints just a newline.
#[test]
fn echo_empty() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();

    let result = try_builtin("echo", &[], &mut state, &vfs);
    if let BuiltinResult::Ok { stdout, .. } = result {
        assert_eq!(stdout, b"\n");
    } else {
        panic!("expected Ok result");
    }
}

/// Verify `export` with no args lists all env vars.
#[test]
fn export_list() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();
    state.env.insert("A".into(), "1".into());
    state.env.insert("B".into(), "2".into());

    let result = try_builtin("export", &[], &mut state, &vfs);
    if let BuiltinResult::Ok { stdout, .. } = result {
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("declare -x A=\"1\""));
        assert!(text.contains("declare -x B=\"2\""));
    } else {
        panic!("expected Ok result");
    }
}

/// Verify `unset` of a non-existent variable is not an error (matches bash).
#[test]
fn unset_nonexistent_ok() {
    let vfs = MemFs::new();
    let mut state = ShellState::new();

    let result = try_builtin("unset", &args(&["NOPE"]), &mut state, &vfs);
    assert!(matches!(result, BuiltinResult::Ok { exit_code: 0, .. }));
}
