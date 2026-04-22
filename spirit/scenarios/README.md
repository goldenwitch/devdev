# DevDev Scenarios

**What these are.** Each `S*.md` file here is a user-surface scenario —
a thing a person installing the `devdev` binary expects to be able to
do. Every scenario is paired 1:1 with a `#[tokio::test]` of the same
name in [../../crates/devdev-scenarios/tests/scenarios.rs](../../crates/devdev-scenarios/tests/scenarios.rs);
an integrity meta-test fails the build if that pairing ever drifts.

**The contract.** Scenarios may drive DevDev **only** through surfaces
a real user hits:

* the `devdev` binary (via `assert_cmd`),
* the IPC protocol it exposes (TCP NDJSON on `127.0.0.1`),
* checkpoint files written to `--data-dir`,
* environment variables documented on the man page (`DEVDEV_HOME`,
  `DEVDEV_GITHUB_ADAPTER`, `GH_TOKEN`).

Scenarios must **never** import `devdev-shell`, `devdev-wasm`,
`devdev-git`, `devdev-acp`, or construct `MemFs` / `ShellSession` /
`AcpClient` directly. The scenarios crate's `Cargo.toml` declares
exactly the deps a user-surface harness needs; a CI guard enforces
that nothing engine-internal sneaks in. If a scenario can only be
proved by peeking at engine internals, rewrite it as an engine-level
acceptance test in the owning crate — not here.

**Why the boundary matters.** The engine will be refactored — libgit2
may be swapped for gitoxide, the shell executor may be rewritten, VFS
may be reshaped. If those refactors don't change what a user can do
or observe, every scenario here must still pass without edits. That
is the whole point.

## Front matter schema

```yaml
---
id: S01              # matches filename prefix and test fn name
title: string
status: draft | ready
blocked-on: []       # list of capability IDs or freeform strings
---
```

## Assertion vocabulary

Scenarios prefer these assertions, in roughly this order:

1. **IPC response shape** — the structural contract a user's script
   would assert against.
2. **Checkpoint projection diff** — after a steady state, `devdev
   down` is called, the checkpoint is decoded into a stable shape
   (paths + file SHA256s + task descriptors), and compared to a
   committed fixture. Decoded-projection, not raw bincode, so serde
   layout changes don't cascade into scenario churn. Rebuild with
   `UPDATE_SCENARIO_FIXTURES=1 cargo test -p devdev-scenarios`.
3. **Host-isolation check** — every scenario wraps its work in a
   scratch `tempdir`; before/after snapshots of the host filesystem
   outside the scratch path must match.
4. **Process observables** — exit codes, stderr substrings,
   stdout JSON shapes from the `devdev` binary.

## Current catalog

| ID | Title | Status | Notes |
|----|-------|--------|-------|
| S01 | Empty workspace up and down | ready | |
| S02 | Load local repo into workspace | draft | needs repo-load IPC |
| S03 | Agent uses the toolbelt | blocked | ACP session backend not yet wired |
| S04 | Event arrives mid-session | blocked | same |
| S05 | Teardown leaves nothing | ready | |
| S06 | Checkpoint round-trip | ready | |

When a blocked scenario unblocks, flip `status: ready` in its front
matter and write the test.
