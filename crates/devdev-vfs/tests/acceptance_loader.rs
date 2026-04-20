//! Acceptance tests for Cap 01 — VFS Loader.
//!
//! Each test maps to one acceptance criterion from capabilities/01-vfs-loader.md.
//! Tests use tempfile to create host filesystem fixtures.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use devdev_vfs::{LoadError, LoadOptions, LoadProgress, MemFs, load_repo};
use tempfile::TempDir;

/// Create a fixture directory with a realistic small repo structure.
fn make_fixture_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Create directory structure
    std::fs::create_dir_all(root.join("src/sub")).unwrap();
    std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    std::fs::create_dir_all(root.join(".git/objects/pack")).unwrap();

    // Regular files
    std::fs::write(root.join("README.md"), "# Hello").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub mod sub;").unwrap();
    std::fs::write(root.join("src/sub/mod.rs"), "// sub module").unwrap();

    // .git files
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(root.join(".git/refs/heads/main"), "abc123\n").unwrap();
    std::fs::write(
        root.join(".git/objects/pack/pack-dummy.pack"),
        b"PACK\x00\x00\x00\x02",
    )
    .unwrap();

    dir
}

/// AC: Load a small test repo, verify all files readable in VFS.
#[test]
fn load_small_repo_all_readable() {
    let fixture = make_fixture_repo();
    let mut vfs = MemFs::new();
    let opts = LoadOptions::default();

    let bytes = load_repo(fixture.path(), &mut vfs, &opts).unwrap();
    assert!(bytes > 0);

    // All files should be readable
    assert_eq!(
        vfs.read(Path::new("/README.md")).unwrap(),
        b"# Hello"
    );
    assert_eq!(
        vfs.read(Path::new("/src/main.rs")).unwrap(),
        b"fn main() {}"
    );
    assert_eq!(
        vfs.read(Path::new("/src/lib.rs")).unwrap(),
        b"pub mod sub;"
    );
    assert_eq!(
        vfs.read(Path::new("/src/sub/mod.rs")).unwrap(),
        b"// sub module"
    );
}

/// AC: `.git` directory is present and contains pack files, refs, HEAD.
#[test]
fn git_directory_loaded() {
    let fixture = make_fixture_repo();
    let mut vfs = MemFs::new();
    let opts = LoadOptions::default();

    load_repo(fixture.path(), &mut vfs, &opts).unwrap();

    assert!(vfs.exists(Path::new("/.git/HEAD")));
    assert_eq!(
        vfs.read(Path::new("/.git/HEAD")).unwrap(),
        b"ref: refs/heads/main\n"
    );
    assert!(vfs.exists(Path::new("/.git/refs/heads/main")));
    assert!(vfs.exists(Path::new("/.git/objects/pack/pack-dummy.pack")));
}

/// AC: Symlink in repo is stored as VFS symlink.
#[test]
fn symlink_preserved() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    std::fs::write(root.join("target.txt"), "data").unwrap();

    // Create a symlink (platform-specific)
    #[cfg(unix)]
    std::os::unix::fs::symlink("target.txt", root.join("link.txt")).unwrap();
    #[cfg(windows)]
    {
        // On Windows, file symlinks may require elevated privileges.
        // Fall back to junction or skip if unavailable.
        if std::os::windows::fs::symlink_file("target.txt", root.join("link.txt")).is_err() {
            eprintln!("skipping symlink test: insufficient privileges on Windows");
            return;
        }
    }

    let mut vfs = MemFs::new();
    load_repo(root, &mut vfs, &LoadOptions::default()).unwrap();

    // The symlink should exist as a symlink in VFS
    let stat = vfs.lstat(Path::new("/link.txt")).unwrap();
    assert_eq!(stat.file_type, devdev_vfs::FileType::Symlink);
}

/// AC: Load a repo exceeding VFS limit → ExceedsLimit error before any files loaded.
#[test]
fn exceeds_limit_fails_fast() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("big.bin"), vec![0u8; 500]).unwrap();
    std::fs::write(dir.path().join("small.txt"), "hi").unwrap();

    let mut vfs = MemFs::with_limit(100); // Way too small
    let result = load_repo(dir.path(), &mut vfs, &LoadOptions::default());

    assert!(matches!(result, Err(LoadError::ExceedsLimit { .. })));
    // VFS should be empty — nothing was loaded
    assert_eq!(vfs.usage().bytes_used, 0);
}

/// AC: Load a repo, then vfs.usage() matches sum of all file sizes.
#[test]
fn usage_matches_file_sizes() {
    let fixture = make_fixture_repo();
    let mut vfs = MemFs::new();

    let bytes_loaded = load_repo(fixture.path(), &mut vfs, &LoadOptions::default()).unwrap();
    assert_eq!(vfs.usage().bytes_used, bytes_loaded);

    // Cross-check: manually sum the known file sizes
    let expected: u64 = [
        b"# Hello".len(),
        b"fn main() {}".len(),
        b"pub mod sub;".len(),
        b"// sub module".len(),
        b"ref: refs/heads/main\n".len(),
        b"abc123\n".len(),
        b"PACK\x00\x00\x00\x02".len(),
    ]
    .iter()
    .map(|&x| x as u64)
    .sum();
    assert_eq!(bytes_loaded, expected);
}

/// AC: Progress callback fires during loading.
#[test]
fn progress_callback_fires() {
    let fixture = make_fixture_repo();
    let mut vfs = MemFs::new();

    let call_count = Arc::new(AtomicU64::new(0));
    let counter = call_count.clone();
    let opts = LoadOptions {
        include_git: true,
        progress: Some(Box::new(move |_progress: LoadProgress| {
            counter.fetch_add(1, Ordering::Relaxed);
        })),
    };

    load_repo(fixture.path(), &mut vfs, &opts).unwrap();
    let count = call_count.load(Ordering::Relaxed);
    // Should fire at least once per file (we have 7 files in the fixture)
    assert!(count >= 7, "progress fired {count} times, expected >= 7");
}

// ── Additional coverage ─────────────────────────────────────────

/// Verify loading a nonexistent path returns HostPathNotFound.
#[test]
fn nonexistent_path() {
    let mut vfs = MemFs::new();
    let result = load_repo(
        Path::new("/this/does/not/exist/anywhere"),
        &mut vfs,
        &LoadOptions::default(),
    );
    assert!(matches!(result, Err(LoadError::HostPathNotFound(_))));
}

/// Verify loading a file (not a directory) returns NotADirectory.
#[test]
fn not_a_directory() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("file.txt");
    std::fs::write(&file, "data").unwrap();

    let mut vfs = MemFs::new();
    let result = load_repo(&file, &mut vfs, &LoadOptions::default());
    assert!(matches!(result, Err(LoadError::NotADirectory(_))));
}

/// Verify include_git=false skips the .git directory.
#[test]
fn exclude_git() {
    let fixture = make_fixture_repo();
    let mut vfs = MemFs::new();
    let opts = LoadOptions {
        include_git: false,
        progress: None,
    };

    load_repo(fixture.path(), &mut vfs, &opts).unwrap();
    assert!(!vfs.exists(Path::new("/.git")));
    assert!(!vfs.exists(Path::new("/.git/HEAD")));
    // Non-.git files should still be there
    assert!(vfs.exists(Path::new("/README.md")));
}
/// AC: A host tree deeper than `MAX_DEPTH` returns `LoadError::ExceedsDepth`
/// instead of blowing the stack or recursing forever.
#[test]
fn deep_tree_rejected() {
    use devdev_vfs::MAX_DEPTH;
    let dir = TempDir::new().unwrap();
    // Build MAX_DEPTH + 10 nested dirs.
    let mut path = dir.path().to_path_buf();
    for i in 0..(MAX_DEPTH + 10) {
        path.push(format!("d{i}"));
    }
    std::fs::create_dir_all(&path).unwrap();

    let mut vfs = MemFs::new();
    let opts = LoadOptions::default();
    let err = load_repo(dir.path(), &mut vfs, &opts).expect_err("expected ExceedsDepth");
    match err {
        LoadError::ExceedsDepth { depth, limit } => {
            assert_eq!(limit, MAX_DEPTH);
            assert!(depth > MAX_DEPTH);
        }
        other => panic!("expected ExceedsDepth, got {other:?}"),
    }
}