use std::path::Path;

use devdev_vfs::{MemFs, VfsError};

/// Verify basic file I/O: write bytes, read them back, contents are identical.
#[test]
fn write_read_roundtrip() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/hello.txt"), b"hello world").unwrap();
    let content = fs.read(Path::new("/hello.txt")).unwrap();
    assert_eq!(content, b"hello world");
}

/// Verify mkdir_p creates all intermediate directories and each level
/// lists exactly the expected child — no phantom entries, no missing levels.
#[test]
fn mkdir_p_and_list_all_levels() {
    let mut fs = MemFs::new();
    fs.mkdir_p(Path::new("/a/b/c")).unwrap();

    let root_entries = fs.list(Path::new("/")).unwrap();
    assert_eq!(root_entries.len(), 1);
    assert_eq!(root_entries[0].name, "a");

    let a_entries = fs.list(Path::new("/a")).unwrap();
    assert_eq!(a_entries.len(), 1);
    assert_eq!(a_entries[0].name, "b");

    let b_entries = fs.list(Path::new("/a/b")).unwrap();
    assert_eq!(b_entries.len(), 1);
    assert_eq!(b_entries[0].name, "c");

    let c_entries = fs.list(Path::new("/a/b/c")).unwrap();
    assert!(c_entries.is_empty());
}

/// Verify symlink chains: readlink returns the immediate target,
/// realpath resolves through the full chain, and reading through
/// a multi-hop symlink delivers the original file content.
#[test]
fn symlink_readlink_realpath() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/target.txt"), b"data").unwrap();
    fs.symlink(Path::new("/target.txt"), Path::new("/link1")).unwrap();
    fs.symlink(Path::new("/link1"), Path::new("/link2")).unwrap();

    assert_eq!(fs.readlink(Path::new("/link1")).unwrap(), Path::new("/target.txt"));
    assert_eq!(fs.realpath(Path::new("/link2")).unwrap(), Path::new("/target.txt"));

    let content = fs.read(Path::new("/link2")).unwrap();
    assert_eq!(content, b"data");
}

/// Verify that a symlink whose target resolves outside the VFS root
/// (via `..` traversal) produces a NotFound error on read, not a panic
/// or silent success.
#[test]
fn symlink_outside_root_not_found() {
    let mut fs = MemFs::new();
    fs.symlink(Path::new("/../outside"), Path::new("/escape")).unwrap();

    let result = fs.read(Path::new("/escape"));
    assert!(matches!(result, Err(VfsError::NotFound(_))));
    // The symlink node itself should still exist (we only failed to follow it)
    assert!(matches!(
        fs.lstat(Path::new("/escape")).unwrap().file_type,
        devdev_vfs::FileType::Symlink
    ));
}

/// Verify glob `**/*.rs` matches .rs files at all depths, excludes non-.rs
/// files, and returns results in sorted order.
#[test]
fn glob_double_star_rs() {
    let mut fs = MemFs::new();
    fs.mkdir_p(Path::new("/src/sub")).unwrap();
    fs.write(Path::new("/src/main.rs"), b"").unwrap();
    fs.write(Path::new("/src/lib.rs"), b"").unwrap();
    fs.write(Path::new("/src/sub/mod.rs"), b"").unwrap();
    fs.write(Path::new("/README.md"), b"").unwrap();

    let results = fs.glob("**/*.rs").unwrap();
    assert_eq!(
        results,
        vec![
            Path::new("/src/lib.rs").to_path_buf(),
            Path::new("/src/main.rs").to_path_buf(),
            Path::new("/src/sub/mod.rs").to_path_buf(),
        ]
    );
    // Negative check: non-.rs file must not appear
    assert!(!results.iter().any(|p| p.to_string_lossy().contains("README")));
}

/// Verify the memory cap: writes that would exceed the limit return
/// CapacityExceeded, and the failed write has no side effects (file
/// not created, bytes_used unchanged).
#[test]
fn capacity_exceeded() {
    let mut fs = MemFs::with_limit(100);
    fs.write(Path::new("/big.bin"), &[0u8; 90]).unwrap();
    assert_eq!(fs.usage().bytes_used, 90);

    let result = fs.write(Path::new("/overflow.bin"), &[0u8; 20]);
    assert!(matches!(result, Err(VfsError::CapacityExceeded { .. })));
    // Failed write must not create the file or change usage
    assert!(!fs.exists(Path::new("/overflow.bin")));
    assert_eq!(fs.usage().bytes_used, 90);
}

/// Verify that removing a file decrements bytes_used, freeing capacity
/// for subsequent writes that previously would have exceeded the limit.
#[test]
fn remove_frees_capacity() {
    let mut fs = MemFs::with_limit(100);
    fs.write(Path::new("/a.bin"), &[0u8; 60]).unwrap();
    fs.write(Path::new("/b.bin"), &[0u8; 30]).unwrap();
    assert_eq!(fs.usage().bytes_used, 90);

    fs.remove(Path::new("/a.bin")).unwrap();
    assert_eq!(fs.usage().bytes_used, 30);

    // Write that was previously capacity-blocked now succeeds
    fs.write(Path::new("/c.bin"), &[0u8; 60]).unwrap();
    assert_eq!(fs.usage().bytes_used, 90);
}

/// Verify chdir to a nonexistent path returns NotFound and leaves
/// the working directory unchanged.
#[test]
fn chdir_nonexistent_error() {
    let mut fs = MemFs::new();
    let result = fs.chdir(Path::new("/nonexistent"));
    assert!(matches!(result, Err(VfsError::NotFound(_))));
    assert_eq!(fs.getcwd(), Path::new("/"));
}

/// Verify chdir to a valid directory updates getcwd.
#[test]
fn chdir_valid_dir() {
    let mut fs = MemFs::new();
    fs.mkdir(Path::new("/home")).unwrap();
    fs.chdir(Path::new("/home")).unwrap();
    assert_eq!(fs.getcwd(), Path::new("/home"));
}

/// Verify rename: old path is gone, new path has identical content.
#[test]
fn rename_file() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/old.txt"), b"contents").unwrap();
    fs.rename(Path::new("/old.txt"), Path::new("/new.txt")).unwrap();

    assert!(!fs.exists(Path::new("/old.txt")));
    assert_eq!(fs.read(Path::new("/new.txt")).unwrap(), b"contents");
}

/// Verify binary safety: all 256 byte values (including null) survive
/// a write/read round-trip without corruption or truncation.
#[test]
fn binary_roundtrip() {
    let mut fs = MemFs::new();
    let data: Vec<u8> = (0..=255).collect();
    fs.write(Path::new("/binary.bin"), &data).unwrap();
    let readback = fs.read(Path::new("/binary.bin")).unwrap();
    assert_eq!(readback, data);
}

/// Compile-time check: multiple simultaneous immutable borrows of MemFs
/// must be legal. If someone changes read methods to require `&mut self`,
/// this test will fail to compile. No runtime assertion — the value is
/// in the compilation itself.
#[test]
fn concurrent_read_compiles() {
    let fs = MemFs::new();
    let _a = fs.exists(Path::new("/a"));
    let _b = fs.exists(Path::new("/b"));
    let _cwd = fs.getcwd();
    let _usage = fs.usage();
    // Static assertion: MemFs is Send + Sync (required by the spec)
    fn _assert_send_sync<T: Send + Sync>() {}
    _assert_send_sync::<MemFs>();
}

// ── Additional coverage ─────────────────────────────────────────

/// Verify append extends file content (not replaces) and bytes_used
/// reflects the total accumulated size.
#[test]
fn append_to_file() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/log.txt"), b"line1\n").unwrap();
    fs.append(Path::new("/log.txt"), b"line2\n").unwrap();
    assert_eq!(fs.read(Path::new("/log.txt")).unwrap(), b"line1\nline2\n");
    assert_eq!(fs.usage().bytes_used, 12);
}

/// Verify stat returns Directory file_type for a directory node.
#[test]
fn stat_directory_type() {
    let mut fs = MemFs::new();
    fs.mkdir(Path::new("/dir")).unwrap();

    let stat = fs.stat(Path::new("/dir")).unwrap();
    assert_eq!(stat.file_type, devdev_vfs::FileType::Directory);
}

/// Verify stat returns File file_type and correct byte size for a file.
#[test]
fn stat_file_type_and_size() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/file.txt"), b"hello").unwrap();

    let stat = fs.stat(Path::new("/file.txt")).unwrap();
    assert_eq!(stat.file_type, devdev_vfs::FileType::File);
    assert_eq!(stat.size, 5);
}

/// Verify that removing a non-empty directory without the recursive flag
/// returns DirectoryNotEmpty, and the directory and its contents survive.
#[test]
fn remove_nonempty_dir_fails() {
    let mut fs = MemFs::new();
    fs.mkdir(Path::new("/dir")).unwrap();
    fs.write(Path::new("/dir/file.txt"), b"x").unwrap();

    let result = fs.remove(Path::new("/dir"));
    assert!(matches!(result, Err(VfsError::DirectoryNotEmpty(_))));
    // Directory and child must still exist after the failed remove
    assert!(fs.exists(Path::new("/dir")));
    assert!(fs.exists(Path::new("/dir/file.txt")));
}

/// Verify recursive removal deletes the target, all descendants, and
/// frees all associated capacity.
#[test]
fn remove_r_recursive() {
    let mut fs = MemFs::new();
    fs.mkdir_p(Path::new("/a/b/c")).unwrap();
    fs.write(Path::new("/a/b/c/file.txt"), b"data").unwrap();
    fs.write(Path::new("/a/b/other.txt"), b"more").unwrap();

    fs.remove_r(Path::new("/a")).unwrap();
    assert!(!fs.exists(Path::new("/a")));
    assert!(!fs.exists(Path::new("/a/b")));
    assert!(!fs.exists(Path::new("/a/b/c/file.txt")));
    assert_eq!(fs.usage().bytes_used, 0);
}

/// Verify that relative paths are resolved against the current working
/// directory, and the resulting file is accessible via its absolute path.
#[test]
fn relative_path_uses_cwd() {
    let mut fs = MemFs::new();
    fs.mkdir(Path::new("/home")).unwrap();
    fs.chdir(Path::new("/home")).unwrap();
    fs.write(Path::new("file.txt"), b"relative").unwrap();

    // Both relative (via cwd) and absolute must reach the same file
    assert_eq!(fs.read(Path::new("/home/file.txt")).unwrap(), b"relative");
    assert_eq!(fs.read(Path::new("file.txt")).unwrap(), b"relative");
}

/// Verify overwriting a file with smaller content decreases bytes_used
/// and the content is actually replaced (not just the counter).
#[test]
fn overwrite_file_adjusts_capacity() {
    let mut fs = MemFs::with_limit(100);
    fs.write(Path::new("/f.bin"), &[0xAA; 80]).unwrap();
    assert_eq!(fs.usage().bytes_used, 80);

    fs.write(Path::new("/f.bin"), &[0xBB; 20]).unwrap();
    assert_eq!(fs.usage().bytes_used, 20);
    // Verify content was actually replaced, not just the counter
    let content = fs.read(Path::new("/f.bin")).unwrap();
    assert_eq!(content.len(), 20);
    assert!(content.iter().all(|&b| b == 0xBB));
}

/// Verify truncate shortens file content and adjusts bytes_used downward.
#[test]
fn truncate_file() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/f.txt"), b"hello world").unwrap();
    assert_eq!(fs.usage().bytes_used, 11);

    fs.truncate(Path::new("/f.txt"), 5).unwrap();
    assert_eq!(fs.read(Path::new("/f.txt")).unwrap(), b"hello");
    assert_eq!(fs.usage().bytes_used, 5);
}

/// Verify chmod changes the permission bits reported by stat.
#[test]
fn chmod_and_stat_permissions() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/script.sh"), b"#!/bin/sh").unwrap();
    let before = fs.stat(Path::new("/script.sh")).unwrap().permissions;
    fs.chmod(Path::new("/script.sh"), 0o755).unwrap();
    let after = fs.stat(Path::new("/script.sh")).unwrap().permissions;
    // Ensure permission actually changed (not a no-op)
    assert_ne!(before, after);
    assert_eq!(after, 0o755);
}

/// Verify renaming a directory moves the directory and all its descendants.
/// Old paths are gone, new paths hold the original content.
#[test]
fn rename_directory_with_contents() {
    let mut fs = MemFs::new();
    fs.mkdir_p(Path::new("/old/sub")).unwrap();
    fs.write(Path::new("/old/sub/file.txt"), b"data").unwrap();
    fs.rename(Path::new("/old"), Path::new("/new")).unwrap();

    assert!(!fs.exists(Path::new("/old")));
    assert!(!fs.exists(Path::new("/old/sub")));
    assert!(!fs.exists(Path::new("/old/sub/file.txt")));
    assert!(fs.exists(Path::new("/new/sub/file.txt")));
    assert_eq!(fs.read(Path::new("/new/sub/file.txt")).unwrap(), b"data");
}
