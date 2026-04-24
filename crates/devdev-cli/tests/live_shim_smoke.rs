//! One-shot live sanity check: spawns Copilot via the ACP backend and confirms
//! the WinFSP realpath shim executes in-process. Emits the shim's stderr log
//! so the test operator can eyeball the "[devdev-shim] patched" line.
//!
//! Runs only when `DEVDEV_LIVE_COPILOT=1`.

#![cfg(target_os = "windows")]

use std::process::Stdio;
use std::time::Duration;

fn live_enabled() -> bool {
    std::env::var("DEVDEV_LIVE_COPILOT")
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn which_windows(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(';') {
        for ext in &[".cmd", ".bat", ".exe"] {
            let candidate = std::path::Path::new(dir).join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

#[test]
#[ignore = "live: requires copilot CLI; run with DEVDEV_LIVE_COPILOT=1 --ignored"]
fn shim_runs_inside_copilot_child_process() {
    if !live_enabled() {
        eprintln!("skipped: DEVDEV_LIVE_COPILOT != 1");
        return;
    }
    let Some(copilot) = which_windows("copilot") else {
        eprintln!("skipped: copilot CLI not on PATH");
        return;
    };

    let overrides = devdev_cli::realpath_shim::prepare_nodejs_options();
    assert!(!overrides.is_empty(), "shim should produce NODE_OPTIONS on Windows");
    let (k, v) = &overrides[0];
    eprintln!("[shim-test] {k}={v}");

    let mut cmd = std::process::Command::new(&copilot);
    cmd.args(["--acp", "--allow-all-tools"])
        .env(k, v)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn copilot");
    // Give Node enough time to run the preload; then drop stdin to trigger shutdown.
    std::thread::sleep(Duration::from_secs(3));
    drop(child.stdin.take());
    let _ = child.kill();
    let out = child.wait_with_output().expect("wait copilot");
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprintln!("---stderr---\n{stderr}\n------");
    assert!(
        stderr.contains("[devdev-shim] loaded"),
        "shim did not load in Copilot child. stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("[devdev-shim] patched"),
        "shim did not complete patching. stderr:\n{stderr}"
    );
}
