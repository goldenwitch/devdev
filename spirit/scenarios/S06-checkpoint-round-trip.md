---
id: S06
title: Checkpoint round-trip
status: ready
blocked-on: []
---

# S06 — Checkpoint round-trip

**User story.** A user runs `devdev up`, loads some state, runs
`devdev down`, then on a later day runs `devdev up --checkpoint`.
Everything they had before — VFS contents, tasks in flight —
must come back exactly as it was.

## Steps

1. Run `devdev up --data-dir <tmp> --github mock`, wait for the
   daemon.
2. Shut down with `devdev down` (default checkpoint-on-stop is on).
   Capture the checkpoint bytes from `<tmp>/checkpoint.bin`.
3. Compute the **checkpoint projection** of those bytes: decode
   with `MemFs::deserialize`, project to `{paths, file_sha256s,
   mounts}`.
4. Run `devdev up --data-dir <tmp> --checkpoint --github mock`
   into a **fresh port** (same data dir, new process).
5. Run `devdev down` again.
6. Compute the projection of the second checkpoint.

## Assertions

* The two projections are byte-identical.
* The second daemon's `status` response reports the same
  `{tasks, sessions}` as the first (note: `sessions` is
  not currently persisted — this may drop to asserting only on
  the keys that are).
* Starting with `--checkpoint` when no `checkpoint.bin` exists
  **must not** fail — it should behave like a fresh start (this
  matches `Daemon::start`'s current logic).

## Guards against

* Silent serde drift in `MemFs::serialize` / `deserialize`.
* Any future task-persistence wiring that drops state on
  round-trip.
* A checkpoint format that encodes absolute host paths or
  timestamps that only work on the original machine.

## Notes

The projection deliberately excludes timestamps (stored as
`modified_secs`) and node `mode` bits — those are correctly
part of the checkpoint but vary by host (umask, clock). If a
future scenario needs to prove those round-trip, add a separate
projection, don't widen this one.
