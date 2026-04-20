---
id: vfs-core
title: "In-Memory Virtual Filesystem"
status: done
type: leaf
phase: 1
crate: devdev-vfs
priority: P0
depends-on: []
effort: L
---

# 00 — In-Memory Virtual Filesystem

The foundational data structure. Every other capability reads from or writes to this. Nothing in the sandbox touches the host filesystem — this is the single source of truth during an evaluation.

## Scope

**In:**
- Tree-based in-memory filesystem (directories, files, symlinks)
- All POSIX-like operations: read, write, append, stat, list, mkdir, remove, rename, exists
- Symlink storage and resolution (within VFS only)
- Glob expansion (`*`, `?`, `**`, `[abc]`, `[a-z]`)
- Working directory tracking (`getcwd`, `chdir`)
- Memory usage tracking with configurable cap (default 2 GB)
- File contents stored as `Vec<u8>` (binary-safe, not String)

**Out:**
- Loading files from host FS (that's `01-vfs-loader`)
- WASI preopen mounting (that's `03-wasm-engine`)
- Any host disk I/O

## Interface

```rust
pub trait VirtualFilesystem: Send + Sync {
    // File I/O
    fn read(&self, path: &Path) -> Result<Vec<u8>>;
    fn write(&mut self, path: &Path, data: &[u8]) -> Result<()>;
    fn append(&mut self, path: &Path, data: &[u8]) -> Result<()>;
    fn truncate(&mut self, path: &Path, size: u64) -> Result<()>;
    
    // Metadata
    fn stat(&self, path: &Path) -> Result<FileStat>;
    fn exists(&self, path: &Path) -> bool;
    fn chmod(&mut self, path: &Path, mode: u32) -> Result<()>;
    
    // Directories
    fn mkdir(&mut self, path: &Path) -> Result<()>;
    fn mkdir_p(&mut self, path: &Path) -> Result<()>;  // recursive
    fn remove(&mut self, path: &Path) -> Result<()>;
    fn remove_r(&mut self, path: &Path) -> Result<()>;  // recursive
    fn list(&self, path: &Path) -> Result<Vec<DirEntry>>;
    
    // Path operations
    fn rename(&mut self, from: &Path, to: &Path) -> Result<()>;
    fn symlink(&mut self, target: &Path, link: &Path) -> Result<()>;
    fn readlink(&self, path: &Path) -> Result<PathBuf>;
    fn realpath(&self, path: &Path) -> Result<PathBuf>;
    
    // Search
    fn glob(&self, pattern: &str) -> Result<Vec<PathBuf>>;
    
    // Working directory
    fn getcwd(&self) -> &Path;
    fn chdir(&mut self, path: &Path) -> Result<()>;
    
    // Memory management
    fn usage(&self) -> MemoryUsage;
    fn set_limit(&mut self, bytes: u64);
}

pub struct FileStat {
    pub size: u64,
    pub file_type: FileType,  // File, Directory, Symlink
    pub permissions: u32,
    pub modified: SystemTime,
}

pub struct MemoryUsage {
    pub bytes_used: u64,
    pub bytes_limit: u64,
}
```

## Implementation Notes

- **Data structure:** `BTreeMap<PathBuf, Node>` where `Node` is an enum of `Dir { entries }`, `File { content: Vec<u8>, mode: u32 }`, `Symlink { target: PathBuf }`. BTreeMap gives sorted directory listings and efficient prefix scanning for glob.
- **Path resolution:** All paths are virtual absolute paths rooted at `/`. Relative paths are resolved against `cwd`. `..` traversal works. Symlinks that point outside VFS root resolve to "not found."
- **Memory tracking:** Every `write`/`append` call updates a running `bytes_used` counter. Writes that would exceed `bytes_limit` return `Err(VfsError::CapacityExceeded)`. Removes decrement the counter.
- **Glob:** Use the `globset` crate for pattern compilation. Walk the BTreeMap to match. `**` recursion is bounded by VFS depth, not host FS.
- **Timestamps:** Synthetic — set to `SystemTime::now()` on write. No need to match source repo timestamps.
- **Thread safety:** The trait requires `Send + Sync`. Interior mutability via `RwLock` if needed, but the primary use is single-threaded per evaluation.

## Files

```
crates/devdev-vfs/Cargo.toml
crates/devdev-vfs/src/lib.rs        — VirtualFilesystem trait, error types, re-exports
crates/devdev-vfs/src/memfs.rs      — MemFs: the BTreeMap-backed implementation
crates/devdev-vfs/src/types.rs      — FileStat, DirEntry, FileType, MemoryUsage
crates/devdev-vfs/src/glob.rs       — Glob expansion against MemFs
crates/devdev-vfs/src/path.rs       — Path normalization, resolution, symlink following
```

## Acceptance Criteria

- [ ] Create a VFS, write a file, read it back — contents match
- [ ] Nested directory creation with `mkdir_p`, list all levels
- [ ] Write a symlink, `readlink` returns target, `realpath` resolves through chain
- [ ] Symlink pointing outside `/` → `NotFound` error
- [ ] Glob `**/*.rs` against a nested tree returns correct paths in sorted order
- [ ] Write files until approaching 2 GB cap → next write returns `CapacityExceeded`
- [ ] Remove a file → `bytes_used` decreases, new writes succeed
- [ ] `chdir` to nonexistent path → error. `chdir` to valid dir → `getcwd` reflects it
- [ ] `rename` a file, old path gone, new path has same content
- [ ] Binary file round-trip: write arbitrary bytes (including null), read back identical
- [ ] Concurrent read access (multiple `&self` borrows) compiles
