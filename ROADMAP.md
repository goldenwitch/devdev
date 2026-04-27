# Roadmap

DevDev is two things at once, and we intend to ship them on different
schedules:

1. **`devdev-workspace`** — a virtual workspace for agents, as a
   library. Reasonably usable on Linux and Windows today.
2. **`devdev`** — the full agent-hosting product. Proof-of-concept on
   the critical path; several large pieces are still behind
   placeholders.

This document is an honest account of where each one stands.

## Today (shipped)

Works end-to-end and is exercised by tests on every push.

**Workspace layer (`crates/devdev-workspace`)**

- In-memory POSIX-ish `Fs` with rename, hardlink, symlink, seek,
  truncate, `O_APPEND`, mode bits, timestamps.
- FUSE driver on Linux; WinFSP driver on Windows (requires WinFSP
  runtime installed).
- `Workspace::exec` — run a real host binary inside the mount under a
  PTY, curated env.
- Serializable snapshots (`bincode`-stable).

**Agent glue**

- ACP backend: spawn Copilot CLI as `copilot --acp --allow-all-tools`,
  observe via `session/update`.
- MCP tool injection: DevDev-specific tools (task queries, preferences)
  surface inside the ACP session. Proven end-to-end with a logged-in
  Copilot CLI.
- Daemon lifecycle: `devdev up` / `devdev down`, TCP NDJSON IPC on
  `127.0.0.1`, checkpoint files in `--data-dir`.
- Scenario harness: user-surface scenarios drive only the `devdev`
  binary + IPC + checkpoints + documented env vars.

**Repo watch → event pipeline (cap 26 / cap 27)**

- `devdev repo watch <owner>/<repo>` polls GitHub, hashes PR state,
  consults an append-only NDJSON idempotency ledger, and emits
  `PrOpened` / `PrUpdated` / `PrClosed` events on an internal
  `EventBus`. Per-PR `MonitorPrTask`s subscribe and re-prompt the
  agent on update.
- `devdev_ask` MCP tool: the universal approval seam. Agent calls it
  with `kind={post_review,post_comment,request_token,question}`;
  daemon routes through `ApprovalGate` and — on approval, for the
  external-action kinds — surfaces a host-derived short-lived
  `GH_TOKEN` so the agent can run `gh` itself. No typed adapter path.

**Vibe Check (cap 25)**

- `devdev init` runs a scribe session that writes `.devdev/*.md`
  preference files in the user's voice (one topic per file, append
  `## Revision <date>` on revisits).
- `devdev preferences list` discovers preferences across repo /
  parents / `~/.devdev/` with repo-wins precedence; `devdev
  preferences edit` opens `$EDITOR`.

**Scenario catalog status**

| ID | Status |
|----|--------|
| S01 empty workspace up/down | ready |
| S05 teardown leaves nothing | ready |
| S06 checkpoint round-trip | ready |
| S03 agent uses the toolbelt | blocked (session backend) |
| S04 event arrives mid-session | blocked (session backend) |

## Next (in flight)

What we're actively working on to close the DevDev-hosting loop.

- **Scout routing.** Pick the right model/agent per task class instead
  of one-size-fits-all.
- **Full ACP session backend (S03/S04).** Enough plumbing that the
  agent's tool calls and mid-session events are observable from the
  scenario surface.
- **End-to-end PR shepherding scenario (S07).** Drives `devdev init`
  → `devdev repo watch` → mock GH adapter → asserts the agent gets
  re-prompted with preference context on each PR update.

### Explicitly not on this list

- **A `devdev repo` command or `--repo` flag.** The workspace is
  repo-unaware by design and stays that way. When a task needs a
  repo inside a workspace, the agent materialises it by running
  `git clone` through the workspace's process launcher — the same
  surface a human would use. See
  [`spirit/02-workspace-contract.md`](spirit/02-workspace-contract.md)
  on what the workspace is unaware of, and
  [`spirit/04-tasks.md`](spirit/04-tasks.md) for how task-layer
  context (repo refs, PR numbers) reaches the agent.

## Aspirational

Direction of travel. Not started, not scheduled, but on the record so
nobody is surprised.

- **Real containment.** This is the single biggest gap. Today DevDev
  runs agent-driven processes as your user, with your network, against
  your filesystem — the virtual workspace is a friendly path, not a
  jail. Target: Linux namespaces + seccomp-bpf, Windows job objects /
  AppContainer, opt-in network policy.
- **macOS support.** Needs a third FS driver (macFUSE or a native
  FSKit implementation) plus a containment story.
- **Published crates.** `devdev-workspace` to crates.io once the API
  has stabilized and the containment story is honest enough that the
  description doesn't need a disclaimer.
- **Coverage gate.** Coverage is measured non-gating today; once the
  DevDev-hosting loop closes, raise it to a threshold.
- **Reusable checkpointing across machines.** Snapshots are
  deterministic, but the DevDev checkpoint format is not yet a
  portable wire format.

## What this roadmap is not

- Not dated. We will not commit to calendar timelines at this stage.
- Not exhaustive for every bug or polish item — see issues for those.
- Not a promise. Priorities shift as we learn.
