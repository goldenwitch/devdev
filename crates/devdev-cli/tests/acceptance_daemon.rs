//! Acceptance tests for the `devdev up / down / status` subcommands.
//!
//! These tests drive the library entry points directly (instead of
//! spawning the binary) so they don't depend on `assert_cmd` and
//! finish in milliseconds. The real binary just wraps the same
//! entry points with clap parsing.
//!
//! Response *shape* is verified via `IpcClient` directly (cleaner
//! than capturing stdout across platforms); `run_status` is then
//! invoked for its side-effect assertions (no error, prints
//! something without panicking).

use std::path::Path;
use std::time::{Duration, Instant};

use devdev_cli::daemon_cli::{
    run_down, run_status, run_up, DownArgs, StatusArgs, UpArgs,
};
use devdev_daemon::ipc::{read_port, IpcClient};

fn up_args(data_dir: &Path) -> UpArgs {
    UpArgs {
        data_dir: Some(data_dir.to_path_buf()),
        checkpoint: false,
        foreground: true,
        github: Some("mock".into()),
        agent_program: "copilot".into(),
        agent_arg: Vec::new(),
    }
}

async fn wait_for_port_file(data_dir: &Path) -> bool {
    let port_file = data_dir.join("daemon.port");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if port_file.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test]
async fn up_status_down_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    let up_dir = data_dir.clone();
    let up_task = tokio::spawn(async move { run_up(up_args(&up_dir)).await });

    assert!(
        wait_for_port_file(&data_dir).await,
        "daemon never wrote daemon.port"
    );
    assert!(data_dir.join("daemon.pid").exists(), "pid file missing");

    // Verify status shape directly via IPC.
    let port = read_port(&data_dir)
        .unwrap()
        .expect("port file exists but parse failed");
    let mut client = IpcClient::connect(port).await.unwrap();
    let resp = client
        .request("status", serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        resp.error.is_none(),
        "status returned error: {:?}",
        resp.error
    );
    let result = resp.result.expect("no result");
    assert!(result.get("tasks").is_some(), "missing tasks key: {result}");
    assert!(
        result.get("sessions").is_some(),
        "missing sessions key: {result}"
    );
    drop(client);

    // Smoke-test the CLI entry point (must not error).
    run_status(StatusArgs {
        data_dir: Some(data_dir.clone()),
        json: true,
    })
    .await
    .expect("run_status failed");

    // Shut down via the CLI entry point.
    run_down(DownArgs {
        data_dir: Some(data_dir.clone()),
    })
    .await
    .expect("down failed");

    // The daemon task should exit cleanly.
    let joined = tokio::time::timeout(Duration::from_secs(5), up_task)
        .await
        .expect("daemon did not exit within 5s")
        .expect("up task panicked");
    joined.expect("up returned error");

    assert!(
        !data_dir.join("daemon.pid").exists(),
        "pid file not removed"
    );
}

#[tokio::test]
async fn down_with_no_daemon_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let result = run_down(DownArgs {
        data_dir: Some(tmp.path().to_path_buf()),
    })
    .await;

    let err = result.expect_err("down without a daemon should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("daemon not running") || msg.contains("no port file"),
        "expected 'daemon not running' message, got: {msg}"
    );
}
