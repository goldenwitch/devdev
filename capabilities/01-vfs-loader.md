---
id: vfs-loader
title: "Repo Loading into VFS"
status: done
type: leaf
phase: 2
crate: devdev-vfs
priority: P0
depends-on: [vfs-core]
effort: S
---

# 01 — Repo Loading into VFS

Load a real repository from the host filesystem into the in-memory VFS. This is the only component that reads from the host disk — once loading is complete, the VFS is entirely self-contained.

## Scope

**In:**
- Accept a local filesystem path, recursively load all files into VFS
- Load the `.git` directory fully (pack files, loose objects, refs, HEAD, index)
- Preserve directory structure, file permissions (mode bits), and symlinks
- Pre-check total size before committing; fail fast if exceeds VFS limit
- Progress reporting (file count, bytes loaded) for large repos

**Out:**
- Loading from git remote URLs (future — would need a git clone step)
- Sparse/partial loading (deferred optimization)
- Any transformation of file contents

## Interface

```rust
pub struct LoadOptions {
    pub include_git: bool,          // default: true
    pub follow_gitignore: bool,     // default: false (load everything)
    pub progress: Option<Box<dyn Fn(LoadProgress)>>,
}

pub struct LoadProgress {
    pub files_loaded: u64,
    pub bytes_loaded: u64,
    pub current_path: String,
}

/// Load a host filesystem path into the VFS.
/// Returns the total bytes loaded.
pub fn load_repo(
    host_path: &Path,
    vfs: &mut dyn VirtualFilesystem,
    options: &LoadOptions,
) -> Result<u64, LoadError>;

pub enum LoadError {
    HostPathNotFound(PathBuf),
    NotADirectory(PathBuf),
    ExceedsLimit { total_bytes: u64, limit: u64 },
    IoError(std::io::Error),
    VfsError(VfsError),
}
```

## Implementation Notes

- **Two-pass loading:** First pass: walk the host directory tree and sum total bytes. If total exceeds VFS limit, fail immediately with `ExceedsLimit` (don't load half the repo then error). Second pass: actually read files and write to VFS.
- **Symlinks:** Store as VFS symlinks. If the target is inside the repo, it will resolve within VFS. If outside, it stays as a dangling symlink (matching real behavior — the agent can `readlink` it but can't follow it).
- **Binary files:** Loaded as raw bytes. No encoding detection or conversion.
- **`.git` loading:** Load the entire `.git` directory into VFS. This makes `.git/objects/pack/*.pack`, `.git/refs/*`, `.git/HEAD`, etc. available for `05-virtual-git-core` to parse. Large repos with multi-GB pack files will hit the VFS cap — that's the intended safety valve.
- **Permissions:** Map host file permissions to VFS mode bits. On Windows, approximate (files get 0o644, dirs get 0o755).

## Files

```
crates/devdev-vfs/src/loader.rs    — load_repo function + LoadOptions
```

## Acceptance Criteria

- [ ] Load a small test repo (~100 files), verify all files readable in VFS
- [ ] `.git` directory is present and contains pack files, refs, HEAD
- [ ] Symlink in repo is stored as VFS symlink
- [ ] Load a repo exceeding VFS limit → `ExceedsLimit` error before any files are loaded
- [ ] Load a repo, then `vfs.usage()` matches sum of all file sizes
- [ ] File permissions preserved (at least on Linux/macOS)
- [ ] Progress callback fires during loading
