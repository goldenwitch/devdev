---
id: S08
title: Multi-host adapter registry routes by host
status: ready
blocked-on: []
---

# S08 — Multi-host adapter registry routes by host

**User story.** A team mirrors the same PR identity (`platform/api#42`)
on `github.com` and on their internal `ghe.acme.io`. They run
`devdev up`, then watch both repos and add a monitor task for each.
The daemon must keep the two PRs separate at every layer — task
keys, ledger keys, event identities — so a comment posted to one
never lands on the other, and a credential intended for one host is
never surfaced to the other.

This scenario validates the [`RepoHostRegistry`](../../devdev-daemon/src/host_registry.rs)
seam end-to-end. The actual ADO/GHE HTTP traffic is covered by
adapter-level tests in `devdev-integrations`; S08's job is to prove
the **dispatch layer** never collides identities across hosts.

## Steps

1. Run `devdev up --data-dir <tmp> --github mock` and wait for the
   daemon's port file to appear.
2. IPC `repo/watch` with `{owner: "platform", repo: "api"}`
   (default host: `github.com`).
3. IPC `repo/watch` with `{owner: "platform", repo: "api", host:
   "ghe.acme.io"}`.
4. IPC `status`. Capture both task ids.
5. IPC `repo/watch` again for `{owner: "platform", repo: "api",
   host: "ghe.acme.io"}` (idempotent re-registration).
6. IPC `repo/unwatch` for the github.com pair.
7. IPC `status`.
8. IPC `repo/unwatch` for the ghe.acme.io pair.
9. `devdev down`.

## Assertions

* Step 2 and step 3 both return `already_watching: false` with
  **distinct** `task_id`s — same `(owner, repo)` on different
  hosts must not collide.
* Step 4 reports two `repo-watch` tasks.
* Step 5 returns `already_watching: true` and the same `task_id`
  as step 3.
* After step 6, only the ghe.acme.io watch remains.
* Step 7 reports exactly one `repo-watch` task (the GHE one).
* `repo/watch` with `host: "gitlab.example.com"` returns a
  `-32602` error ("not a recognised repo host") — unknown hosts
  are hard rejections, not silent github.com fallbacks.

## Guards against

* A regression where `repo_watch_ids` keys lose their `RepoHostId`
  prefix and conflate hosts.
* A change to `RepoHostId::from_browse_host` that accidentally
  classifies an unrelated forge as github/ghe/ado.
* A future credential-routing bug that surfaces a github.com token
  in response to a `host: "ghe.acme.io"` ask.

## Notes

This scenario does **not** exercise `MonitorPrTask` review
posting (the `gh`/`az` token surface is covered by the ask
provider unit tests in
[crates/devdev-daemon/src/mcp/provider.rs](../../devdev-daemon/src/mcp/provider.rs)).
S08's job is to prove the host id propagates through the user-
visible IPC surface so that the registry can do its job.
