---
id: engine-cleanup
title: "Engine Cleanup (Phase 1 Gaps)"
status: obsolete
type: leaf
phase: 2
crate: devdev-wasm, devdev-git  # both deleted in Phase 3
priority: P1
depends-on: []
effort: M
---

# P2-08 — Engine Cleanup (Phase 1 Gaps)

> **STATUS: OBSOLETE (Phase 3 consolidation, 2026-04-22).** This cap targeted two crates — `devdev-wasm` (the WASM coreutils shim registry) and `devdev-git` (the libgit2-over-MemFs wrapper) — that **no longer exist**. Phase 3 deleted both: the agent now runs the host's real `sed`/`awk`/`git` binaries inside the FUSE/WinFSP mount, so the sed-shim, git-log-flag, and git-diff-path-filter gaps enumerated below were rendered moot rather than fixed. `spirit/spec-copilot-integration.md` reconciliation (§E) still applies but has been tracked informally. No follow-up work is planned under this cap ID.

Fix the known gaps from Phase 1 that cause the agent to hit dead ends. These are independent of the daemon architecture and can be built in parallel with the early Phase 2 capabilities.

## Scope

**In:**

### A. sed shim via sd.wasm

- Compile the `sd` crate to `wasm32-wasip1`. `sd` is a simpler sed alternative that covers the agent's common use cases (find-and-replace in files).
- Add shim entry: `"sed" → "sd"` in `WasmToolRegistry::shims`.
- Argument translation: basic `sed 's/old/new/g' file` → `sd 'old' 'new' file`. Only the most common `s///` form. Log a warning for unsupported sed syntax rather than silently failing.
- Register `sd.wasm` in the embedded tools list.

### B. git log flags

- `--since=<date>` / `--after=<date>` — filter commits by date.
- `--follow` — follow renames for a specific file.
- `-- <path>` — filter log to commits touching specific paths.
- These extend `crates/devdev-git/src/commands/log.rs`.

### C. git diff path filtering

- `git diff <commit> -- <path>` — show diff for specific files only.
- Extends `crates/devdev-git/src/commands/diff.rs`.

### D. git status VFS-aware

- Currently `git status` reflects the on-disk state (temp dir), not VFS mutations.
- Implement: diff the VFS working tree against the git index to produce status output.
- Show files that the agent has modified (via VFS writes) as "modified" even though the on-disk temp dir hasn't changed.
- This is medium effort — requires walking the VFS tree and comparing against libgit2's index.

### E. ACP spec reconciliation

- Rewrite `spirit/spec-copilot-integration.md` Requirements section to match the Resolved Questions section.
- The spec currently says "preToolUse hooks" in Requirements but "client capabilities" in Resolved Questions. The implementation correctly follows Resolved Questions. Fix the spec to match reality.

**Out:**
- awk (deferred — no good wasm target, agent rarely needs it).
- WASM temp-dir optimization (separate, larger effort).
- Git write commands (commit, push — out of scope for read-only sandbox).

## PoC Requirement (Spec Rule 2)

### sd.wasm compilation

1. `cargo build --target wasm32-wasip1 -p sd` — does it compile?
2. Run the resulting `.wasm` through wasmtime with a simple test: `echo "hello" | sd "hello" "world"` → `"world"`.
3. If sd doesn't compile to wasm32-wasip1, evaluate alternatives: `rep` (simpler regex replacer), or a custom WASM tool.

**PoC Result:** _Not yet run._

## Interface Changes

### A. Shim Table (devdev-wasm)

```rust
// In WasmToolRegistry::new()
let mut shims: HashMap<&'static str, &'static str> = HashMap::new();
shims.insert("sed", "sd");
```

### B/C. Git Command Extensions (devdev-git)

```rust
// Extended git log options
pub struct LogOptions {
    pub max_count: Option<usize>,
    pub oneline: bool,
    pub format: Option<String>,
    pub since: Option<String>,    // NEW
    pub follow: bool,             // NEW
    pub paths: Vec<String>,       // NEW — filter to specific paths
}

// Extended git diff options
pub struct DiffOptions {
    pub commit: Option<String>,
    pub stat: bool,
    pub name_only: bool,
    pub paths: Vec<String>,       // NEW — filter to specific paths
}
```

### D. VFS-aware git status (devdev-git)

```rust
impl VirtualGitRepo<'_> {
    /// Compare VFS working tree against git index.
    /// Returns status entries reflecting VFS mutations.
    fn status_vfs_aware(
        &self,
        vfs: &MemFs,
        repo_root: &str,
    ) -> GitResult;
}
```

## Implementation Notes

### A. sd.wasm

- The `sd` crate uses regex. The `regex` crate compiles to wasm32-wasip1 (confirmed in Phase 1 with grep.wasm).
- Argument translation is intentionally minimal: only `sed 's/pattern/replacement/flags'` and `sed -i 's/...'`. Anything else logs "unsupported sed syntax, using sd directly" and passes args through.
- If sd doesn't have a built-in stdin-to-stdout mode, we may need to materialize the target file to temp dir and read it back, same as other WASM tools.

### B. git log --since

- libgit2 revwalk doesn't natively support date filtering. Walk the revwalk, check `commit.time()`, skip commits older than the cutoff.
- `--since` parsing: support ISO 8601 (`2026-04-01`), relative (`2 weeks ago`), and Unix timestamp. Use `chrono` if needed, or manual parsing for the common cases.

### C. git diff -- path

- libgit2 diff supports pathspec filtering via `DiffOptions::pathspec()`. Wire this through.

### D. VFS-aware status

- Walk VFS files under `repo_root`.
- For each file, check if it exists in the git index (via `repo.index()`).
  - Exists in index + content differs from VFS → "modified"
  - Not in index + exists in VFS → "untracked" (new file)
  - In index + not in VFS → "deleted"
- Format output to match `git status --porcelain` for agent compatibility.

### E. Spec fix

- Mechanical edit: replace "preToolUse hooks" language with "client capabilities" in the Requirements section of `spirit/spec-copilot-integration.md`.

## Files

```
# A. sed shim
tools/wasm/sd.wasm                          — compiled sd binary (if PoC passes)
crates/devdev-wasm/src/registry.rs          — shim table entry, sd module embedding
crates/devdev-wasm/src/native/sed_shim.rs   — argument translation (sed syntax → sd syntax)

# B. git log flags
crates/devdev-git/src/commands/log.rs       — --since, --follow, -- <path>

# C. git diff path filtering
crates/devdev-git/src/commands/diff.rs      — -- <path> filtering

# D. VFS-aware git status
crates/devdev-git/src/commands/status.rs    — NEW or extend existing status command

# E. Spec fix
spirit/spec-copilot-integration.md          — edit Requirements section
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-08-1 | §1 (Known Gaps) | sed/awk missing → agent gets exit 127 |
| SR-08-2 | §1 (Known Gaps) | git --since, --follow, path filtering missing |
| SR-08-3 | §1 (Known Gaps) | git status doesn't reflect VFS mutations |
| SR-08-4 | §3.7 | Build sd.wasm, wire sed shim |
| SR-08-5 | §3.7 | Extend git log with --since, --follow, -- <path> |
| SR-08-6 | §3.7 | Extend git diff with -- <path> |
| SR-08-7 | §3.7 | Reconcile ACP spec |

## Acceptance Tests

### A. sed/sd shim

- [ ] `sed_shim_basic_substitution` — `sed 's/hello/world/g' file.txt` → file content updated
- [ ] `sed_shim_inplace` — `sed -i 's/old/new/' file.txt` → file modified in VFS
- [ ] `sed_shim_stdin` — `echo "hello" | sed 's/hello/world/'` → stdout "world"
- [ ] `sed_shim_unsupported_warns` — `sed '/pattern/d' file.txt` → warning logged, best-effort execution
- [ ] `sd_direct_invocation` — `sd 'pattern' 'replacement' file.txt` → works without shim

### B. git log flags

- [ ] `git_log_since_filters_by_date` — `git log --since="2026-01-01"` → only recent commits
- [ ] `git_log_follow_tracks_rename` — `git log --follow renamed-file.rs` → includes pre-rename history
- [ ] `git_log_path_filter` — `git log -- src/main.rs` → only commits touching that file
- [ ] `git_log_combined_flags` — `git log --since="2026-01-01" --oneline -- src/` → combined filtering works

### C. git diff path filtering

- [ ] `git_diff_path_filter` — `git diff HEAD~1 -- src/config.rs` → only that file's diff
- [ ] `git_diff_multiple_paths` — `git diff HEAD~1 -- src/a.rs src/b.rs` → both files' diffs

### D. VFS-aware git status

- [ ] `git_status_vfs_modified` — write to a file in VFS → `git status` shows "modified"
- [ ] `git_status_vfs_new_file` — create new file in VFS → shows "untracked"
- [ ] `git_status_vfs_deleted` — remove file from VFS → shows "deleted"
- [ ] `git_status_vfs_unchanged` — no VFS mutations → clean status
- [ ] `git_status_porcelain_format` — output matches `git status --porcelain` format

### E. Spec reconciliation

- [ ] `spec_requirements_match_resolved` — (manual check) read the ACP spec, verify no contradictions

## Spec Compliance Checklist

- [ ] SR-08-1 through SR-08-7: all requirements covered
- [ ] sd.wasm PoC result recorded
- [ ] All acceptance tests passing
