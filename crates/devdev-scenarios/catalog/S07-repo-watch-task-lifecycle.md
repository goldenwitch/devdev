---
id: S07
title: Repo watch task lifecycle
status: ready
blocked-on: []
---

# S07 — Repo watch task lifecycle

**User story.** A user runs `devdev up`, then `devdev repo watch
owner/repo` to point DevDev at a GitHub repository. The daemon
spins up a `RepoWatchTask`. Later they run `devdev repo unwatch
owner/repo`; the task disappears. Everything stays inside the
data dir.

This scenario validates the IPC plumbing and task lifecycle for
the repo-watch feature against the **mock** GitHub adapter — no
real API calls. Actual PR-event delivery and re-prompting of
agents is covered by `crates/devdev-daemon/tests/e2e_pr_shepherding.rs`.

## Steps

1. Run `devdev up --data-dir <tmp> --github mock`, wait for the
   daemon.
2. IPC `repo/watch` with `{owner: "fake", repo: "repo"}`.
3. IPC `status`. Capture task count.
4. IPC `repo/watch` again with the same params (should be
   idempotent).
5. IPC `status`. Task count must be unchanged.
6. IPC `repo/unwatch` with the same params.
7. IPC `status`. Task count back to baseline.
8. `devdev down`.

## Assertions

* Step 2 returns `{ task_id: <non-empty>, already_watching: false }`.
* Step 4 returns `already_watching: true` and the **same** `task_id`.
* Step 5's task count equals step 3's task count (idempotency).
* Step 6 returns the same `task_id` it cancelled.
* Step 7's task count is back to baseline (≤ step 3's count − 1).
* The outer scratch is host-confined: every diff lives inside
  `data_dir`.

## Guards against

* A regression in `dispatch::handle_repo_watch`/`handle_repo_unwatch`
  that breaks idempotency.
* A future change that leaks repo state outside the data dir.
* An unwatch that silently leaves the task running.

## Notes

This scenario does **not** exercise event delivery, MonitorPrTask
spawning, or `devdev_ask`. Those paths have unit and e2e coverage
in the `devdev-daemon` and `devdev-tasks` crates. S07's job is to
prove the user-surface plumbing is wired end-to-end through the
real binary.
