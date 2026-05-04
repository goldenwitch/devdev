//! Live integration test for the daemon-integrated MCP-fs-write path.
//!
//! Claim under test (`DAEMON-AGENT-FS-WRITE` in `claims.toml`):
//! DevDev can inject arbitrary tools into the agent's tool list, and
//! the agent will use them to mutate daemon-owned state. Specifically:
//! a running `devdev` daemon, driven through its normal IPC surface
//! by a user prompt, routes the agent through our MCP server; the
//! agent calls our injected `devdev_fs_write` tool; the byte pattern
//! we asked for lands in the daemon's in-memory `Fs`; we read it back
//! through the same IPC surface a user would use.
//!
//! This is the *containment* proof: every future policy hook,
//! audit ledger, virtual-git substitution, or network gate rides on
//! the agent choosing our injected tool over a stock one.
//!
//! ## What makes this proof honest (per spirit/05-validation.md)
//!
//! - **Real path.** Live `copilot --acp --allow-all-tools` subprocess.
//!   Real `run_up`. Real `IpcServer`/`IpcClient`. Real `McpServer`
//!   over loopback HTTP. Real `DaemonToolProvider` wrapping the
//!   daemon's `Arc<Mutex<Fs>>`. No mocks in the hot path.
//! - **DevDev-specific failure mode.** Fails if the MCP server isn't
//!   wired to the daemon's Fs, if the tool isn't registered, if the
//!   router drops the MCP endpoint, if the agent's tool allow-list
//!   excludes MCP, or if the `fs/read` IPC plumbing is broken.
//! - **Claim matches assertion.** The nonce in the written bytes is
//!   generated per-test-run; the only way it lands in Fs is via the
//!   agent invoking the tool we fed it. No pre-seeding, no echo.
//!
//! ## What this deliberately does NOT prove
//!
//! - That the Fs is mounted as a real host filesystem. That's
//!   `AGENT-FS-WRITE`'s job.
//! - That multiple concurrent sessions share Fs coherently.
//! - That the tool description is good enough for *every* model; we
//!   prompt Copilot explicitly to call `devdev_fs_write`.
//!
//! ## Running
//!
//! Gated behind `--ignored` and `DEVDEV_LIVE_COPILOT=1`:
//!
//! ```powershell
//! $env:DEVDEV_LIVE_COPILOT = "1"
//! cargo test -p devdev-cli --test live_daemon_fs_write -- --ignored --nocapture
//! ```

use std::path::Path;
use std::time::{Duration, Instant};

use devdev_cli::daemon_cli::{UpArgs, run_up};
use devdev_daemon::ipc::{IpcClient, read_port};

fn live_enabled() -> bool {
    std::env::var("DEVDEV_LIVE_COPILOT")
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Resolve the `copilot` binary using the same logic the daemon
/// uses at spawn time. Keeps the test in sync with production: a
/// Windows-specific PATHEXT search via
/// [`devdev_cli::agent_command::resolve_on_path`].
fn which_copilot() -> Option<String> {
    devdev_cli::agent_command::resolve_on_path("copilot")
}

fn init_tracing() {
    let default_filter =
        "devdev_acp::wire=trace,devdev_daemon::mcp=debug,devdev_daemon=debug,devdev_cli=debug,warn";
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .with_target(true)
        .try_init();
}

async fn wait_for_port_file(data_dir: &Path) -> bool {
    let port_file = data_dir.join("daemon.port");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if port_file.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

fn up_args(data_dir: &Path, agent_program: String) -> UpArgs {
    UpArgs {
        data_dir: Some(data_dir.to_path_buf()),
        checkpoint: false,
        foreground: true,
        // Mock adapter: this test isn't about GitHub.
        github: Some("mock".into()),
        agent_program,
        // Clap defaults apply: `--acp --allow-all-tools`.
        agent_arg: vec!["--acp".into(), "--allow-all-tools".into()],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires live, signed-in Copilot CLI; run with DEVDEV_LIVE_COPILOT=1 and --ignored"]
async fn devdev_up_agent_fs_write_lands_in_daemon_fs() {
    if !live_enabled() {
        eprintln!("skipped: DEVDEV_LIVE_COPILOT != 1");
        return;
    }
    init_tracing();

    let copilot_bin = match which_copilot() {
        Some(p) => p,
        None => {
            eprintln!("skipped: `copilot` not on PATH");
            return;
        }
    };
    eprintln!("[daemon-fs-write] copilot binary: {copilot_bin}");

    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();

    // Boot the daemon via the same entry point the binary uses.
    let up_dir = data_dir.clone();
    let up_task = tokio::spawn(async move { run_up(up_args(&up_dir, copilot_bin)).await });

    assert!(
        wait_for_port_file(&data_dir).await,
        "daemon never wrote daemon.port"
    );
    let port = read_port(&data_dir)
        .expect("read port file")
        .expect("port file exists but parse failed");
    eprintln!("[daemon-fs-write] daemon listening on port {port}");

    // Nonce makes the test self-certifying: the bytes we assert on
    // are generated here, so a PASS means the agent transported this
    // exact run's value through MCP → Fs → IPC.
    let nonce = format!("daemon-canary-{}", std::process::id());
    let target_path = "/notes/greeting.txt";
    let prompt = format!(
        "Call the MCP tool `devdev_fs_write` with path=\"{target_path}\" and \
         content=\"{nonce}\" (exactly, no trailing newline). Do not use any \
         shell command or any other tool; use only `devdev_fs_write`. \
         Reply with the single word DONE when the tool call succeeds."
    );
    eprintln!("[daemon-fs-write] prompt: {prompt}");

    // ── 1. Send the prompt. The daemon lazily creates an interactive
    //       session against the real Copilot subprocess, which sees
    //       our MCP endpoint in its server list and can invoke
    //       `devdev_fs_write`.
    let mut client = IpcClient::connect(port).await.expect("ipc connect");
    let send_resp = tokio::time::timeout(
        Duration::from_secs(180),
        client.request("send", serde_json::json!({ "text": prompt })),
    )
    .await
    .expect("send IPC timed out after 180s")
    .expect("send IPC errored");

    if let Some(err) = &send_resp.error {
        panic!("send returned error: {err:?}");
    }
    let send_result = send_resp.result.clone().expect("send: no result");
    let agent_reply = send_result
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let stop_reason = send_result
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    eprintln!("[daemon-fs-write] agent reply: {agent_reply}");
    eprintln!("[daemon-fs-write] stop_reason: {stop_reason}");

    // ── 2. Read the file back through the daemon's Fs via IPC.
    //       This is the proof surface: a user (or this test) observes
    //       the post-state through the same channel the agent used.
    let read_resp = tokio::time::timeout(
        Duration::from_secs(5),
        client.request("fs/read", serde_json::json!({ "path": target_path })),
    )
    .await
    .expect("fs/read IPC timed out")
    .expect("fs/read IPC errored");

    if let Some(err) = &read_resp.error {
        panic!(
            "fs/read returned error: {err:?}. Agent reply was: {agent_reply:?} \
             (stop_reason={stop_reason:?}). Either the agent didn't invoke \
             devdev_fs_write, or the tool didn't reach Fs."
        );
    }
    let read_result = read_resp.result.expect("fs/read: no result");
    let bytes_utf8 = read_result
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("fs/read result missing `content` string field: {read_result}"))
        .to_string();

    // Shut the daemon down cleanly before the assert so a failure
    // doesn't leave a zombie port/pid file behind.
    let _ = client.request("shutdown", serde_json::json!({})).await;
    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(5), up_task).await;

    // ── 3. The assertion. Nonce in == nonce out, exactly.
    assert_eq!(
        bytes_utf8, nonce,
        "bytes in Fs at {target_path} do not match what the agent was asked \
         to write. Agent reply: {agent_reply:?}. stop_reason={stop_reason:?}."
    );
}
