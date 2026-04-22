//! Acceptance tests for P2-02 — Daemon Lifecycle & IPC.

use std::path::Path;

use devdev_daemon::ipc::{self, IpcClient, IpcResponse, IpcServer};
use devdev_daemon::{Daemon, DaemonConfig, DaemonError};
use tempfile::TempDir;

fn test_config(dir: &TempDir) -> DaemonConfig {
    DaemonConfig {
        data_dir: dir.path().to_path_buf(),
        checkpoint_on_stop: true,
        foreground: true,
    }
}

// ── daemon_start_creates_pid_file ──────────────────────────────

#[tokio::test]
async fn daemon_start_creates_pid_file() {
    let dir = TempDir::new().unwrap();
    let daemon = Daemon::start(test_config(&dir), false).await.unwrap();

    let pid_file = dir.path().join("daemon.pid");
    assert!(pid_file.exists());
    let content = std::fs::read_to_string(&pid_file).unwrap();
    let pid: u32 = content.trim().parse().unwrap();
    assert_eq!(pid, std::process::id());

    daemon.stop().await.unwrap();
}

// ── daemon_double_start_fails ──────────────────────────────────

#[tokio::test]
async fn daemon_double_start_fails() {
    let dir = TempDir::new().unwrap();
    let _daemon = Daemon::start(test_config(&dir), false).await.unwrap();

    // Second start should fail because PID is alive (it's our own process).
    let result = Daemon::start(test_config(&dir), false).await;
    assert!(result.is_err());
    match result.err().unwrap() {
        DaemonError::AlreadyRunning(pid) => {
            assert_eq!(pid, std::process::id());
        }
        e => panic!("expected AlreadyRunning, got: {e}"),
    }

    _daemon.stop().await.unwrap();
}

// ── daemon_stale_pid_cleaned ───────────────────────────────────

#[tokio::test]
async fn daemon_stale_pid_cleaned() {
    let dir = TempDir::new().unwrap();
    // Write a PID file with a definitely-dead PID (PID 1 or a very high number).
    // On Windows, PID 99999999 is almost certainly not running.
    std::fs::write(dir.path().join("daemon.pid"), "99999999").unwrap();

    // Start should succeed, cleaning the stale PID.
    let daemon = Daemon::start(test_config(&dir), false).await.unwrap();
    let content = std::fs::read_to_string(dir.path().join("daemon.pid")).unwrap();
    assert_eq!(content.trim().parse::<u32>().unwrap(), std::process::id());

    daemon.stop().await.unwrap();
}

// ── daemon_stop_removes_pid ────────────────────────────────────

#[tokio::test]
async fn daemon_stop_removes_pid() {
    let dir = TempDir::new().unwrap();
    let daemon = Daemon::start(test_config(&dir), false).await.unwrap();
    assert!(dir.path().join("daemon.pid").exists());

    daemon.stop().await.unwrap();
    assert!(!dir.path().join("daemon.pid").exists());
}

// ── daemon_stop_saves_checkpoint ───────────────────────────────

#[tokio::test]
async fn daemon_stop_saves_checkpoint() {
    let dir = TempDir::new().unwrap();
    let daemon = Daemon::start(test_config(&dir), false).await.unwrap();

    // Write a file to the VFS.
    {
        let mut vfs = daemon.vfs.lock().await;
        vfs.write(Path::new("/hello.txt"), b"world").unwrap();
    }

    daemon.stop().await.unwrap();

    // Checkpoint should exist.
    assert!(dir.path().join("checkpoint.bin").exists());
}

// ── daemon_start_from_checkpoint ───────────────────────────────

#[tokio::test]
async fn daemon_start_from_checkpoint() {
    let dir = TempDir::new().unwrap();

    // First daemon: create data, stop (saves checkpoint).
    {
        let daemon = Daemon::start(test_config(&dir), false).await.unwrap();
        {
            let mut vfs = daemon.vfs.lock().await;
            vfs.mkdir_p(Path::new("/src")).unwrap();
            vfs.write(Path::new("/src/main.rs"), b"fn main() {}").unwrap();
        }
        daemon.stop().await.unwrap();
    }

    // Second daemon: start from checkpoint, verify data.
    {
        let daemon = Daemon::start(test_config(&dir), true).await.unwrap();
        let vfs = daemon.vfs.lock().await;
        assert_eq!(vfs.read(Path::new("/src/main.rs")).unwrap(), b"fn main() {}");
        drop(vfs);
        daemon.stop().await.unwrap();
    }
}

// ── checkpoint_atomic_write ────────────────────────────────────

#[tokio::test]
async fn checkpoint_atomic_write() {
    let dir = TempDir::new().unwrap();
    let daemon = Daemon::start(test_config(&dir), false).await.unwrap();

    daemon.save_checkpoint().await.unwrap();

    // checkpoint.bin should exist, checkpoint.tmp should NOT exist.
    assert!(dir.path().join("checkpoint.bin").exists());
    assert!(!dir.path().join("checkpoint.tmp").exists());

    daemon.stop().await.unwrap();
}

// ── IPC: status returns JSON ───────────────────────────────────

#[tokio::test]
async fn ipc_status_returns_json() {
    let server = IpcServer::bind().await.unwrap();
    let port = server.port();

    // Spawn a handler task.
    let handle = tokio::spawn(async move {
        let mut conn = server.accept().await.unwrap();
        let req = conn.read_request().await.unwrap().unwrap();
        assert_eq!(req.method, "status");

        let resp = IpcResponse::ok(
            req.id,
            serde_json::json!({"running": true, "tasks": 0, "repos": []}),
        );
        conn.write_response(&resp).await.unwrap();
    });

    // Client side.
    let mut client = IpcClient::connect(port).await.unwrap();
    let resp = client
        .request("status", serde_json::json!({}))
        .await
        .unwrap();

    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["running"], true);
    assert_eq!(result["tasks"], 0);

    handle.await.unwrap();
}

// ── IPC: shutdown response ─────────────────────────────────────

#[tokio::test]
async fn ipc_shutdown_exits_cleanly() {
    let server = IpcServer::bind().await.unwrap();
    let port = server.port();

    let handle = tokio::spawn(async move {
        let mut conn = server.accept().await.unwrap();
        let req = conn.read_request().await.unwrap().unwrap();
        assert_eq!(req.method, "shutdown");

        let resp = IpcResponse::ok(
            req.id,
            serde_json::json!({"checkpoint_saved": true}),
        );
        conn.write_response(&resp).await.unwrap();
    });

    let mut client = IpcClient::connect(port).await.unwrap();
    let resp = client
        .request("shutdown", serde_json::json!({}))
        .await
        .unwrap();

    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["checkpoint_saved"], true);

    handle.await.unwrap();
}

// ── IPC: concurrent connections ────────────────────────────────

#[tokio::test]
async fn ipc_concurrent_connections() {
    let server = IpcServer::bind().await.unwrap();
    let port = server.port();

    // Server handles two connections.
    let server_handle = tokio::spawn(async move {
        for _ in 0..2 {
            let mut conn = server.accept().await.unwrap();
            tokio::spawn(async move {
                let req = conn.read_request().await.unwrap().unwrap();
                let resp = IpcResponse::ok(req.id, serde_json::json!({"ok": true}));
                conn.write_response(&resp).await.unwrap();
            });
        }
    });

    // Two clients connect simultaneously.
    let c1 = tokio::spawn(async move {
        let mut client = IpcClient::connect(port).await.unwrap();
        let resp = client.request("ping", serde_json::json!({})).await.unwrap();
        assert!(resp.error.is_none());
    });
    let c2 = tokio::spawn(async move {
        let mut client = IpcClient::connect(port).await.unwrap();
        let resp = client.request("ping", serde_json::json!({})).await.unwrap();
        assert!(resp.error.is_none());
    });

    c1.await.unwrap();
    c2.await.unwrap();
    server_handle.await.unwrap();
}

// ── IPC: error response ────────────────────────────────────────

#[tokio::test]
async fn ipc_error_response() {
    let server = IpcServer::bind().await.unwrap();
    let port = server.port();

    let handle = tokio::spawn(async move {
        let mut conn = server.accept().await.unwrap();
        let req = conn.read_request().await.unwrap().unwrap();
        let resp = IpcResponse::err(req.id, -32601, "method not found");
        conn.write_response(&resp).await.unwrap();
    });

    let mut client = IpcClient::connect(port).await.unwrap();
    let resp = client
        .request("nonexistent", serde_json::json!({}))
        .await
        .unwrap();

    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);

    handle.await.unwrap();
}

// ── ipc_port_file_round_trip ───────────────────────────────────

#[tokio::test]
async fn ipc_port_file_round_trip() {
    let dir = TempDir::new().unwrap();
    let server = IpcServer::bind().await.unwrap();
    let expected_port = server.port();

    server.write_port_file(dir.path()).unwrap();

    let read_port = ipc::read_port(dir.path()).unwrap().unwrap();
    assert_eq!(read_port, expected_port);
}

// ── daemon_no_checkpoint_start_fresh ───────────────────────────

#[tokio::test]
async fn daemon_no_checkpoint_start_fresh() {
    let dir = TempDir::new().unwrap();

    // Create a leftover checkpoint.
    std::fs::write(dir.path().join("checkpoint.bin"), b"garbage").unwrap();

    // Start WITHOUT checkpoint → fresh VFS, ignores the checkpoint file.
    let daemon = Daemon::start(test_config(&dir), false).await.unwrap();
    let vfs = daemon.vfs.lock().await;
    // Fresh VFS should have only the root directory.
    assert!(vfs.list(Path::new("/")).unwrap().is_empty());
    drop(vfs);
    daemon.stop().await.unwrap();
}

// ── daemon_stop_no_checkpoint_config ───────────────────────────

#[tokio::test]
async fn daemon_stop_no_checkpoint_when_disabled() {
    let dir = TempDir::new().unwrap();
    let mut cfg = test_config(&dir);
    cfg.checkpoint_on_stop = false;

    let daemon = Daemon::start(cfg, false).await.unwrap();
    {
        let mut vfs = daemon.vfs.lock().await;
        vfs.write(Path::new("/data.txt"), b"test").unwrap();
    }
    daemon.stop().await.unwrap();

    // Checkpoint should NOT exist.
    assert!(!dir.path().join("checkpoint.bin").exists());
}

// ── pid_read_nonexistent ───────────────────────────────────────

#[test]
fn pid_read_nonexistent() {
    let dir = TempDir::new().unwrap();
    let pid = devdev_daemon::pid::read_pid(dir.path()).unwrap();
    assert!(pid.is_none());
}

