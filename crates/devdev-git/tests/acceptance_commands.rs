//! Acceptance tests for capability 06 — Git Subcommand Implementations.
//!
//! Each test maps to one acceptance criterion in
//! `capabilities/06-virtual-git-commands.md`. Fixtures build real repos via
//! `git2::Repository::init` so output can be compared against real `git`
//! where byte-for-byte fidelity is called out.

use devdev_git::{VirtualGit, VirtualGitRepo, VirtualRepo};
use devdev_vfs::{LoadOptions, MemFs, load_repo};
use tempfile::TempDir;

fn sig() -> git2::Signature<'static> {
    // Fixed identity so the default (non-oneline) log format is stable
    // across runs — not comparing against real git here, just asserting
    // shape.
    git2::Signature::new(
        "Test User",
        "test@example.com",
        &git2::Time::new(1_700_000_000, 0),
    )
    .unwrap()
}

/// A fixture repo with two commits on the default branch touching two files.
fn make_two_commit_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // --- commit 1 ---
    std::fs::write(dir.path().join("a.txt"), "alpha\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "beta\n").unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = sig();
    let first = repo
        .commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
        .unwrap();

    // --- commit 2 ---
    std::fs::write(dir.path().join("a.txt"), "alpha\nalpha-2\n").unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let parent = repo.find_commit(first).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "second commit", &tree, &[&parent])
        .unwrap();

    dir
}

fn load(fixture: &TempDir) -> (MemFs, VirtualRepo) {
    let mut vfs = MemFs::new();
    load_repo(fixture.path(), &mut vfs, &LoadOptions::default()).unwrap();
    let repo = VirtualRepo::from_vfs(&vfs, "/").unwrap();
    (vfs, repo)
}

fn run(repo: &VirtualRepo, args: &[&str]) -> devdev_git::GitResult {
    let vg = VirtualGitRepo::new(repo);
    let owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    vg.execute(&owned, "/")
}

fn stdout(r: &devdev_git::GitResult) -> String {
    String::from_utf8(r.stdout.clone()).unwrap()
}

fn stderr(r: &devdev_git::GitResult) -> String {
    String::from_utf8(r.stderr.clone()).unwrap()
}

// ---------- P0: log ----------

/// AC: `git log --oneline -5` emits `<sha7> <subject>` one per commit.
#[test]
fn log_oneline_emits_short_sha_and_subject() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["log", "--oneline", "-5"]);
    assert_eq!(out.exit_code, 0, "stderr: {}", stderr(&out));
    let body = stdout(&out);
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("second commit"), "got: {}", lines[0]);
    assert!(lines[1].ends_with("initial commit"), "got: {}", lines[1]);
    // sha7 prefix + single space before subject
    assert_eq!(lines[0].split_once(' ').unwrap().0.len(), 7);
}

/// AC: `-n <N>` limit respected.
#[test]
fn log_respects_n_limit() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["log", "--oneline", "-n", "1"]);
    assert_eq!(out.exit_code, 0);
    assert_eq!(stdout(&out).lines().count(), 1);
}

/// AC: `--author=<pat>` filters commits by author name substring.
#[test]
fn log_author_filter() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let hit = run(&r, &["log", "--oneline", "--author=Test"]);
    assert_eq!(hit.exit_code, 0);
    assert_eq!(stdout(&hit).lines().count(), 2);

    let miss = run(&r, &["log", "--oneline", "--author=Nobody"]);
    assert_eq!(miss.exit_code, 0);
    assert_eq!(stdout(&miss), "");
}

/// AC: `--format=%H %s` prints custom format.
#[test]
fn log_custom_format() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["log", "--format=%H %s"]);
    assert_eq!(out.exit_code, 0);
    let first = stdout(&out).lines().next().unwrap().to_owned();
    let (sha, subject) = first.split_once(' ').unwrap();
    assert_eq!(sha.len(), 40);
    assert_eq!(subject, "second commit");
}

// ---------- P0: show ----------

/// AC: `git show HEAD` displays commit metadata + unified diff.
#[test]
fn show_head_emits_metadata_and_diff() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["show", "HEAD"]);
    assert_eq!(out.exit_code, 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.starts_with("commit "), "got: {s}");
    assert!(s.contains("Author: Test User <test@example.com>"));
    assert!(s.contains("second commit"));
    assert!(s.contains("+alpha-2"), "diff body missing: {s}");
}

// ---------- P0: diff ----------

/// AC: `git diff HEAD~1..HEAD` produces a valid unified diff.
#[test]
fn diff_commit_range_unified() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["diff", "HEAD~1..HEAD"]);
    assert_eq!(out.exit_code, 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("--- a/a.txt"), "missing --- header: {s}");
    assert!(s.contains("+++ b/a.txt"), "missing +++ header: {s}");
    assert!(s.contains("+alpha-2"), "missing addition line: {s}");
}

/// AC: `git diff --name-only HEAD~1..HEAD` lists changed paths only.
#[test]
fn diff_name_only() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["diff", "--name-only", "HEAD~1..HEAD"]);
    assert_eq!(out.exit_code, 0);
    assert_eq!(stdout(&out).trim(), "a.txt");
}

/// AC: `git diff --stat HEAD~1..HEAD` produces a file change summary.
#[test]
fn diff_stat_summary() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["diff", "--stat", "HEAD~1..HEAD"]);
    assert_eq!(out.exit_code, 0);
    let s = stdout(&out);
    assert!(s.contains("a.txt"), "{s}");
    assert!(s.contains('+'), "{s}");
}

// ---------- P0: status ----------

/// AC: `git status` reports the current branch and a clean work tree.
///
/// `VirtualRepo::from_vfs` materialises both `.git/` and the working-tree
/// files from the VFS, so `git status` sees an up-to-date checkout.
#[test]
fn status_reports_branch_and_worktree() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["status"]);
    assert_eq!(out.exit_code, 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.starts_with("On branch "), "{s}");
    assert!(
        s.contains("nothing to commit"),
        "expected clean working tree, got: {s}"
    );
}

/// AC: `git status` detects a VFS working-tree modification.
///
/// When a VFS file is modified after the index was built, `git status`
/// should report it as modified — proving the working-tree bridge works.
#[test]
fn status_detects_vfs_modification() {
    let fx = make_two_commit_repo();
    let (mut vfs, _old_repo) = load(&fx);
    // Modify a tracked file in the VFS.
    vfs.write(std::path::Path::new("/a.txt"), b"modified content\n")
        .unwrap();
    // Reload the repo so it sees the modified working tree.
    let repo = VirtualRepo::from_vfs(&vfs, "/").unwrap();
    let out = run(&repo, &["status"]);
    assert_eq!(out.exit_code, 0, "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("modified:") && s.contains("a.txt"),
        "expected modified a.txt in status, got: {s}"
    );
}

// ---------- P0: blame ----------

/// AC: `git blame <file>` attributes each line to a commit.
#[test]
fn blame_attributes_each_line() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["blame", "a.txt"]);
    assert_eq!(out.exit_code, 0, "stderr: {}", stderr(&out));
    let body = stdout(&out);
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2, "got: {:?}", lines);
    for line in &lines {
        // sha8 prefix + " (author   N) content"
        assert!(line.len() >= 10);
        assert!(line.contains("Test User"));
    }
}

// ---------- P1: rev-parse / branch / tag / ls-files ----------

/// AC: `git rev-parse HEAD` returns full 40-char SHA.
#[test]
fn rev_parse_head_full_sha() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["rev-parse", "HEAD"]);
    assert_eq!(out.exit_code, 0);
    assert_eq!(stdout(&out).trim().len(), 40);
}

/// AC: `git branch` lists branches with `*` on the current one.
#[test]
fn branch_marks_current() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["branch"]);
    assert_eq!(out.exit_code, 0);
    let s = stdout(&out);
    let starred: Vec<&str> = s.lines().filter(|l| l.starts_with("* ")).collect();
    assert_eq!(starred.len(), 1, "{s}");
}

/// `git tag` on a tag-free repo prints nothing and exits 0.
#[test]
fn tag_empty() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["tag"]);
    assert_eq!(out.exit_code, 0);
    assert_eq!(stdout(&out), "");
}

/// `git ls-files` lists every tracked path, sorted.
#[test]
fn ls_files_lists_tracked() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["ls-files"]);
    assert_eq!(out.exit_code, 0);
    let body = stdout(&out);
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines, vec!["a.txt", "b.txt"]);
}

// ---------- guard: blocked commands ----------

/// AC: `git push` → exit 1 with the exact spec message.
#[test]
fn push_is_blocked() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["push"]);
    assert_eq!(out.exit_code, 1);
    assert_eq!(
        stderr(&out),
        "devdev: git push is not available in the virtual workspace.\n"
    );
    assert!(out.stdout.is_empty());
}

/// Each entry of `BLOCKED` produces the same shape of error.
#[test]
fn all_mutating_commands_blocked() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    for cmd in devdev_git::BLOCKED {
        let out = run(&r, &[cmd]);
        assert_eq!(out.exit_code, 1, "{cmd}");
        assert_eq!(
            stderr(&out),
            format!("devdev: git {cmd} is not available in the virtual workspace.\n")
        );
    }
}

/// Unknown subcommand → stderr + exit 1 (not a panic).
#[test]
fn unknown_subcommand_errors() {
    let fx = make_two_commit_repo();
    let (_vfs, r) = load(&fx);
    let out = run(&r, &["quibble"]);
    assert_eq!(out.exit_code, 1);
    assert!(stderr(&out).contains("not a devdev git command"));
}
