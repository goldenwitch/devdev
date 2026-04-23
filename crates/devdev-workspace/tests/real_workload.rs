//! Real-workload stress test (Linux only).
//!
//! Unlike `cargo_build.rs` (which is blocked on containment because
//! cargo/rustup use absolute `$HOME` paths), this test stays strictly
//! inside the mount via `cwd_in_fs` + relative paths. It pushes the
//! FUSE driver with thousands of small ops: creating many files,
//! walking a directory tree, reading every file back, concatenating
//! results, and running nested shell pipelines.
//!
//! Goal: surface correctness or concurrency bugs in the
//! `Arc<Mutex<Fs>>` backing store and the FUSE translation layer that
//! toy 6-op tests miss.

#![cfg(target_os = "linux")]

use std::ffi::OsStr;

use devdev_workspace::Workspace;

fn mount_with_tree() -> Workspace {
    let mut ws = Workspace::new();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        // Seed a tree: /data/{a..j}/file_{0..9}.txt, each containing
        // a deterministic short string. 100 files total across 10 dirs.
        g.mkdir_p(b"/data", 0o755).unwrap();
        for d in b'a'..=b'j' {
            let dir = format!("/data/{}", d as char);
            g.mkdir_p(dir.as_bytes(), 0o755).unwrap();
            for n in 0..10 {
                let path = format!("{dir}/file_{n}.txt");
                let content = format!("dir={} idx={n}\n", d as char);
                g.write_path(path.as_bytes(), content.as_bytes()).unwrap();
            }
        }
        g.mkdir_p(b"/work", 0o755).unwrap();
    }
    ws.mount().expect("mount");
    ws
}

fn run(ws: &Workspace, script: &str, cwd: &[u8]) -> (i32, String) {
    let mut out = Vec::new();
    let code = ws
        .exec(
            OsStr::new("/bin/sh"),
            &[OsStr::new("-c"), OsStr::new(script)],
            cwd,
            &mut out,
        )
        .expect("exec");
    (code, String::from_utf8_lossy(&out).to_string())
}

#[test]
fn find_and_wc_the_tree() {
    let ws = mount_with_tree();
    // Count all files under the seeded tree. Uses `find` + `wc -l`
    // which drives hundreds of lookups/readdirs through the mount.
    let (code, out) = run(&ws, "find . -type f | wc -l", b"/data");
    assert_eq!(code, 0, "output:\n{out}");
    let n: u32 = out.trim().parse().unwrap_or_else(|_| panic!("bad wc out: {out:?}"));
    assert_eq!(n, 100, "expected 100 files, saw {n}");
}

#[test]
fn cat_every_file_and_total_bytes() {
    let ws = mount_with_tree();
    // Concatenate every file's content and count the bytes. Exercises
    // ~200 read ops plus the full tree walk. Relies only on relative
    // paths and cwd, staying inside the mount.
    let (code, out) = run(
        &ws,
        "find . -type f -print0 | xargs -0 cat | wc -c",
        b"/data",
    );
    assert_eq!(code, 0, "output:\n{out}");
    let bytes: usize = out.trim().parse().unwrap_or_else(|_| panic!("bad out: {out:?}"));
    // Each file is exactly "dir=X idx=N\n". X is 1 byte, N is 1 byte.
    // Total per file: "dir=X idx=N\n" = 12 bytes. 100 files => 1200.
    assert_eq!(bytes, 1200, "expected 1200 bytes, saw {bytes}");
}

#[test]
fn write_many_files_via_shell() {
    let ws = mount_with_tree();
    // Ask the shell to create 200 small files inside the mount.
    // Forces 200 create + 200 write + 200 close roundtrips through
    // the FUSE layer in one process.
    let (code, out) = run(
        &ws,
        "for i in $(seq 1 200); do echo \"line $i\" > \"f_$i.txt\"; done && ls | wc -l",
        b"/work",
    );
    assert_eq!(code, 0, "output:\n{out}");
    let n: u32 = out.trim().parse().unwrap_or_else(|_| panic!("bad out: {out:?}"));
    assert_eq!(n, 200, "expected 200 files in /work, saw {n}");

    // Verify a handful of them land in MemFs with correct content.
    let fs = ws.fs();
    let g = fs.lock().unwrap();
    for i in [1u32, 42, 100, 199, 200] {
        let path = format!("/work/f_{i}.txt");
        let bytes = g.read_path(path.as_bytes()).unwrap_or_else(|e| {
            panic!("read {path}: {e:?}");
        });
        let expected = format!("line {i}\n");
        assert_eq!(
            std::str::from_utf8(&bytes).unwrap(),
            expected,
            "content mismatch at {path}"
        );
    }
}

#[test]
fn grep_pipeline() {
    let ws = mount_with_tree();
    // Grep for a pattern that appears in exactly 10 files (all idx=7).
    let (code, out) = run(&ws, "grep -l 'idx=7' */*.txt | wc -l", b"/data");
    assert_eq!(code, 0, "output:\n{out}");
    let n: u32 = out.trim().parse().unwrap_or_else(|_| panic!("bad out: {out:?}"));
    assert_eq!(n, 10, "expected 10 matches, saw {n}");
}

#[test]
fn deep_mkdir_and_nested_writes() {
    let ws = mount_with_tree();
    // Build a deep nested tree purely through the shell + FUSE.
    let (code, out) = run(
        &ws,
        "mkdir -p a/b/c/d/e/f && echo deep > a/b/c/d/e/f/marker && cat a/b/c/d/e/f/marker",
        b"/work",
    );
    assert_eq!(code, 0, "output:\n{out}");
    assert!(out.contains("deep"), "unexpected output: {out:?}");

    // Confirm via MemFs.
    let fs = ws.fs();
    let g = fs.lock().unwrap();
    let marker = g.read_path(b"/work/a/b/c/d/e/f/marker").expect("read marker");
    assert_eq!(marker, b"deep\n");
}
