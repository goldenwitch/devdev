---
id: 29-workspace-repo-setup
title: Workspace Repo Setup
status: done
type: leaf
phase: 4
crate: devdev-workspace
priority: P0
depends-on: [00-vfs-core]
effort: S
---

# Workspace Repo Setup

## Why

The agent's core workflow is "evaluate a repo." That means the daemon
needs to be able to put a real git repository *inside the workspace*
and have it behave like a real repository once mounted: files at the
right POSIX paths, `.git/` intact, commands like `git log` working
against it, and everything surviving a checkpoint snapshot.

Before this capability, we had mount primitives (`Fs` + FUSE +
WinFSP) and we had `Workspace::exec`, but nothing pinned down the
end-to-end: *seed real bytes from a host git repo into the `Fs`,
mount, and read them back as a functioning repo.* Without that proof
we'd only find out a shape mismatch (permissions, path joining,
snapshot fidelity for `.git/`) when the first agent workflow tried
to clone something.

## Scope

Prove, with tests that run against the **live** mount driver on both
supported host OSes:

1. A host-side git repo can be copied into `Fs` via `mkdir_p` /
   `write_path` using a trivial recursive walk. No new public API.
2. The mounted view exposes the worktree *and* `.git/` at the
   expected POSIX paths with byte-identical content.
3. `Fs::serialize` / `Fs::deserialize` preserves the repo — a
   fresh `Workspace::from_fs` mounts the revived image and the
   same bytes are visible.
4. On Linux, `Workspace::exec` running `git log --oneline` inside
   the mount lists the seeded commits. This proves the materialised
   bytes form a valid git repository from the perspective of a real
   git binary, not just a directory of lookalike files.

On Windows, (4) is deferred — `Workspace::exec` sets `HOME=/home/agent`
which doesn't resolve to a real Windows path, so git's config resolver
misfires. This is the same containment gap documented in
`tests/cargo_build.rs`. (1)-(3) still run through WinFSP.

## Contract

No new public API. This capability is a *proof*, not a feature. It
locks the following existing contract in tests so we catch drift:

- `Fs::mkdir_p(path, mode)` and `Fs::write_path(path, bytes)` are
  sufficient to lay down an arbitrary git repo.
- The mount point presents every `Fs` path at
  `<mount_point>/<posix-path-without-leading-slash>` with platform
  separator conversion.
- `Fs::serialize` / `Fs::deserialize` are repo-faithful, including
  `.git/` internals (objects, refs, HEAD).

## Acceptance

`crates/devdev-workspace/tests/repo_setup.rs`:

- `repo_materialises_in_fs_and_reads_through_mount` — seeds a repo,
  mounts, reads `README.md`, `src/main.rs`, and `.git/HEAD` through
  the mount. Default-run on Linux; `#[ignore]` on Windows (WinFSP
  must be installed; also needs `--test-threads=1` due to drive-letter
  auto-probe collisions).
- `repo_survives_fs_snapshot_roundtrip` — serialises the `Fs`,
  revives into a fresh `Workspace`, remounts, re-reads. Same platform
  gating as above.
- `git_log_reads_materialised_repo` — Linux-only (`#[cfg(target_os =
  "linux")]`); runs `git log --oneline` via `Workspace::exec` and
  asserts the seeded commit SHA and subject appear in the output.

## Non-goals

- Cloning from a remote — the daemon seeds by copying bytes it
  already has. Remote-clone workflows are a daemon concern.
- Symlink preservation — the seeder skips symlinks; real repos
  rarely need them at this layer.
- Windows `git log`-via-exec coverage — blocked on the HOME
  containment gap tracked in `tests/cargo_build.rs`.

## Status

**Done (2026-04-22).** Tests green on Windows with WinFSP
(`--ignored --test-threads=1`) and on Linux with FUSE.
