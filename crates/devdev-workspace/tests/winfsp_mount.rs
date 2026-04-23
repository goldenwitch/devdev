//! WinFSP mount integration tests (Windows only).
//!
//! Mirror of `tests/fuse_mount.rs` for the Windows path. These tests
//! perform real drive-letter mounts and shell out to host utilities,
//! so they are marked `#[ignore]` by default — run with
//! `cargo test -p devdev-workspace --test winfsp_mount -- --ignored`.
//!
//! Requires WinFSP to be installed (see `build.rs` for the lookup
//! path).

#![cfg(target_os = "windows")]

use devdev_workspace::Workspace;

fn workspace_with_basics() -> Workspace {
    use std::sync::mpsc;
    use std::time::Duration;

    let mut ws = Workspace::new();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.mkdir_p(b"/home/agent", 0o755).unwrap();
        g.mkdir_p(b"/tmp", 0o755).unwrap();
        g.mkdir_p(b"/etc", 0o755).unwrap();
    }
    // Run mount() on a worker thread with a watchdog so a hang
    // produces an error message instead of a test-runner timeout.
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let res = ws.mount();
        let _ = tx.send((ws, res));
    });
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok((ws, res)) => {
            res.expect("mount");
            let _ = handle.join();
            ws
        }
        Err(_) => panic!("mount() hung for >10s — see [winfsp] eprintln trace above"),
    }
}

#[test]
#[ignore]
fn mount_and_ls_directory() {
    let ws = workspace_with_basics();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.write_path(b"/home/agent/hello.txt", b"hi\r\n").unwrap();
    }
    let mp = ws.mount_point().expect("mount point").to_path_buf();
    let entries: Vec<String> = std::fs::read_dir(mp.join("home\\agent"))
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        entries.iter().any(|n| n == "hello.txt"),
        "entries = {entries:?}"
    );
}

#[test]
#[ignore]
fn read_file_via_host_fs() {
    let ws = workspace_with_basics();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.write_path(b"/etc/greeting", b"hello world\r\n").unwrap();
    }
    let mp = ws.mount_point().expect("mount point").to_path_buf();
    let bytes = std::fs::read(mp.join("etc\\greeting")).expect("read_file");
    assert_eq!(bytes, b"hello world\r\n");
}
