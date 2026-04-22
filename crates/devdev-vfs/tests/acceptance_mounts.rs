use std::path::Path;

use devdev_vfs::{LoadOptions, MemFs, VfsError, load_repo, load_repo_at};
use tempfile::TempDir;

/// Create a minimal fixture repo on the host.
fn make_fixture(name: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("README.md"), format!("# {name}")).unwrap();
    std::fs::write(root.join("src/main.rs"), format!("// {name} main")).unwrap();
    dir
}

// ── mount_single_repo ──────────────────────────────────────────

#[test]
fn mount_single_repo() {
    let fixture = make_fixture("foo");
    let mut vfs = MemFs::new();
    let opts = LoadOptions {
        include_git: false,
        ..Default::default()
    };
    let host = fixture.path().to_owned();

    vfs.mount(Path::new("/repos/test/foo"), move |vfs, prefix| {
        load_repo_at(&host, vfs, prefix, &opts).map_err(|e| {
            VfsError::PermissionDenied(format!("load error: {e}"))
        })?;
        Ok(())
    })
    .unwrap();

    assert_eq!(
        vfs.read(Path::new("/repos/test/foo/README.md")).unwrap(),
        b"# foo"
    );
    assert_eq!(
        vfs.read(Path::new("/repos/test/foo/src/main.rs")).unwrap(),
        b"// foo main"
    );
}

// ── mount_two_repos_isolated ───────────────────────────────────

#[test]
fn mount_two_repos_isolated() {
    let fix_a = make_fixture("alpha");
    let fix_b = make_fixture("beta");
    let mut vfs = MemFs::new();
    let _opts = LoadOptions {
        include_git: false,
        ..Default::default()
    };

    let host_a = fix_a.path().to_owned();
    vfs.mount(Path::new("/repos/org/a"), {
        let opts = LoadOptions { include_git: false, ..Default::default() };
        move |vfs, prefix| {
            load_repo_at(&host_a, vfs, prefix, &opts)
                .map_err(|e| VfsError::PermissionDenied(format!("{e}")))?;
            Ok(())
        }
    }).unwrap();

    let host_b = fix_b.path().to_owned();
    vfs.mount(Path::new("/repos/org/b"), {
        let opts = LoadOptions { include_git: false, ..Default::default() };
        move |vfs, prefix| {
            load_repo_at(&host_b, vfs, prefix, &opts)
                .map_err(|e| VfsError::PermissionDenied(format!("{e}")))?;
            Ok(())
        }
    }).unwrap();

    // A has alpha content
    assert_eq!(
        vfs.read(Path::new("/repos/org/a/README.md")).unwrap(),
        b"# alpha"
    );
    // B has beta content
    assert_eq!(
        vfs.read(Path::new("/repos/org/b/README.md")).unwrap(),
        b"# beta"
    );
    // A does NOT have beta's files
    let a_entries = vfs.list(Path::new("/repos/org/a")).unwrap();
    let names: Vec<&str> = a_entries.iter().map(|e| e.name.as_str()).collect();
    assert!(!names.contains(&"beta"));
}

// ── mount_creates_parent_dirs ──────────────────────────────────

#[test]
fn mount_creates_parent_dirs() {
    let mut vfs = MemFs::new();

    vfs.mount(Path::new("/repos/deep/nested/repo"), |_vfs, _prefix| Ok(()))
        .unwrap();

    assert!(vfs.exists(Path::new("/repos")));
    assert!(vfs.exists(Path::new("/repos/deep")));
    assert!(vfs.exists(Path::new("/repos/deep/nested")));
    assert!(vfs.exists(Path::new("/repos/deep/nested/repo")));
}

// ── mount_over_existing_files_fails ────────────────────────────

#[test]
fn mount_over_existing_files_fails() {
    let mut vfs = MemFs::new();
    vfs.mkdir_p(Path::new("/repos/org/x")).unwrap();
    vfs.write(Path::new("/repos/org/x/foo.txt"), b"existing").unwrap();

    let result = vfs.mount(Path::new("/repos/org/x"), |_vfs, _prefix| Ok(()));
    assert!(result.is_err());
    match result.err().unwrap() {
        VfsError::AlreadyExists(_) => {}
        e => panic!("expected AlreadyExists, got: {e}"),
    }
}

// ── unmount_removes_all_files ──────────────────────────────────

#[test]
fn unmount_removes_all_files() {
    let fixture = make_fixture("removeme");
    let mut vfs = MemFs::new();
    let host = fixture.path().to_owned();

    vfs.mount(Path::new("/repos/org/r"), move |vfs, prefix| {
        let opts = LoadOptions { include_git: false, ..Default::default() };
        load_repo_at(&host, vfs, prefix, &opts)
            .map_err(|e| VfsError::PermissionDenied(format!("{e}")))?;
        Ok(())
    }).unwrap();

    let used_before = vfs.usage().bytes_used;
    assert!(used_before > 0);

    vfs.unmount(Path::new("/repos/org/r")).unwrap();

    assert!(!vfs.exists(Path::new("/repos/org/r")));
    assert!(!vfs.exists(Path::new("/repos/org/r/README.md")));
    assert_eq!(vfs.usage().bytes_used, 0);
}

// ── unmount_resets_cwd_if_inside ───────────────────────────────

#[test]
fn unmount_resets_cwd_if_inside() {
    let mut vfs = MemFs::new();
    vfs.mount(Path::new("/repos/org/x"), |vfs, prefix| {
        let dir = prefix.join("src");
        vfs.mkdir_p(&dir)?;
        Ok(())
    }).unwrap();

    vfs.chdir(Path::new("/repos/org/x/src")).unwrap();
    assert_eq!(vfs.getcwd(), Path::new("/repos/org/x/src"));

    vfs.unmount(Path::new("/repos/org/x")).unwrap();
    assert_eq!(vfs.getcwd(), Path::new("/"));
}

// ── unmount_nonexistent_is_noop ────────────────────────────────

#[test]
fn unmount_nonexistent_is_noop() {
    let mut vfs = MemFs::new();
    // Should not error
    vfs.unmount(Path::new("/never/mounted")).unwrap();
}

// ── mounts_returns_sorted_list ─────────────────────────────────

#[test]
fn mounts_returns_sorted_list() {
    let mut vfs = MemFs::new();

    vfs.mount(Path::new("/repos/z"), |_, _| Ok(())).unwrap();
    vfs.mount(Path::new("/repos/a"), |_, _| Ok(())).unwrap();
    vfs.mount(Path::new("/repos/m"), |_, _| Ok(())).unwrap();

    let ms = vfs.mounts();
    assert_eq!(ms.len(), 3);
    assert_eq!(ms[0], Path::new("/repos/a"));
    assert_eq!(ms[1], Path::new("/repos/m"));
    assert_eq!(ms[2], Path::new("/repos/z"));
}

// ── mount_roundtrip_through_checkpoint ─────────────────────────

#[test]
fn mount_roundtrip_through_checkpoint() {
    let fix_a = make_fixture("alpha");
    let fix_b = make_fixture("beta");
    let mut vfs = MemFs::new();

    let host_a = fix_a.path().to_owned();
    vfs.mount(Path::new("/repos/org/a"), {
        let opts = LoadOptions { include_git: false, ..Default::default() };
        move |vfs, prefix| {
            load_repo_at(&host_a, vfs, prefix, &opts)
                .map_err(|e| VfsError::PermissionDenied(format!("{e}")))?;
            Ok(())
        }
    }).unwrap();

    let host_b = fix_b.path().to_owned();
    vfs.mount(Path::new("/repos/org/b"), {
        let opts = LoadOptions { include_git: false, ..Default::default() };
        move |vfs, prefix| {
            load_repo_at(&host_b, vfs, prefix, &opts)
                .map_err(|e| VfsError::PermissionDenied(format!("{e}")))?;
            Ok(())
        }
    }).unwrap();

    // Serialize and deserialize
    let blob = vfs.serialize();
    let restored = MemFs::deserialize(&blob).unwrap();

    assert_eq!(
        restored.read(Path::new("/repos/org/a/README.md")).unwrap(),
        b"# alpha"
    );
    assert_eq!(
        restored.read(Path::new("/repos/org/b/README.md")).unwrap(),
        b"# beta"
    );

    // Mount tracking survived
    let ms = restored.mounts();
    assert_eq!(ms.len(), 2);
    assert!(ms.contains(&Path::new("/repos/org/a").to_path_buf()));
    assert!(ms.contains(&Path::new("/repos/org/b").to_path_buf()));
}

// ── load_repo_prefix_backward_compatible ───────────────────────

#[test]
fn load_repo_prefix_backward_compatible() {
    let fixture = make_fixture("compat");
    let mut vfs = MemFs::new();
    let opts = LoadOptions {
        include_git: false,
        ..Default::default()
    };

    // Original load_repo still works (loads at /)
    load_repo(fixture.path(), &mut vfs, &opts).unwrap();

    assert_eq!(
        vfs.read(Path::new("/README.md")).unwrap(),
        b"# compat"
    );
}
