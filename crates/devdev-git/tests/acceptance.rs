//! Acceptance tests for Cap 05 — Git Object Database (In-Memory).
//!
//! Each test maps to one acceptance criterion from capabilities/05-virtual-git-core.md.
//! Tests create real git repos via git2, load them into VFS, then verify VirtualRepo.

use devdev_git::{GitLoadError, VirtualRepo};
use devdev_vfs::{LoadOptions, MemFs, load_repo};
use tempfile::TempDir;

/// Create a fixture: a real git repo with one commit containing two files.
fn make_git_fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();

    // Create files on disk
    std::fs::write(dir.path().join("hello.txt"), "hello world\n").unwrap();
    std::fs::write(dir.path().join("src.rs"), "fn main() {}\n").unwrap();

    // Stage and commit
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
        .unwrap();

    dir
}

/// Load a host fixture directory into VFS.
fn load_fixture_into_vfs(fixture: &TempDir) -> MemFs {
    let mut vfs = MemFs::new();
    let opts = LoadOptions::default();
    load_repo(fixture.path(), &mut vfs, &opts).unwrap();
    vfs
}

/// AC: Load `.git` from VFS of a test repo → VirtualRepo constructed successfully.
#[test]
fn load_repo_from_vfs() {
    let fixture = make_git_fixture();
    let vfs = load_fixture_into_vfs(&fixture);
    let vrepo = VirtualRepo::from_vfs(&vfs, "/").unwrap();
    // Should have a valid repo reference
    assert!(!vrepo.repo().is_empty().unwrap());
}

/// AC: `head_ref()` returns current branch name.
#[test]
fn head_ref_returns_branch() {
    let fixture = make_git_fixture();
    let vfs = load_fixture_into_vfs(&fixture);
    let vrepo = VirtualRepo::from_vfs(&vfs, "/").unwrap();

    let head = vrepo.head_ref().unwrap();
    // git init defaults to "master" or "main" depending on config
    assert!(
        head == "refs/heads/master" || head == "refs/heads/main",
        "unexpected HEAD ref: {head}"
    );
}

/// AC: `head_commit()` returns a valid commit object.
#[test]
fn head_commit_valid() {
    let fixture = make_git_fixture();
    let vfs = load_fixture_into_vfs(&fixture);
    let vrepo = VirtualRepo::from_vfs(&vfs, "/").unwrap();

    let commit = vrepo.head_commit().unwrap();
    assert_eq!(commit.message().unwrap(), "initial commit");
    assert_eq!(commit.author().name().unwrap(), "Test");
}

/// AC: `repo.revwalk()` from HEAD produces commit history.
#[test]
fn revwalk_produces_history() {
    let fixture = make_git_fixture();

    // Add a second commit to the fixture
    {
        let repo = git2::Repository::open(fixture.path()).unwrap();
        std::fs::write(fixture.path().join("extra.txt"), "more data\n").unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "second commit",
            &tree,
            &[&head],
        )
        .unwrap();
    }

    let vfs = load_fixture_into_vfs(&fixture);
    let vrepo = VirtualRepo::from_vfs(&vfs, "/").unwrap();

    let mut revwalk = vrepo.repo().revwalk().unwrap();
    revwalk.push_head().unwrap();
    let commits: Vec<_> = revwalk.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(commits.len(), 2, "expected 2 commits in history");
}

/// AC: `repo.find_blob()` retrieves file contents for a known path at HEAD.
#[test]
fn find_blob_at_head() {
    let fixture = make_git_fixture();
    let vfs = load_fixture_into_vfs(&fixture);
    let vrepo = VirtualRepo::from_vfs(&vfs, "/").unwrap();

    let commit = vrepo.head_commit().unwrap();
    let tree = commit.tree().unwrap();
    let entry = tree.get_name("hello.txt").unwrap();
    let blob = vrepo.repo().find_blob(entry.id()).unwrap();
    assert_eq!(blob.content(), b"hello world\n");
}

/// AC: Repo with packed refs → branches and tags resolve correctly.
#[test]
fn packed_refs_resolve() {
    let fixture = make_git_fixture();

    // Create a tag and pack refs
    {
        let repo = git2::Repository::open(fixture.path()).unwrap();
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.tag_lightweight("v0.1", head_commit.as_object(), false)
            .unwrap();

        // Force pack refs by writing packed-refs file
        let head_oid = head_commit.id();
        let packed_refs = format!(
            "# pack-refs with: peeled fully-peeled sorted \n{} refs/tags/v0.1\n",
            head_oid
        );
        std::fs::write(
            fixture.path().join(".git/packed-refs"),
            packed_refs,
        )
        .unwrap();

        // Remove the loose tag ref to force lookup via packed-refs
        let loose_tag = fixture.path().join(".git/refs/tags/v0.1");
        if loose_tag.exists() {
            std::fs::remove_file(loose_tag).unwrap();
        }
    }

    let vfs = load_fixture_into_vfs(&fixture);
    let vrepo = VirtualRepo::from_vfs(&vfs, "/").unwrap();

    // Tag should resolve through packed-refs
    let tag_ref = vrepo.repo().find_reference("refs/tags/v0.1").unwrap();
    assert!(tag_ref.target().is_some());
}

/// AC: Repo with no `.git` directory → NoGitDir error.
#[test]
fn no_git_dir_error() {
    let vfs = MemFs::new();
    let result = VirtualRepo::from_vfs(&vfs, "/");
    assert!(matches!(result, Err(GitLoadError::NoGitDir(_))));
}

// ── Additional coverage ─────────────────────────────────────────

/// Verify VirtualRepo can read both files from the tree.
#[test]
fn both_files_in_tree() {
    let fixture = make_git_fixture();
    let vfs = load_fixture_into_vfs(&fixture);
    let vrepo = VirtualRepo::from_vfs(&vfs, "/").unwrap();

    let commit = vrepo.head_commit().unwrap();
    let tree = commit.tree().unwrap();

    let hello = tree.get_name("hello.txt").unwrap();
    let src = tree.get_name("src.rs").unwrap();

    assert_eq!(
        vrepo.repo().find_blob(hello.id()).unwrap().content(),
        b"hello world\n"
    );
    assert_eq!(
        vrepo.repo().find_blob(src.id()).unwrap().content(),
        b"fn main() {}\n"
    );
}
