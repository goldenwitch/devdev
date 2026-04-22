use std::path::Path;

use devdev_vfs::{MemFs, VfsError};

// ── Round-trip: empty VFS ──────────────────────────────────────

#[test]
fn checkpoint_roundtrip_empty_vfs() {
    let fs = MemFs::new();
    let blob = fs.serialize();
    let restored = MemFs::deserialize(&blob).unwrap();

    // Root dir exists, cwd is /
    assert_eq!(restored.getcwd(), Path::new("/"));
    assert!(restored.exists(Path::new("/")));
    // No files
    let entries = restored.list(Path::new("/")).unwrap();
    assert!(entries.is_empty());
    // Limits preserved
    assert_eq!(restored.usage().bytes_limit, fs.usage().bytes_limit);
    assert_eq!(restored.usage().bytes_used, 0);
}

// ── Round-trip: files and directories ──────────────────────────

#[test]
fn checkpoint_roundtrip_files_and_dirs() {
    let mut fs = MemFs::new();
    fs.mkdir_p(Path::new("/a/b/c")).unwrap();
    fs.write(Path::new("/a/file1.txt"), b"hello").unwrap();
    fs.write(Path::new("/a/b/file2.rs"), b"fn main() {}").unwrap();
    fs.write(Path::new("/a/b/c/data.bin"), &[0u8, 1, 2, 255]).unwrap();
    fs.chmod(Path::new("/a/b/file2.rs"), 0o755).unwrap();

    let blob = fs.serialize();
    let restored = MemFs::deserialize(&blob).unwrap();

    assert_eq!(
        restored.read(Path::new("/a/file1.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        restored.read(Path::new("/a/b/file2.rs")).unwrap(),
        b"fn main() {}"
    );
    assert_eq!(
        restored.read(Path::new("/a/b/c/data.bin")).unwrap(),
        &[0u8, 1, 2, 255]
    );

    // Permissions preserved
    let stat = restored.stat(Path::new("/a/b/file2.rs")).unwrap();
    assert_eq!(stat.permissions, 0o755);

    // Directory structure
    let entries = restored.list(Path::new("/a/b")).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"c"));
    assert!(names.contains(&"file2.rs"));

    // bytes_used recalculated
    assert_eq!(restored.usage().bytes_used, fs.usage().bytes_used);
}

// ── Round-trip: symlinks ───────────────────────────────────────

#[test]
fn checkpoint_roundtrip_symlinks() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/target.txt"), b"data").unwrap();
    fs.symlink(Path::new("/target.txt"), Path::new("/link1")).unwrap();
    fs.symlink(Path::new("/link1"), Path::new("/link2")).unwrap();

    let blob = fs.serialize();
    let restored = MemFs::deserialize(&blob).unwrap();

    assert_eq!(
        restored.readlink(Path::new("/link1")).unwrap(),
        Path::new("/target.txt")
    );
    assert_eq!(
        restored.readlink(Path::new("/link2")).unwrap(),
        Path::new("/link1")
    );
    // Reading through symlink chain works
    assert_eq!(
        restored.read(Path::new("/link2")).unwrap(),
        b"data"
    );
}

// ── Round-trip: binary content (null bytes, high bytes, empty) ─

#[test]
fn checkpoint_roundtrip_binary_content() {
    let mut fs = MemFs::new();

    // Null bytes
    fs.write(Path::new("/nulls.bin"), &[0u8; 256]).unwrap();
    // All high bytes
    let high: Vec<u8> = (0..=255).collect();
    fs.write(Path::new("/all_bytes.bin"), &high).unwrap();
    // Empty file
    fs.write(Path::new("/empty"), b"").unwrap();

    let blob = fs.serialize();
    let restored = MemFs::deserialize(&blob).unwrap();

    assert_eq!(restored.read(Path::new("/nulls.bin")).unwrap(), &[0u8; 256]);
    assert_eq!(restored.read(Path::new("/all_bytes.bin")).unwrap(), high);
    assert_eq!(restored.read(Path::new("/empty")).unwrap(), b"");
}

// ── Round-trip: cwd preservation ───────────────────────────────

#[test]
fn checkpoint_roundtrip_cwd() {
    let mut fs = MemFs::new();
    fs.mkdir_p(Path::new("/some/dir")).unwrap();
    fs.chdir(Path::new("/some/dir")).unwrap();

    let blob = fs.serialize();
    let restored = MemFs::deserialize(&blob).unwrap();

    assert_eq!(restored.getcwd(), Path::new("/some/dir"));
}

// ── Round-trip: custom bytes_limit ─────────────────────────────

#[test]
fn checkpoint_roundtrip_bytes_limit() {
    let fs = MemFs::with_limit(42_000_000);

    let blob = fs.serialize();
    let restored = MemFs::deserialize(&blob).unwrap();

    assert_eq!(restored.usage().bytes_limit, 42_000_000);
}

// ── bytes_used is recalculated, not trusted ────────────────────

#[test]
fn checkpoint_roundtrip_bytes_used_recalculated() {
    let mut fs = MemFs::new();
    fs.write(Path::new("/a.txt"), b"hello").unwrap(); // 5 bytes
    fs.write(Path::new("/b.txt"), b"world!").unwrap(); // 6 bytes

    let blob = fs.serialize();
    let restored = MemFs::deserialize(&blob).unwrap();

    // bytes_used should be 11, recalculated from actual file contents
    assert_eq!(restored.usage().bytes_used, 11);
}

// ── Deserialize bad magic fails ────────────────────────────────

#[test]
fn deserialize_bad_magic_fails() {
    let result = MemFs::deserialize(b"GARBAGE_DATA_HERE");
    let err = result.err().expect("should fail on bad magic");
    match err {
        VfsError::InvalidCheckpoint(msg) => assert!(msg.contains("magic")),
        e => panic!("expected InvalidCheckpoint, got: {e}"),
    }
}

// ── Deserialize wrong version fails ────────────────────────────

#[test]
fn deserialize_wrong_version_fails() {
    let mut data = Vec::new();
    data.extend_from_slice(b"DDVFS\x00"); // correct magic
    data.push(255); // bad version
    data.extend_from_slice(b"some body data");

    let err = MemFs::deserialize(&data).err().expect("should fail on wrong version");
    match err {
        VfsError::InvalidCheckpoint(msg) => assert!(msg.contains("version")),
        e => panic!("expected InvalidCheckpoint, got: {e}"),
    }
}

// ── Deserialize truncated data fails ───────────────────────────

#[test]
fn deserialize_truncated_data_fails() {
    // Valid header, but the body is truncated garbage
    let mut data = Vec::new();
    data.extend_from_slice(b"DDVFS\x00"); // magic
    data.push(1); // version
    data.extend_from_slice(b"tru"); // truncated body

    let err = MemFs::deserialize(&data).err().expect("should fail on truncated data");
    match err {
        VfsError::InvalidCheckpoint(msg) => assert!(msg.contains("corrupt")),
        e => panic!("expected InvalidCheckpoint, got: {e}"),
    }
}

// ── Deserialize too-short data fails ───────────────────────────

#[test]
fn deserialize_too_short_fails() {
    let err = MemFs::deserialize(b"DD").err().expect("should fail on short data");
    match err {
        VfsError::InvalidCheckpoint(msg) => assert!(msg.contains("short")),
        e => panic!("expected InvalidCheckpoint, got: {e}"),
    }
}

// ── Post-restore VFS is fully functional ───────────────────────

#[test]
fn checkpoint_restored_vfs_is_functional() {
    let mut fs = MemFs::new();
    fs.mkdir_p(Path::new("/src")).unwrap();
    fs.write(Path::new("/src/main.rs"), b"fn main() {}").unwrap();

    let blob = fs.serialize();
    let mut restored = MemFs::deserialize(&blob).unwrap();

    // Can write new files
    restored
        .write(Path::new("/src/lib.rs"), b"pub mod foo;")
        .unwrap();
    assert_eq!(
        restored.read(Path::new("/src/lib.rs")).unwrap(),
        b"pub mod foo;"
    );

    // Can create directories
    restored.mkdir(Path::new("/src/foo")).unwrap();
    assert!(restored.exists(Path::new("/src/foo")));

    // Can remove files
    restored.remove(Path::new("/src/main.rs")).unwrap();
    assert!(!restored.exists(Path::new("/src/main.rs")));

    // bytes_used tracks correctly after mutations
    // started with 12 ("fn main() {}"), added 12 ("pub mod foo;"), removed 12
    assert_eq!(restored.usage().bytes_used, 12);
}
