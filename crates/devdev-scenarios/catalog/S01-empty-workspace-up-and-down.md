---
id: S01
title: Empty workspace up and down
status: ready
blocked-on: []
---

# S01 — Empty workspace up and down

**User story.** A user runs `devdev up` against a fresh data
directory, asks the daemon whether it is alive, and runs `devdev
down`. Nothing about their real home directory or any other repo
has been touched.

## Steps

1. Create a scratch `DEVDEV_HOME` pointing to an empty temp dir.
2. Spawn `devdev up --data-dir <tmp> --foreground --github mock`.
3. Poll for `<tmp>/daemon.port` to appear (timeout: 5 s).
4. Over IPC, send method `status` with empty params.
5. Spawn `devdev down --data-dir <tmp>`.
6. Wait for the `up` process to exit.

## Assertions

* `status` response has both `tasks` and `sessions` keys (IPC
  shape contract).
* `up` exits with status 0.
* After shutdown:
  * `<tmp>/daemon.pid` does not exist.
  * `<tmp>/daemon.port` does not exist.
  * `<tmp>/checkpoint.bin` exists and decodes to an empty `MemFs`
    (no files, no mounts).
* Host isolation: no file was created or modified outside `<tmp>`.

## Guards against

* Regressions in single-instance PID management
  (`crates/devdev-daemon/src/pid.rs`).
* Regressions in port-file lifecycle.
* Regressions in checkpoint-on-stop (`Daemon::stop` should always
  write a checkpoint when `checkpoint_on_stop` is set).
* Any future change that introduces writes to `~/.devdev` or other
  host paths during an otherwise scoped run.
