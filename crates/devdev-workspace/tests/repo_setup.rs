//! Repo-setup proof: show that a real git repo can be materialized
//! into the in-memory `Fs`, presented through a live FUSE/WinFSP
//! mount, round-tripped through a snapshot, and read back by the
//! host's own `git` binary running inside the mount.
//!
//! These tests answer a concrete product question: *if the daemon
//! wants to give the agent a repo to work against, does our
//! workspace layer actually support that end-to-end?* The contract
//! proven here:
//!
//!   1. The daemon can drive host `git` against a scratch dir,
//!      capture the resulting `.git/` + worktree bytes, and stuff
//!      them into `Fs` via `write_path` / `mkdir_p`.
//!   2. Once mounted, host code (and by extension any agent subprocess
//!      operating on the mount) sees the repo at its POSIX path with
//!      byte-identical content.
//!   3. Serialising the `Fs` and re-hydrating into a fresh `Workspace`
//!      preserves the repo — the checkpoint story works for repos, not
//!      just arbitrary files.
//!   4. On Linux, running `git log` via `Workspace::exec` inside the
//!      mount reads the materialised repo correctly, proving the
//!      mounted bytes are a legitimate git worktree end-to-end.
//!
//! On Windows these tests mount via WinFSP and are therefore
//! `#[ignore]`d by default (same policy as `tests/winfsp_mount.rs`).
//! Run them explicitly with
//! `cargo test -p devdev-workspace --test repo_setup -- --ignored --test-threads=1`.
//! The `--test-threads=1` is load-bearing on Windows: both tests mount
//! a WinFSP filesystem and the auto-drive-letter probe collides if two
//! mounts race.
//!
//! Portability notes:
//!
//!   - The `git` binary must be on PATH. CI always has one; dev boxes
//!     typically do.
//!   - We invoke git with `-c user.name=... -c user.email=...` on the
//!     command line because the curated env sets `HOME=/home/agent`,
//!     which doesn't resolve to a real gitconfig on either host. This
//!     mirrors what the daemon will need to do when driving `git`
//!     itself.

#![cfg(any(target_os = "linux", target_os = "windows"))]

#[cfg(target_os = "linux")]
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use devdev_workspace::{Fs, Workspace};

const REPO_MOUNT_PREFIX: &[u8] = b"/repos/org/acme";

/// Build a real git repo in a host tempdir with one commit, one
/// tracked file (`README.md`), and one second commit touching a
/// nested path. Returns the tempdir handle (keeps it alive) and the
/// short SHA of the HEAD commit for later assertions.
fn seed_host_git_repo() -> (tempfile::TempDir, String) {
    let td = tempfile::Builder::new()
        .prefix("devdev-repo-seed-")
        .tempdir()
        .expect("tempdir");
    let root = td.path();

    git(root, &["init", "--quiet", "--initial-branch=main"]);
    git(
        root,
        &[
            "-c",
            "user.name=devdev",
            "-c",
            "user.email=devdev@example.com",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "chore: initial",
        ],
    );

    std::fs::write(root.join("README.md"), b"# acme\n\nHello from the seed.\n")
        .expect("write README");
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    std::fs::write(root.join("src").join("main.rs"), b"fn main() {}\n").expect("write main.rs");

    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=devdev",
            "-c",
            "user.email=devdev@example.com",
            "commit",
            "--quiet",
            "-m",
            "feat: add README and src",
        ],
    );

    let sha = git_out(root, &["rev-parse", "--short", "HEAD"]);
    (td, sha.trim().to_string())
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} exited with {status}");
}

fn git_out(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} exited with {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Recursively copy a host directory tree into `fs` at the given
/// POSIX prefix. The prefix and all intermediate directories are
/// created with mode 0o755.
///
/// Symlinks are resolved to their targets — the daemon can keep it
/// simple until a real workflow demands symlink preservation.
fn copy_host_dir_into_fs(fs: &mut Fs, host_root: &Path, fs_prefix: &[u8]) {
    fs.mkdir_p(fs_prefix, 0o755).expect("mkdir prefix");
    walk(fs, host_root, host_root, fs_prefix);
}

fn walk(fs: &mut Fs, host_root: &Path, host_cur: &Path, fs_prefix: &[u8]) {
    for entry in std::fs::read_dir(host_cur).expect("read_dir") {
        let entry = entry.expect("dirent");
        let meta = entry.metadata().expect("metadata");
        let rel = entry
            .path()
            .strip_prefix(host_root)
            .expect("strip_prefix")
            .to_path_buf();
        let fs_path = join_fs_path(fs_prefix, &rel);

        if meta.is_dir() {
            fs.mkdir_p(&fs_path, 0o755).expect("mkdir child");
            walk(fs, host_root, &entry.path(), fs_prefix);
        } else if meta.is_file() {
            let bytes = std::fs::read(entry.path()).expect("read file");
            fs.write_path(&fs_path, &bytes).expect("write_path");
        }
        // symlinks / other: skip (see module comment)
    }
}

fn join_fs_path(prefix: &[u8], rel: &Path) -> Vec<u8> {
    let mut out = prefix.to_vec();
    for comp in rel.components() {
        out.push(b'/');
        out.extend_from_slice(comp.as_os_str().to_string_lossy().as_bytes());
    }
    out
}

/// Convert a POSIX path under the mount point to a host path. Handles
/// `/` vs `\` separator mismatch on Windows.
fn host_path(mount: &Path, posix: &[u8]) -> PathBuf {
    let mut tail = posix;
    while tail.first() == Some(&b'/') {
        tail = &tail[1..];
    }
    let s = String::from_utf8_lossy(tail).into_owned();
    #[cfg(windows)]
    let s = s.replace('/', "\\");
    mount.join(&s)
}

fn workspace_with_seeded_repo() -> (Workspace, String) {
    let (host_repo, head_sha) = seed_host_git_repo();
    let mut ws = Workspace::new();
    {
        let fs = ws.fs();
        let mut g = fs.lock().unwrap();
        g.mkdir_p(b"/home/agent", 0o755).unwrap();
        copy_host_dir_into_fs(&mut g, host_repo.path(), REPO_MOUNT_PREFIX);
    }
    drop(host_repo);
    ws.mount().expect("mount");
    (ws, head_sha)
}

#[test]
#[cfg_attr(target_os = "windows", ignore)]
fn repo_materialises_in_fs_and_reads_through_mount() {
    let (ws, _sha) = workspace_with_seeded_repo();
    let mp = ws.mount_point().expect("mount point").to_path_buf();

    let readme = std::fs::read(host_path(&mp, b"/repos/org/acme/README.md")).expect("read README");
    let readme = String::from_utf8(readme).expect("utf8");
    assert!(
        readme.contains("Hello from the seed."),
        "README content through mount was: {readme:?}"
    );

    let main_rs = std::fs::read(host_path(&mp, b"/repos/org/acme/src/main.rs")).expect("read main");
    assert_eq!(main_rs, b"fn main() {}\n");

    let head_ref = std::fs::read(host_path(&mp, b"/repos/org/acme/.git/HEAD")).expect("read HEAD");
    let head_ref = String::from_utf8_lossy(&head_ref);
    assert!(
        head_ref.starts_with("ref: refs/heads/"),
        ".git/HEAD through mount was: {head_ref:?}"
    );
}

#[test]
#[cfg_attr(target_os = "windows", ignore)]
fn repo_survives_fs_snapshot_roundtrip() {
    let (ws, _sha) = workspace_with_seeded_repo();
    let blob = {
        let fs = ws.fs();
        let g = fs.lock().unwrap();
        g.serialize()
    };
    drop(ws);

    let revived = Fs::deserialize(&blob).expect("deserialize");
    let mut ws2 = Workspace::from_fs(revived);
    ws2.mount().expect("mount revived");
    let mp = ws2.mount_point().expect("mount point").to_path_buf();

    let readme = std::fs::read(host_path(&mp, b"/repos/org/acme/README.md")).expect("read README");
    assert!(
        String::from_utf8_lossy(&readme).contains("Hello from the seed."),
        "README survived snapshot: {readme:?}"
    );
    let head = std::fs::read(host_path(&mp, b"/repos/org/acme/.git/HEAD")).expect("read HEAD");
    assert!(String::from_utf8_lossy(&head).starts_with("ref: refs/heads/"));
}

/// Linux-only: run `git log --oneline` via `Workspace::exec` inside
/// the mount. Proves the materialised bytes actually form a valid
/// git repository from the perspective of a real `git` binary.
/// On Windows this path hits the HOME-isn't-a-POSIX-path containment
/// gap that's documented in `tests/cargo_build.rs`; not attempted.
#[cfg(target_os = "linux")]
#[test]
fn git_log_reads_materialised_repo() {
    let (ws, head_sha) = workspace_with_seeded_repo();
    let mut out = Vec::new();
    let args: &[&OsStr] = &[OsStr::new("log"), OsStr::new("--oneline")];
    let code = ws
        .exec(OsStr::new("git"), args, b"/repos/org/acme", &mut out)
        .expect("exec git log");
    let text = String::from_utf8_lossy(&out);
    assert_eq!(code, 0, "git log exit={code}, output:\n{text}");
    assert!(
        text.contains(&head_sha),
        "expected head sha {head_sha} in git log output:\n{text}"
    );
    assert!(
        text.contains("feat: add README and src"),
        "expected second commit subject in git log output:\n{text}"
    );
}
