---
id: S05
title: Teardown leaves nothing
status: ready
blocked-on: []
---

# S05 — Teardown leaves nothing

**User story.** After any non-trivial session, running `devdev
down` must not leak state into the user's environment. A daemon
that silently builds up garbage is a daemon users will stop
trusting.

## Steps

1. Run the steps of S01 end-to-end, then additionally:
2. Confirm that after `devdev down` exits, the scratch data dir
   contains **only** the expected artifacts
   (`checkpoint.bin`; no `daemon.pid`, no `daemon.port`, no stray
   temp files).
3. Confirm that nothing outside the scratch dir changed during
   the run.

## Assertions

* The set of files under `<tmp>` after shutdown is a subset of
  `{checkpoint.bin}`.
* No file outside `<tmp>` was created, modified, or deleted
  during the run (snapshot-diff the parent directory of `<tmp>`;
  the scratch dir is the only expected delta).

## Guards against

* Future features that write to `~/.devdev/logs/…` or similar
  without taking `data_dir` into account.
* Orphaned sockets or lock files.
* "Demo mode" shortcuts that stash state in `/tmp`.
