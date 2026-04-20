//! E2E tests gated on `DEVDEV_E2E=1` plus an on-PATH `copilot` plus a
//! `GH_TOKEN` / `GITHUB_TOKEN`. All are `#[ignore]`d so `cargo test`
//! skips them by default. Run with:
//!
//! ```sh
//! DEVDEV_E2E=1 cargo test -p devdev-cli --test e2e -- --ignored
//! ```
//!
//! These tests shell out to the real `devdev` binary (no mock agent).
//! They seed tempdirs programmatically — nothing is committed under
//! `tests/fixtures/`. (capability 14, AC-E1..AC-E5.)

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn devdev_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devdev"))
}

/// Skip the test with a printed reason when prerequisites aren't met.
/// Returns `true` when the test should proceed.
fn e2e_enabled() -> bool {
    if std::env::var("DEVDEV_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping: DEVDEV_E2E != 1");
        return false;
    }
    if std::env::var("GH_TOKEN").is_err() && std::env::var("GITHUB_TOKEN").is_err() {
        eprintln!("skipping: neither GH_TOKEN nor GITHUB_TOKEN is set");
        return false;
    }
    if which_copilot().is_none() {
        eprintln!("skipping: `copilot` not found on PATH");
        return false;
    }
    true
}

fn which_copilot() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let candidate = dir.join(format!("copilot{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn base_args(repo: &Path, task: &str) -> Vec<String> {
    vec![
        "eval".into(),
        "--repo".into(),
        repo.display().to_string(),
        "--task".into(),
        task.into(),
        "--json".into(),
    ]
}

fn seed_simple_repo(files: &[(&str, &str)]) -> TempDir {
    let td = TempDir::new().expect("tempdir");
    for (name, body) in files {
        let path = td.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
    }
    td
}

fn seed_git_repo(files: &[(&str, &str)], subject: &str) -> TempDir {
    let td = seed_simple_repo(files);
    let repo = git2::Repository::init(td.path()).expect("git init");
    let mut index = repo.index().unwrap();
    for (name, _) in files {
        index.add_path(Path::new(name)).unwrap();
    }
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("DevDev Test", "test@devdev").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, subject, &tree, &[])
        .expect("commit");
    td
}

fn run_devdev(args: &[String], extra_timeout: Duration) -> std::process::Output {
    let mut cmd = Command::new(devdev_bin());
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Hard wall-clock so a broken test cannot hang CI forever, even
    // though `devdev` has its own --timeout flag.
    let child = cmd.spawn().expect("spawn devdev");
    let started = Instant::now();
    let out = wait_with_limit(child, extra_timeout);
    eprintln!(
        "devdev e2e took {:.2}s (exit {:?})",
        started.elapsed().as_secs_f64(),
        out.status.code()
    );
    out
}

fn wait_with_limit(mut child: std::process::Child, limit: Duration) -> std::process::Output {
    let start = Instant::now();
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => return child.wait_with_output().expect("wait_with_output"),
            None => {
                if start.elapsed() > limit {
                    let _ = child.kill();
                    return child.wait_with_output().expect("wait_with_output after kill");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn parse_json(stdout: &[u8]) -> serde_json::Value {
    let s = std::str::from_utf8(stdout).expect("utf-8 stdout");
    serde_json::from_str(s.trim())
        .unwrap_or_else(|e| panic!("stdout is not JSON:\n{s}\nerror: {e}"))
}

// ─── AC-E1 e2e_simple_eval ────────────────────────────────────────────

#[test]
#[ignore]
fn e2e_simple_eval() {
    if !e2e_enabled() {
        return;
    }
    let repo = seed_simple_repo(&[
        ("README.md", "# Test\n"),
        ("src/lib.rs", "pub fn add(a:i32,b:i32)->i32{a+b}\n"),
        ("src/main.rs", "fn main(){}\n"),
    ]);

    let out = run_devdev(
        &base_args(repo.path(), "Say hello and list the files you see."),
        Duration::from_secs(300),
    );

    assert!(
        out.status.success(),
        "devdev exited {:?}:\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let v = parse_json(&out.stdout);
    let verdict = v["verdict"].as_str().unwrap_or("");
    assert!(!verdict.trim().is_empty(), "verdict should be non-empty");
    assert_eq!(v["stop_reason"].as_str().unwrap(), "endTurn");
}

// ─── AC-E2 e2e_tool_execution ─────────────────────────────────────────

#[test]
#[ignore]
fn e2e_tool_execution() {
    if !e2e_enabled() {
        return;
    }
    // A distinctive token the agent can only have seen via a tool
    // call into the VFS.
    let token = "XYZZY_DEVDEV_E2E_MARKER_42";
    let body = format!("marker inside file: {token}\n");
    let repo = seed_simple_repo(&[("notes.txt", &body)]);

    let out = run_devdev(
        &base_args(
            repo.path(),
            "Read notes.txt with a shell command and tell me exactly what token appears on the single line inside the file.",
        ),
        Duration::from_secs(300),
    );

    assert!(out.status.success(), "devdev failed: {:?}", out.status.code());
    let v = parse_json(&out.stdout);
    let verdict = v["verdict"].as_str().unwrap_or("");
    assert!(
        verdict.contains(token),
        "verdict missing token — agent never read the file:\n{verdict}"
    );
    let tool_calls = v["tool_calls"].as_array().unwrap();
    assert!(
        !tool_calls.is_empty(),
        "expected at least one tool call in the transcript"
    );
}

// ─── AC-E3 e2e_file_modification ──────────────────────────────────────

#[test]
#[ignore]
fn e2e_file_modification() {
    if !e2e_enabled() {
        return;
    }
    // We can't observe the VFS from outside the devdev process, so
    // instead we ask the agent to write a sentinel it CAN only know
    // by writing the file, then read it back via a second tool call
    // it reports in the verdict.
    let repo = seed_simple_repo(&[("README.md", "# test\n")]);

    let out = run_devdev(
        &base_args(
            repo.path(),
            "Create a file NOTES.md containing the single word 'BANANA', then use a shell command to verify its contents and quote them back in your final answer.",
        ),
        Duration::from_secs(300),
    );

    assert!(out.status.success());
    let v = parse_json(&out.stdout);
    let verdict = v["verdict"].as_str().unwrap_or("");
    assert!(
        verdict.to_uppercase().contains("BANANA"),
        "verdict should quote the sentinel:\n{verdict}"
    );
}

// ─── AC-E4 e2e_git_operations ─────────────────────────────────────────

#[test]
#[ignore]
fn e2e_git_operations() {
    if !e2e_enabled() {
        return;
    }
    let subject = "Seed commit: DEVDEV_E2E_COMMIT_SUBJECT";
    let repo = seed_git_repo(&[("README.md", "# test\n")], subject);

    let out = run_devdev(
        &base_args(
            repo.path(),
            "Run `git log` and quote the subject line of the most recent commit verbatim in your answer.",
        ),
        Duration::from_secs(300),
    );

    assert!(out.status.success());
    let v = parse_json(&out.stdout);
    let verdict = v["verdict"].as_str().unwrap_or("");
    assert!(
        verdict.contains("DEVDEV_E2E_COMMIT_SUBJECT"),
        "verdict missing commit subject:\n{verdict}"
    );
    assert_eq!(
        v["is_git_repo"].as_bool(),
        Some(true),
        "is_git_repo should be true"
    );
}

// ─── AC-E5 e2e_timeout_graceful ───────────────────────────────────────

#[test]
#[ignore]
fn e2e_timeout_graceful() {
    if !e2e_enabled() {
        return;
    }
    let repo = seed_simple_repo(&[("README.md", "# test\n")]);

    let mut args = base_args(
        repo.path(),
        "Take as long as you need. Think very hard about every file.",
    );
    // Remove --json for this one — we care about the exit code and
    // stderr shape.
    args.retain(|s| s != "--json");
    args.push("--timeout".into());
    args.push("5".into());

    let out = run_devdev(&args, Duration::from_secs(60));
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 on timeout; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("timed out") || stderr.to_lowercase().contains("timeout"),
        "stderr should mention the timeout:\n{stderr}"
    );
}
