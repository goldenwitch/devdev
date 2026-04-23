//! FUSE mount integration tests (Linux only).
//!
//! Note: commands execute in the host namespace, so absolute paths
//! like `/home/agent` resolve against the host, not the mount. We
//! therefore drive tests via `cwd_in_fs` + relative paths so every
//! fs op is routed through our FUSE adapter.

#![cfg(target_os = "linux")]

use std::ffi::OsStr;

use devdev_workspace::{Errno, Kind, ROOT_INO, Workspace};

fn workspace_with_basics() -> Workspace {
    let mut ws = Workspace::new();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.mkdir_p(b"/home/agent", 0o755).unwrap();
        g.mkdir_p(b"/tmp", 0o755).unwrap();
        g.mkdir_p(b"/etc", 0o755).unwrap();
    }
    ws.mount().expect("mount");
    ws
}

fn run(ws: &Workspace, cmd: &str, args: &[&str], cwd: &[u8]) -> (i32, String) {
    let mut out = Vec::new();
    let a: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let code = ws.exec(OsStr::new(cmd), &a, cwd, &mut out).expect("exec");
    (code, String::from_utf8_lossy(&out).to_string())
}

#[test]
fn mount_and_ls() {
    let ws = workspace_with_basics();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.write_path(b"/home/agent/hello.txt", b"hi\n").unwrap();
    }
    let (code, out) = run(&ws, "/bin/ls", &["."], b"/home/agent");
    assert_eq!(code, 0, "ls output:\n{out}");
    assert!(out.contains("hello.txt"), "out was:\n{out}");
}

#[test]
fn cat_file() {
    let ws = workspace_with_basics();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.write_path(b"/etc/greeting", b"hello world\n").unwrap();
    }
    let (code, out) = run(&ws, "/bin/cat", &["greeting"], b"/etc");
    assert_eq!(code, 0);
    assert!(out.contains("hello world"), "out was: {out:?}");
}

#[test]
fn echo_writes_through_driver() {
    let ws = workspace_with_basics();
    let (code, out) = run(&ws, "/bin/sh", &["-c", "echo stuff > note"], b"/tmp");
    assert_eq!(code, 0, "sh output:\n{out}");
    let fs = ws.fs();
    let g = fs.lock().unwrap();
    let data = g.read_path(b"/tmp/note").expect("read /tmp/note");
    assert_eq!(data, b"stuff\n");
}

#[test]
fn mkdir_through_driver() {
    let ws = workspace_with_basics();
    let (code, _) = run(&ws, "/bin/sh", &["-c", "mkdir -p work/a/b/c"], b"/");
    assert_eq!(code, 0);
    let fs = ws.fs();
    let g = fs.lock().unwrap();
    let ino = g.resolve(b"/work/a/b/c").expect("resolve");
    let attr = g.getattr(ino).unwrap();
    assert_eq!(attr.kind, Kind::Directory);
}

#[test]
fn rm_through_driver() {
    let ws = workspace_with_basics();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.write_path(b"/tmp/doomed", b"bye").unwrap();
    }
    let (code, out) = run(&ws, "/bin/rm", &["doomed"], b"/tmp");
    assert_eq!(code, 0, "rm output:\n{out}");
    let fs = ws.fs();
    let g = fs.lock().unwrap();
    let tmp_ino = g.resolve(b"/tmp").unwrap();
    assert_eq!(g.lookup(tmp_ino, b"doomed"), Err(Errno::NoEnt));
}

#[test]
fn symlink_roundtrip() {
    let ws = workspace_with_basics();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.write_path(b"/target", b"payload-bytes\n").unwrap();
        g.symlink(ROOT_INO, b"link", b"target").unwrap();
    }
    let (code, out) = run(&ws, "/bin/cat", &["link"], b"/");
    assert_eq!(code, 0);
    assert!(out.contains("payload-bytes"), "out: {out:?}");
}
