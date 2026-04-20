//! Deterministic acceptance tests for the `devdev` binary
//! (capability 14, AC-01..AC-07).
//!
//! These tests drive the real `devdev` executable as a subprocess via
//! `CARGO_BIN_EXE_devdev`. For cases that need an ACP peer, we point
//! `--agent-program` at the `devdev-fake-agent` sibling binary which
//! replays a scripted NDJSON conversation — no network, no Copilot,
//! no tokio dance in the test itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

// ── Paths to compiled helpers ───────────────────────────────────────────

fn devdev_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devdev"))
}

fn fake_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devdev-fake-agent"))
}

/// Empty tempdir with a single tiny file so `load_repo` has something
/// to hash.
fn tiny_repo() -> TempDir {
    let td = TempDir::new().expect("tempdir");
    std::fs::write(td.path().join("README.md"), "hi").expect("write readme");
    td
}

/// Common argv prefix for `devdev eval --repo <repo> --task x`.
fn base_args(repo: &Path) -> Vec<String> {
    vec![
        "eval".into(),
        "--repo".into(),
        repo.display().to_string(),
        "--task".into(),
        "check everything".into(),
    ]
}

// ─── AC-01 clap_parses_minimum_args ────────────────────────────────────
//
// We can't introspect clap's parse result from outside the process, so
// instead we assert the binary accepts the minimum argv (does not exit
// with the clap code 2) when pointed at a bogus agent. The expected
// outcome is an exit-1 "could not spawn agent" error — not a parse
// error.

#[test]
fn ac_01_clap_parses_minimum_args() {
    let repo = tiny_repo();
    let out = Command::new(devdev_bin())
        .args(base_args(repo.path()))
        .arg("--agent-program")
        .arg("__definitely_not_a_real_binary__")
        .output()
        .expect("spawn devdev");

    // Exit 2 is clap's usage error. Anything else means the parse
    // succeeded — which is what we want to prove.
    assert_ne!(
        out.status.code(),
        Some(2),
        "clap rejected the minimal invocation:\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ─── AC-02 clap_rejects_missing_required ───────────────────────────────

#[test]
fn ac_02_clap_rejects_missing_required() {
    let out = Command::new(devdev_bin())
        .args(["eval", "--task", "x"])
        .output()
        .expect("spawn devdev");

    assert_eq!(out.status.code(), Some(2), "expected clap exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--repo") || stderr.to_lowercase().contains("required"),
        "stderr did not mention the missing flag:\n{stderr}"
    );
}

// ─── AC-03 workspace_limit_prints_clean_error ──────────────────────────

#[test]
fn ac_03_workspace_limit_prints_clean_error() {
    let td = TempDir::new().unwrap();
    std::fs::write(td.path().join("file.txt"), "this is more than sixteen bytes long").unwrap();

    let out = Command::new(devdev_bin())
        .args(base_args(td.path()))
        .args(["--workspace-limit", "16"])
        // Point at a guaranteed-invalid agent so that IF the VFS check
        // didn't fire, the subprocess spawn would fail with a
        // DIFFERENT message. This is our guarantee that no subprocess
        // was spawned.
        .arg("--agent-program")
        .arg("__devdev_test_unreachable__")
        .output()
        .expect("spawn devdev");

    assert_eq!(out.status.code(), Some(1), "expected exit 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("repo too large"),
        "stderr missing 'repo too large':\n{stderr}"
    );
    assert!(
        !stderr.contains("__devdev_test_unreachable__"),
        "subprocess spawn was attempted — the bogus agent name leaked into stderr:\n{stderr}"
    );
}

// ─── AC-04 json_output_matches_snapshot ────────────────────────────────

#[test]
fn ac_04_json_output_matches_snapshot() {
    let repo = tiny_repo();
    let out = Command::new(devdev_bin())
        .args(base_args(repo.path()))
        .arg("--json")
        .arg("--agent-program")
        .arg(fake_agent_bin())
        .output()
        .expect("spawn devdev");

    assert!(
        out.status.success(),
        "devdev exited {:?}:\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not JSON:\n{stdout}\nerror: {e}"));

    // Every documented field, with the expected shape.
    assert!(v.get("verdict").and_then(|x| x.as_str()).is_some());
    assert!(v.get("stop_reason").and_then(|x| x.as_str()).is_some());
    assert!(v.get("tool_calls").and_then(|x| x.as_array()).is_some());
    assert!(v.get("duration_ms").and_then(|x| x.as_u64()).is_some());
    assert!(v.get("is_git_repo").and_then(|x| x.as_bool()).is_some());
    let stats = v
        .get("repo_stats")
        .and_then(|x| x.as_object())
        .expect("repo_stats");
    assert!(stats.get("files").and_then(|x| x.as_u64()).is_some());
    assert!(stats.get("bytes").and_then(|x| x.as_u64()).is_some());

    let tool_calls = v["tool_calls"].as_array().unwrap();
    assert!(
        !tool_calls.is_empty(),
        "fake agent should have produced at least one tool call"
    );
    for tc in tool_calls {
        assert!(tc.get("command").and_then(|x| x.as_str()).is_some());
        assert!(tc.get("exit_code").and_then(|x| x.as_i64()).is_some());
        assert!(tc.get("duration_ms").and_then(|x| x.as_u64()).is_some());
    }

    assert_eq!(v["stop_reason"].as_str().unwrap(), "endTurn");
}

// ─── AC-05 human_output_lists_each_tool_call ───────────────────────────

#[test]
fn ac_05_human_output_lists_each_tool_call() {
    let repo = tiny_repo();
    let out = Command::new(devdev_bin())
        .args(base_args(repo.path()))
        .arg("--agent-program")
        .arg(fake_agent_bin())
        .output()
        .expect("spawn devdev");

    assert!(out.status.success(), "devdev exited {:?}", out.status.code());
    let stdout = String::from_utf8(out.stdout).expect("utf-8");

    // The fake agent always issues exactly `echo hello` as its only
    // tool call.
    assert!(
        stdout.contains("echo"),
        "human output should list the echo tool call:\n{stdout}"
    );
    assert!(
        stdout.contains("Evaluation complete"),
        "human output should end with 'Evaluation complete':\n{stdout}"
    );
    assert!(
        stdout.contains("Verdict"),
        "human output should include a Verdict section:\n{stdout}"
    );
}

// ─── AC-06 verbose_flag_enables_debug_tracing ──────────────────────────

#[test]
fn ac_06_verbose_flag_enables_debug_tracing() {
    let repo = tiny_repo();
    let out = Command::new(devdev_bin())
        .args(base_args(repo.path()))
        .arg("--verbose")
        .arg("--agent-program")
        .arg(fake_agent_bin())
        // Clear the host's RUST_LOG so it can't override the CLI's
        // default.
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn devdev");

    assert!(
        out.status.success(),
        "devdev exited {:?}:\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8(out.stderr).expect("utf-8");
    assert!(
        stderr.contains("DEBUG") || stderr.contains("debug"),
        "--verbose should produce DEBUG lines on stderr:\n{stderr}"
    );
}

// ─── AC-07 trace_file_written ──────────────────────────────────────────

#[test]
fn ac_07_trace_file_written() {
    let repo = tiny_repo();
    let tracedir = TempDir::new().unwrap();
    let tracefile = tracedir.path().join("trace.log");

    let out = Command::new(devdev_bin())
        .args(base_args(repo.path()))
        .arg("--trace-file")
        .arg(&tracefile)
        .arg("--agent-program")
        .arg(fake_agent_bin())
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn devdev");

    assert!(
        out.status.success(),
        "devdev exited {:?}:\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(tracefile.exists(), "trace file not created");
    let body = std::fs::read_to_string(&tracefile).expect("read trace file");
    assert!(!body.is_empty(), "trace file is empty");
    assert!(
        body.contains("acp::init"),
        "trace file missing acp::init record:\n{body}"
    );
}
