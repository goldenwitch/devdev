# Spec: Virtual Filesystem (VFS)

**Status:** Draft — Updated with research findings (April 2026)
**Depends on:** Nothing — foundational component.

---

## Purpose

Provide a pure in-memory filesystem that serves as the agent's entire view of the world. All file operations — reads, writes, directory listings, metadata queries — are served from memory. No host filesystem is touched during agent execution.

---

## Requirements

### Core Operations
The VFS must support the following POSIX-like operations:
- **Read/Write**: open, read, write, close, truncate
- **Metadata**: stat (size, timestamps, type), chmod (permissions tracking)
- **Directory**: mkdir, rmdir, readdir, getcwd, chdir
- **Path**: rename, unlink, symlink, readlink, realpath
- **Search**: glob pattern matching (e.g., `*.ts`, `src/**/*.rs`)

### Repo Loading
- Accept a local filesystem path (or git remote URL) and recursively load all file contents into memory.
- Preserve directory structure, file permissions, and symlinks.
- Binary files should be loaded as opaque byte buffers.
- `.git` directory contents should be loaded selectively — enough to support virtual git operations (see spec-virtual-git.md), not necessarily the full object store for massive repos.

### Size Management
- **Default cap:** 2 GB of in-memory content.
- **Configurable:** A user-facing flag (`--workspace-limit <bytes>`) overrides the default.
- The system must track current memory usage and reject writes that would exceed the cap, returning a clear error (not a crash or silent truncation).
- Repo loading must check total size before committing and fail fast with a descriptive message if the repo exceeds the limit.

### Lifecycle
- **Creation:** A new VFS instance is created per evaluation (per event/PR/ticket).
- **Destruction:** The entire VFS is dropped when the evaluation completes. No persistence, no cleanup, no temp files on disk.
- **Isolation:** Multiple concurrent evaluations must not share VFS state.

---

## Interface Contract

The VFS exposes a single abstract interface that all consumers (WASM tool execution, virtual git, shell parser) depend on. Conceptually:

```
interface VirtualFilesystem {
  read(path) -> bytes
  write(path, bytes) -> void
  append(path, bytes) -> void
  stat(path) -> { size, type, permissions, modified }
  list(path) -> [entry...]
  mkdir(path) -> void
  remove(path) -> void
  rename(from, to) -> void
  glob(pattern) -> [path...]
  exists(path) -> bool
  symlink(target, link_path) -> void
  readlink(path) -> path
  
  getcwd() -> path
  chdir(path) -> void
  
  usage() -> { bytes_used, bytes_limit }
}
```

All paths are virtual absolute paths rooted at `/`. There is no concept of a "host path" inside the VFS.

---

## WASI Integration

The VFS must be compatible with the **WASI (WebAssembly System Interface) preopens mechanism**. This is how WASM tool modules see the filesystem:
- The VFS is mounted as a "preopened directory" at `/` (or a configured root path).
- WASM modules call standard WASI syscalls (`fd_read`, `fd_write`, `path_open`, `fd_prestat_get`, etc.) which the WASM runtime translates into VFS operations.
- The runtime should provide built-in in-memory filesystem implementations (e.g., `mem_fs`, `overlay_fs`, `mount_fs` traits/interfaces) rather than requiring custom WASI shims. Runtimes with first-class virtual FS support are strongly preferred.

This means the VFS interface must satisfy two consumers:
1. **DevDev's own code** (repo loading, git library, shell builtins) — via the direct interface.
2. **WASM tool modules** (grep, cat, etc.) — via WASI preopens backed by the same VFS instance.

Both consumers must see the same filesystem state. A write by one is immediately visible to the other.

---

## Design Notes

- The VFS is the single source of truth for all file content during an evaluation. There is no "fallthrough" to host disk.
- The VFS should use a tree structure (not a flat map) to support efficient directory listing and glob operations.
- File contents should be stored as byte arrays, not strings, to correctly handle binary files.
- Timestamps can be synthetic (set on write) — they don't need to match the source repo's timestamps unless git operations require it.
- Symlinks should be stored and resolved within the VFS. Symlinks that point outside the VFS root resolve to "not found."

---

## Open Questions

1. **Sparse loading:** For repos exceeding the 2 GB cap, should we support loading only a subset of files (e.g., guided by the Scout's file pointers)? Or is "fail fast" the right behavior?
2. **File watching:** Should the VFS support change notifications (for future use by language servers running inside the sandbox)? Or is polling sufficient?
