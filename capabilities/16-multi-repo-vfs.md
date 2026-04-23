---
id: multi-repo-vfs
title: "Multi-Repo VFS Mounts"
status: done
type: leaf
phase: 2
crate: devdev-workspace  # originally devdev-vfs; consolidated in Phase 3
priority: P0
depends-on: []
effort: M
---

# P2-01 — Multi-Repo VFS Mounts

> **Status note (2026-04-22):** Shipped against the original `MemFs::mount/unmount/mounts` API. Phase 3 deleted `MemFs`; multi-repo visibility is now a property of `devdev-workspace`'s real-FS mount and is exposed differently (one workspace root with multiple bind-style attaches, not multiple in-memory mount points). The product behavior — "agent sees several repos under stable paths" — still holds; the API surface described below is **historical**.

Phase 1 loads one repo at `/`. The daemon manages many repos simultaneously — the agent monitoring three PRs across two repos needs all three accessible in the same VFS. This capability makes `MemFs` support multiple repos mounted under `/repos/<owner>/<name>/`.

## Scope

**In:**
- Mount API: `MemFs::mount(path: &Path, source: &Path) -> VfsResult<()>` — load a host repo into the VFS at the given mount point.
- Convention: repos live under `/repos/<owner>/<name>/` (e.g., `/repos/org/api-server/`). The mount point is just a path — the VFS doesn't enforce the naming convention.
- Isolation: each mounted repo is a normal directory subtree. No overlay semantics, no union mounts. The agent can `cd /repos/org/api-server && ls`.
- Existing `load_repo` reuse: `mount` calls `load_repo` with a target prefix. All existing loader logic (gitignore, depth limits, memory cap) applies per-mount.
- Unmount: `MemFs::unmount(path: &Path) -> VfsResult<()>` — remove all nodes under the mount point. Frees memory.
- Mount listing: `MemFs::mounts() -> Vec<PathBuf>` — return all active mount points.
- Round-trip through checkpoint: mounts serialize/deserialize as regular VFS subtrees (P2-00 handles the format; this cap just ensures the data is correct).

**Out:**
- Overlay / union mounts (over-engineering for now — spec §3.6 says path prefix).
- Cross-mount symlinks (symlinks within a mount can point anywhere in the VFS, but we don't create cross-mount symlinks automatically).
- Lazy loading (repos are loaded eagerly on mount).

## Preconditions

- `MemFs` and `load_repo` exist (caps 00, 01 — done).
- `load_repo` currently assumes root `/`. Needs a `prefix` parameter or a post-load tree graft.

## Interface

```rust
impl MemFs {
    /// Load a host repo into the VFS under `mount_point`.
    /// Creates `mount_point` and all parent dirs if they don't exist.
    /// Fails if `mount_point` already contains files (must unmount first).
    pub fn mount(&mut self, mount_point: &Path, host_repo: &Path) -> VfsResult<()>;

    /// Remove all nodes under `mount_point`, including the mount dir itself.
    /// Reclaims memory. No-op if mount_point doesn't exist.
    pub fn unmount(&mut self, mount_point: &Path) -> VfsResult<()>;

    /// List all active mount points (directories that were created via mount()).
    pub fn mounts(&self) -> Vec<PathBuf>;
}
```

### Changes to `load_repo`

```rust
/// Extended: load into a target prefix within the VFS instead of always `/`.
pub fn load_repo(
    vfs: &mut MemFs,
    host_path: &Path,
    prefix: &Path,       // NEW — was implicitly `/`
    options: &LoadOptions,
) -> Result<LoadProgress, LoadError>;
```

## Implementation Notes

- **Mount tracking:** Store mount points in a `HashSet<PathBuf>` on `MemFs`. `mount()` inserts, `unmount()` removes. `mounts()` returns sorted vec.
- **Prefix parameter:** The simplest approach is adding a `prefix: &Path` to `load_repo` and prepending it to every VFS path during loading. All existing callers pass `/` (backward compatible via default).
- **Unmount:** Walk the BTreeMap with `range(mount_point..)` and remove all entries whose path starts with `mount_point`. Update `bytes_used`.
- **Conflict detection:** `mount()` checks if any files exist under `mount_point` before loading. If files exist, return `VfsError::AlreadyExists`. This prevents accidental overwrites.
- **cwd behavior:** If the user's cwd is inside a mount that gets unmounted, reset cwd to `/`. This is a safety measure — don't leave cwd dangling.

## Files

```
crates/devdev-vfs/src/memfs.rs      — mount(), unmount(), mounts() methods, mount tracking HashSet
crates/devdev-vfs/src/loader.rs     — add prefix parameter to load_repo
crates/devdev-vfs/src/lib.rs        — re-export new methods
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-01-1 | §3.6 | Multiple repos in same VFS under different mount points |
| SR-01-2 | §3.6 | Mount paths: `/repos/<owner>/<name>/` |
| SR-01-3 | §4 (Multi-repo VFS row) | Isolation: agent in repo A can't accidentally read repo B (unless asked) |
| SR-01-4 | §4 (Multi-repo VFS row) | Mount points are clean, path resolution respects boundaries |

## Acceptance Tests

- [ ] `mount_single_repo` — mount a test repo at `/repos/test/foo/`, verify files accessible at `/repos/test/foo/src/main.rs`
- [ ] `mount_two_repos_isolated` — mount repo A at `/repos/org/a/`, repo B at `/repos/org/b/`, verify `ls /repos/org/a/` only shows A's files
- [ ] `mount_creates_parent_dirs` — mount at `/repos/deep/nested/repo/`, verify `/repos/`, `/repos/deep/`, etc. exist as directories
- [ ] `mount_over_existing_files_fails` — write a file at `/repos/org/x/foo.txt`, then `mount("/repos/org/x/", ...)` → `VfsError::AlreadyExists`
- [ ] `unmount_removes_all_files` — mount, verify files, unmount, verify all files gone, `bytes_used` decreased
- [ ] `unmount_resets_cwd_if_inside` — `chdir("/repos/org/x/src/")`, unmount `/repos/org/x/`, verify `getcwd()` returns `/`
- [ ] `unmount_nonexistent_is_noop` — unmount a path that was never mounted → no error
- [ ] `mounts_returns_sorted_list` — mount three repos, verify `mounts()` returns them in sorted order
- [ ] `mount_roundtrip_through_checkpoint` — mount two repos, serialize VFS (P2-00), deserialize, verify both repos present and correct
- [ ] `load_repo_prefix_backward_compatible` — existing `load_repo(..., "/", ...)` still works

## Spec Compliance Checklist

- [ ] SR-01-1: multiple repos loadable
- [ ] SR-01-2: convention path works
- [ ] SR-01-3: isolation verified
- [ ] SR-01-4: path resolution correct
- [ ] All acceptance tests passing
