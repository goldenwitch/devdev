---
id: vfs-serialization
title: "VFS Serialization for Checkpoint"
status: superseded
type: leaf
phase: 2
crate: devdev-workspace  # originally devdev-vfs; consolidated in Phase 3
priority: P0
depends-on: []
effort: M
---

# P2-00 — VFS Serialization for Checkpoint

> **Status note (2026-04-22):** Shipped against the original `MemFs` (BTreeMap-backed in-memory filesystem). Phase 3 deleted `MemFs` and replaced the in-process VFS with a real OS mount (FUSE on Linux, WinFSP on Windows) via `devdev-workspace`. The bincode blob format described below is therefore **historical**. The semantic intent — "persist enough state across `devdev down`/`devdev up --checkpoint` that the daemon resumes cleanly" — is unsatisfied in the new world and will be re-expressed as a Phase 5 capability (likely scoped to mount configs + task state + shell scrollback, not raw FS bytes since the host now owns those).
>
> **Additional finding (2026-04-22, post-P2-06 PoC):** The Copilot CLI advertises a `loadSession` capability and a `sessionCapabilities.list` method during `initialize`. That means Copilot-side session state (conversation history, tool-call context) *can* potentially survive a daemon restart — we don't have to re-seed from `SessionContext` on every `up`. The Phase 5 checkpoint redesign should investigate whether `loadSession(sessionId)` does what it sounds like and whether the sessionId is stable across CLI process restarts. If yes, the checkpoint can store `(task_id → sessionId)` and skip context replay. If no, the current replay-from-context approach in [cap 21](21-session-router.md) stays correct.

The daemon needs to persist the virtual filesystem across restarts. This capability adds `serialize` and `deserialize` to `MemFs`, producing a self-contained binary blob that can be written to disk on `devdev down` and loaded on `devdev up --checkpoint`.

## Scope

**In:**
- `MemFs::serialize(&self) -> Vec<u8>` — walk the BTreeMap, write every node (files, dirs, symlinks, metadata) into a binary format.
- `MemFs::deserialize(data: &[u8]) -> VfsResult<MemFs>` — reconstruct a fully functional `MemFs` from the blob.
- Round-trip correctness: serialize → deserialize produces a bitwise-identical tree. File contents, permissions, `cwd`, `bytes_used`, `bytes_limit`, symlink targets, and directory structure all survive.
- Format versioning: a magic header + version byte so we can evolve the format without corrupting old checkpoints.
- Performance target: serialize/deserialize a 500 MB VFS in under 5 seconds on typical hardware.

**Out:**
- Shell session state (that's in the checkpoint manager, P2-02).
- Task state serialization (that's P2-04).
- Compression (not v1 — add if checkpoints are too large).
- Streaming serialization (full snapshot, not incremental journal).

## Preconditions

- `MemFs` exists with its `BTreeMap<PathBuf, Node>` internals (cap 00, done).
- `Node` enum has `File`, `Directory`, `Symlink` variants with `content`, `mode`, `modified`, `target` fields (done).
- `serde` is already a workspace dependency.

## PoC Requirement (Spec Rule 2)

Before committing to bincode:

1. Build a throwaway test that serializes a 500 MB `MemFs` with bincode.
2. Measure: serialization time, deserialization time, blob size.
3. If bincode can't do it in <5s, try msgpack or raw manual serialization.
4. Record result here as **Validated** or **Failed — using alternative**.

**PoC Result:** _Not yet run._

## Interface

```rust
use serde::{Serialize, Deserialize};

impl MemFs {
    /// Serialize the entire VFS tree to a binary blob.
    /// Format: magic bytes + version + bincode-encoded VfsSnapshot.
    pub fn serialize(&self) -> Vec<u8>;

    /// Deserialize a VFS tree from a binary blob.
    /// Returns `VfsError::InvalidCheckpoint` if magic/version mismatch
    /// or data is corrupt.
    pub fn deserialize(data: &[u8]) -> VfsResult<MemFs>;
}

/// Internal serialization structure — not public API.
#[derive(Serialize, Deserialize)]
struct VfsSnapshot {
    version: u8,
    cwd: PathBuf,
    bytes_limit: u64,
    nodes: Vec<(PathBuf, SerializedNode)>,
}

#[derive(Serialize, Deserialize)]
enum SerializedNode {
    File { content: Vec<u8>, mode: u32, modified_secs: u64 },
    Directory { mode: u32, modified_secs: u64 },
    Symlink { target: PathBuf, modified_secs: u64 },
}
```

## Implementation Notes

- **Derive Serialize/Deserialize on Node?** Tempting, but `SystemTime` serialization is platform-dependent. Convert to `u64` seconds-since-epoch in `SerializedNode` for portability.
- **Magic header:** `b"DDVFS\x00"` (6 bytes) followed by `version: u8`. Version 1 for now.
- **BTreeMap ordering:** The serialized `nodes` vec is naturally sorted (BTreeMap iteration order). On deserialize, insert in order — BTreeMap will maintain it.
- **bytes_used recalculation:** On deserialize, recalculate `bytes_used` from the inserted nodes rather than trusting the stored value. This catches corruption.
- **Error type:** Add `VfsError::InvalidCheckpoint(String)` variant for format/version/corruption errors.

## Files

```
crates/devdev-vfs/src/memfs.rs      — serialize()/deserialize() methods on MemFs
crates/devdev-vfs/src/types.rs      — VfsError::InvalidCheckpoint variant, SerializedNode
crates/devdev-vfs/Cargo.toml        — add serde, bincode dependencies
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-00-1 | §3.1, §3.6 | VFS persistence: `serialize()`/`deserialize()` for checkpoint save/restore |
| SR-00-2 | §4 (VFS serialization row) | Round-trip preserves all VFS state bit-for-bit |
| SR-00-3 | §5 Rule 2 | bincode PoC validated before use |

## Acceptance Tests

- [ ] `checkpoint_roundtrip_empty_vfs` — serialize empty MemFs, deserialize, verify empty tree, default cwd `/`
- [ ] `checkpoint_roundtrip_files_and_dirs` — populate with nested dirs, regular files, various permissions → serialize → deserialize → assert tree equality (walk both, compare node-by-node)
- [ ] `checkpoint_roundtrip_symlinks` — create symlinks, serialize → deserialize, `readlink` returns same targets
- [ ] `checkpoint_roundtrip_binary_content` — write files with null bytes, high bytes, empty content → round-trip preserves content exactly
- [ ] `checkpoint_roundtrip_cwd` — `chdir` to `/some/dir`, serialize → deserialize, `getcwd` returns `/some/dir`
- [ ] `checkpoint_roundtrip_bytes_limit` — set custom limit, serialize → deserialize, new `MemFs` has same limit
- [ ] `checkpoint_roundtrip_bytes_used_recalculated` — tamper with serialized bytes_used, deserialize still works (recalculates)
- [ ] `checkpoint_roundtrip_500mb_under_5s` — create ~500 MB of file data, measure serialize + deserialize time, assert < 5 seconds
- [ ] `deserialize_bad_magic_fails` — garbage bytes → `VfsError::InvalidCheckpoint`
- [ ] `deserialize_wrong_version_fails` — valid magic, version 255 → `VfsError::InvalidCheckpoint`
- [ ] `deserialize_truncated_data_fails` — valid header, truncated body → error (not panic)

## Spec Compliance Checklist

- [ ] SR-00-1: serialize/deserialize exist and work
- [ ] SR-00-2: round-trip tests pass for all node types
- [ ] SR-00-3: PoC result recorded above
- [ ] All acceptance tests passing
