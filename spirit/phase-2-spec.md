# Phase 2 Spec: The Persistent Sandbox

**Date:** April 19, 2026
**Status:** Draft
**Prerequisite:** Phase 1 postmortem (Postmortem.md)

---

## 1. Where We Are

### What Exists (Phase 1 Output)

Six crates forming a virtual unix environment:

| Crate | Lines | Tests | What It Does |
|-------|-------|-------|--------------|
| `devdev-vfs` | ~1,355 | 28 | In-memory POSIX filesystem (BTreeMap, 2 GiB cap) |
| `devdev-wasm` | ~1,800 | 32 | 13 WASM coreutils + 3 native tools (grep/find/diff) |
| `devdev-git` | ~1,116 | 26 | 9 read-only git commands via libgit2 |
| `devdev-shell` | ~2,200 | 64 | Bash-subset parser, pipeline engine, 7 builtins |
| `devdev-acp` | ~2,300 | 34 | ACP client, sandbox handler, thread-pinned shell worker |
| `devdev-cli` | ~1,400 | 25 | One-shot `evaluate()` orchestrator, CLI binary |

**What's sound:** The engine crates (vfs, wasm, git, shell, acp) don't assume a one-shot lifecycle. MemFs can live indefinitely. ShellSession is stateful across commands. AcpClient can manage sessions over time. These are reusable.

**What's wrong:** The orchestration layer (`devdev-cli`) hardcodes a single-shot model: create sandbox → load one repo → run one conversation → destroy. The CLI binary exposes `devdev eval` — a function call, not a service.

### Known Gaps in the Engine

> **Historical (2026-04-19).** This section reflects the state at the start of Phase 2. Most of these gaps were either closed (Phase 2: git `--since`/`--follow`/path filtering, git diff `-- path`) or rendered moot by Phase 3 consolidation (the `devdev-wasm`, `devdev-git`, and `devdev-shell` crates were collapsed into `devdev-workspace`, which mounts a real OS filesystem via FUSE/WinFSP — no more VFS temp-dir materialization). Preserved for historical context.

| Gap | Impact | Effort |
|-----|--------|--------|
| sed/awk missing | Agent gets exit 127 on common commands | Medium — build `sd.wasm`, wire shim |
| git `--since`, `--follow`, path filtering | Agent can't filter log or diff by path | Small — extend existing command modules |
| git status doesn't reflect VFS mutations | Agent sees stale working-tree state | Medium — diff VFS against index |
| WASM temp-dir materialization is O(VFS) | Slow on large repos, repeated disk I/O | Large — investigate Wasmtime VFS adapters or selective materialization |
| Git temp-dir loading | Disk I/O per evaluation | Medium — investigate mempack or lazy loading |

These are Phase 2 cleanup tasks, not blockers for the architecture change.

---

## 2. Where We Want to Go

### The User Experience

```
$ devdev up
DevDev daemon started (pid 41823)

$ devdev up --checkpoint
DevDev daemon started from checkpoint (pid 41824, 3 repos loaded)

$ devdev
┌─────────────────────────────────────────┐
│ DevDev                                  │
├─────────────────────────────────────────┤
│ > Monitor the PR at                     │
│   github.com/org/repo/pull/247          │
│                                         │
│ Loading org/repo into workspace...      │
│ Fetching PR #247 diff...                │
│ Watching for updates.                   │
│                                         │
│ I've reviewed the PR. 3 files changed,  │
│ 47 additions. I found two issues:       │
│                                         │
│ 1. The new `parse_config()` function    │
│    doesn't validate the `timeout` field │
│    — it accepts negative values.        │
│                                         │
│ 2. The test in `test_config.rs` only    │
│    covers the happy path. No test for   │
│    missing fields or invalid types.     │
│                                         │
│ Want me to draft review comments?       │
│                                         │
│ > Yes, and keep watching. Flag anything │
│   new that gets pushed.                 │
│                                         │
│ Comments drafted. I'll watch for force  │
│ pushes and new commits.                 │
└─────────────────────────────────────────┘

$ devdev down
Checkpoint saved. Daemon stopped.
```

### The Architecture

```
┌──────────────────────────────────────────────────────────┐
│                    devdev daemon                          │
│                                                          │
│  ┌─────────────┐  ┌─────────────────────────────────┐   │
│  │  TUI / Chat  │  │     Task Manager                │   │
│  │  Interface   │──│  (active tasks, polling loops)  │   │
│  └─────────────┘  └──────────┬──────────────────────┘   │
│                               │                          │
│                    ┌──────────▼──────────────┐           │
│                    │    Integration Layer     │           │
│                    │  (adapter pattern)       │           │
│                    │                          │           │
│                    │  ┌─────────┐ ┌────────┐ │           │
│                    │  │ GitHub  │ │ Local  │ │           │
│                    │  │ Adapter │ │  Git   │ │           │
│                    │  └─────────┘ └────────┘ │           │
│                    └──────────┬──────────────┘           │
│                               │                          │
│                    ┌──────────▼──────────────┐           │
│                    │   Session Router         │           │
│                    │  (maps tasks → agent     │           │
│                    │   sessions)              │           │
│                    └──────────┬──────────────┘           │
│                               │                          │
│            ┌──────────────────▼────────────────────┐     │
│            │         Sandbox (Phase 1 engine)      │     │
│            │                                       │     │
│            │  ┌──────┐ ┌───────┐ ┌──────┐ ┌─────┐│     │
│            │  │ VFS  │ │ Shell │ │ WASM │ │ Git ││     │
│            │  │(MemFs)│ │Session│ │Tools │ │     ││     │
│            │  └──────┘ └───────┘ └──────┘ └─────┘│     │
│            └──────────────────┬────────────────────┘     │
│                               │                          │
│                    ┌──────────▼──────────────┐           │
│                    │   ACP Client             │           │
│                    │  (Copilot CLI subprocess)│           │
│                    └─────────────────────────┘           │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │  Checkpoint Manager                               │   │
│  │  (serialize/deserialize VFS + task state to disk) │   │
│  └──────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

### Core Principles

1. **The daemon is the product.** `devdev up` starts it, `devdev down` stops it, `devdev` (no subcommand) opens the chat interface. Everything else is a background task.

2. **The sandbox is persistent.** The VFS lives as long as the daemon. Repos are loaded in and stay loaded. The agent accumulates context over time. Checkpoints serialize state to disk for daemon restart.

3. **Tasks, not evaluations.** The user gives DevDev tasks ("monitor this PR", "review all PRs in this repo", "watch for dependency updates"). Tasks are long-lived. They may poll, react to events, and produce output over hours or days.

4. **Adapters, not hardcoded integrations.** GitHub is the first adapter. The integration surface is a trait — fetch PR, post comment, list files, get diff. Future adapters (GitLab, Jira, Linear, local git hooks) implement the same trait.

5. **Copilot is the agent.** We don't abstract over multiple LLM backends. The ACP client talks to Copilot CLI, period. If that changes, it's a Phase 3 concern.

6. **Every HITL path has a headless equivalent.** Anywhere a human can interact via the TUI, there is a corresponding non-interactive path — CLI commands, structured I/O, and auto-approve flags. DevDev must be fully scriptable and embeddable in CI pipelines without a terminal.

---

## 3. What Needs to Be Built

### 3.1 Daemon Lifecycle (`devdev-daemon`)

**New crate.** Manages the long-running process.

| Component | Responsibility |
|-----------|---------------|
| `daemon::start()` | Boot the daemon, optionally from checkpoint |
| `daemon::stop()` | Save checkpoint, shut down cleanly |
| Process management | PID file, single-instance guard, signal handling (SIGTERM → checkpoint + exit) |
| IPC | Communication channel between CLI commands and the running daemon (Unix socket / named pipe) |

**CLI surface:**
- `devdev up` — start daemon (foreground or detached)
- `devdev up --checkpoint` — start from last checkpoint
- `devdev down` — checkpoint + stop
- `devdev status` — is the daemon running, what tasks are active (human-readable default, `--json` for scripts)
- `devdev` (bare) — open TUI, connect to running daemon
- `devdev attach --headless` — connect to daemon via stdin/stdout NDJSON (no TUI, for piping and CI)
- `devdev task add "Monitor PR #247 in org/repo"` — create a task without entering the TUI
- `devdev task list` — list active tasks (`--json` for scripts)
- `devdev task cancel <id>` — cancel a task without entering the TUI
- `devdev send "Review the latest push"` — send a one-shot message to the daemon, print response, exit

**Checkpoint format:** Serialize VFS tree + task state + shell environments to a single file on disk. Binary format (bincode or msgpack) for speed. The checkpoint is a snapshot, not a journal — `devdev down` writes the full state, `devdev up --checkpoint` loads it whole.

**What we're NOT building:** Process supervision (systemd, launchd). The daemon is a foreground process that the user starts and stops. Service integration is a future concern.

### 3.2 Chat Interface (`devdev-tui`)

**New crate.** Terminal UI for user interaction.

| Component | Responsibility |
|-----------|---------------|
| Input/output | Scrollable chat history, input line, markdown rendering |
| IPC client | Connect to running daemon via socket |
| Message routing | User messages → daemon → agent → daemon → display |

**Minimal viable TUI:**
- Single-pane chat (no splits, no tabs for v1)
- User types at bottom, messages scroll up
- Agent responses stream in token-by-token
- Status bar: daemon status, active tasks, loaded repos

**Library choice:** `ratatui` (Rust TUI framework, mature, cross-platform).

**Headless mode:** `devdev attach --headless` connects to the same daemon IPC but reads/writes NDJSON on stdin/stdout instead of rendering a TUI. One JSON object per message in, one per message out. This is the integration surface for:
- CI pipelines (`echo '{"text":"Review PR #247"}' | devdev attach --headless`)
- Editor extensions (VS Code, Neovim) that want to embed DevDev
- Scripts that automate multi-step workflows
- Testing (deterministic I/O, no terminal emulation)

The TUI and headless mode share the same IPC client crate. The TUI is a rendering layer on top; headless is the raw protocol.

**What we're NOT building:** A web UI, an Electron app, or a VS Code extension. Terminal first, headless always.

### 3.3 Task Manager (`devdev-tasks`)

**New crate.** Manages long-lived background tasks.

| Component | Responsibility |
|-----------|---------------|
| `Task` trait | Define a unit of ongoing work |
| Task registry | Track active tasks, their state, their polling intervals |
| Scheduler | Drive polling loops, handle backpressure |
| Task persistence | Serialize task state into checkpoint |

**Task trait:**

```rust
trait Task: Send + Sync {
    /// Unique identifier for this task instance.
    fn id(&self) -> &str;

    /// Human description ("Monitoring PR #247 in org/repo").
    fn describe(&self) -> String;

    /// Called on schedule or event. Returns messages for the user.
    async fn poll(&mut self, ctx: &mut TaskContext) -> Vec<TaskMessage>;

    /// Serialize state for checkpoint.
    fn serialize(&self) -> serde_json::Value;
}
```

**First task implementation:** `MonitorPrTask` — watches a single PR, reacts to new commits, reviews changes, drafts comments.

**TaskContext** provides access to: the sandbox (VFS + shell + tools + git), the agent (ACP session), and integration adapters (GitHub API).

**Approval policy:** Tasks that want to take external actions (post a review, comment on a PR) go through an approval gate. The gate has three modes:

| Mode | Behavior | Flag |
|------|----------|------|
| **Ask** (default) | Queue the action, notify the user via TUI/headless, wait for approval | — |
| **Auto-approve** | Execute immediately, log the action | `--auto-approve` or `devdev task add --auto-approve "..."` |
| **Dry-run** | Log what would happen, never execute | `--dry-run` |

In headless mode, pending approvals are emitted as NDJSON messages. A script can respond with `{"approve": true}` or `{"approve": false}`. If no response arrives within a configurable timeout, the action is dropped (not auto-approved — fail-safe).

This replaces the outline's `--rude` flag with a more granular model. `--auto-approve` is `--rude` by another name.

### 3.4 Integration Layer (`devdev-integrations`)

**New crate.** Adapter pattern over external services.

**GitHub adapter (first):**

```rust
trait GitHubAdapter: Send + Sync {
    async fn get_pr(&self, owner: &str, repo: &str, number: u64) -> Result<PullRequest>;
    async fn get_pr_diff(&self, owner: &str, repo: &str, number: u64) -> Result<String>;
    async fn list_pr_comments(&self, owner: &str, repo: &str, number: u64) -> Result<Vec<Comment>>;
    async fn post_review(&self, owner: &str, repo: &str, number: u64, review: Review) -> Result<()>;
    async fn post_comment(&self, owner: &str, repo: &str, number: u64, comment: Comment) -> Result<()>;
    async fn get_pr_status(&self, owner: &str, repo: &str, number: u64) -> Result<PrStatus>;
}
```

**Authentication:** Reuse `GH_TOKEN` from Phase 1 (already validated with Copilot). The same token works for GitHub API calls.

**Rate limiting:** Respect GitHub API rate limits. Back off on 429. Log remaining quota.

**What we're NOT building yet:** Webhook receivers. Phase 2 polls. Webhooks are a Phase 3 optimization.

### 3.5 Session Router

**Lives in `devdev-daemon`.** Maps tasks to agent sessions.

| Concern | Approach |
|---------|----------|
| Session lifecycle | ACP sessions are created per-task, not per-daemon. A task that monitors a PR has its own session with accumulated context. |
| Session multiplexing | One Copilot CLI subprocess, multiple logical sessions (ACP supports `session/new` for each). |
| Session death | If the Copilot process crashes, restart it and recreate active sessions. Tasks are durable; sessions are not. |
| Context management | Each task injects its context (repo state, PR diff, prior observations) into its session prompt. |

### 3.6 Sandbox Lifecycle Changes

> **Historical (2026-04-22).** This subsection enumerated changes to `devdev-vfs`, `devdev-git`, `devdev-shell`, and `devdev-acp`. Phase 3 deleted the first three (`devdev-vfs`, `devdev-git`, `devdev-shell`) and consolidated their responsibilities into `devdev-workspace`, which mounts a real OS filesystem via FUSE/WinFSP. `devdev-acp` survived. The product intent below ("VFS persistence", "multiple repos", "shell reuse", "multi-session ACP") still matters and feeds the remaining Phase-2 work (P2-06 session router) plus a future Phase-5 checkpoint redesign — but the table's crate column for the deleted crates and `MemFs`-based API references are obsolete. Preserved for context.

**Modifications to existing crates.** The sandbox must support:

| Change | Crate | Description |
|--------|-------|-------------|
| VFS persistence | `devdev-vfs` | `MemFs::serialize()` / `MemFs::deserialize()` for checkpoint save/restore |
| Multiple repos | `devdev-vfs` | Load multiple repos into the same VFS under different mount points (`/repos/org/name/`) |
| Git multi-repo | `devdev-git` | `VirtualRepo` keyed by repo path, not assuming single root |
| Shell session reuse | `devdev-shell` | ShellSession persists across task invocations (already supported, just needs wiring) |
| ACP session management | `devdev-acp` | Support multiple concurrent sessions over one Copilot subprocess |

### 3.7 Engine Cleanup (from Phase 1 gaps)

> **Historical (2026-04-22).** Most items below were rendered moot by Phase 3: with a real OS mount, the agent runs the host's actual `sed`, `awk`, and `git` binaries — no WASM shim needed, no per-command flag gaps. "Reconcile ACP spec" landed in Phase 1 cleanup. Preserved for context.

| Item | Crate | Work |
|------|-------|------|
| Build `sd.wasm`, wire sed shim | `devdev-wasm` | Compile sd to wasm32-wasip1, add shim entry, translate `sed` → `sd` args |
| awk (P2 — defer) | — | Not Phase 2 |
| `git log --since`, `--follow` | `devdev-git` | Extend `commands/log.rs` |
| `git diff -- <path>` filtering | `devdev-git` | Extend `commands/diff.rs` |
| `git status` VFS-aware | `devdev-git` | Diff VFS working tree against index |
| Reconcile ACP spec | `spirit/` | Rewrite Requirements section to match Resolved Questions |

---

## 4. How We Will Test and Validate

### Testing Strategy per Component

| Component | Test Approach | What "Passing" Means |
|-----------|--------------|---------------------|
| Daemon lifecycle | Integration tests: start → IPC ping → stop → verify clean exit. Checkpoint: start → load repo → checkpoint → restart from checkpoint → verify VFS state matches. | Daemon boots, accepts connections, checkpoints correctly, restores correctly. |
| TUI | Headless terminal simulation (ratatui has test backends). Verify: user input dispatched, agent response rendered, status bar updated. | Messages round-trip through TUI without corruption. Chat history scrolls. |
| Headless attach | Send NDJSON messages on stdin, verify NDJSON responses on stdout. Round-trip: message in → daemon → agent → daemon → message out. Approval flow: emit pending action → send `{"approve": true}` → verify action executed. | Full conversation works over pipes. Approval protocol correct. |
| CLI task commands | `devdev task add`, `task list --json`, `task cancel`, `devdev send --json`. Verify structured output matches schema. | Tasks can be created, queried, and cancelled without TUI. One-shot `send` returns valid JSON. |
| Approval gate | Unit tests for all three modes: ask (blocks until response), auto-approve (executes immediately), dry-run (logs only, never executes). Headless timeout: no response within timeout → action dropped. | External actions only fire when explicitly approved or auto-approved. Dry-run never mutates. Timeout is fail-safe. |
| Task Manager | Unit tests with mock adapters and mock agent. MonitorPrTask: given a PR diff, verify it produces review. Given no changes, verify it stays quiet. Given a new push, verify it re-reviews. | Tasks poll correctly, produce expected messages, serialize/deserialize for checkpoint. |
| GitHub Adapter | Integration tests against GitHub API using a test repo (gated behind `DEVDEV_E2E`). Unit tests with recorded HTTP responses (wiremock). | Adapter fetches PRs, posts comments, respects rate limits. |
| Session Router | Unit tests: create session, route task message, verify agent receives it. Crash recovery: kill Copilot subprocess, verify restart and session recreation. | Tasks maintain agent sessions. Crash recovery doesn't lose task state. |
| VFS serialization | Round-trip: populate VFS → serialize → deserialize → verify identical tree (content, permissions, structure). | Checkpoint preserves all VFS state bit-for-bit. |
| Multi-repo VFS | Load two repos, verify isolation. Agent in repo A can't accidentally read repo B (unless asked). | Mount points are clean. Path resolution respects boundaries. |

### Acceptance Criteria for "PR Shepherding Works"

**Scenario A: Interactive (TUI)**

1. User starts daemon: `devdev up`
2. User opens TUI: `devdev`
3. User says: "Monitor PR #247 in org/repo"
4. DevDev: loads repo into VFS, fetches PR diff via GitHub adapter, creates ACP session, injects context
5. Agent: explores codebase via sandbox tools, produces review
6. DevDev: displays review in TUI, asks "Want me to post this?"
7. User: "Yes"
8. DevDev: posts review via GitHub adapter
9. PR author pushes a new commit
10. DevDev: (on next poll) detects new commit, fetches updated diff, re-reviews, notifies user
11. User: `devdev down` → checkpoint saved

**Scenario B: Headless (CI/scripting)**

1. CI starts daemon: `devdev up`
2. CI creates task: `devdev task add --auto-approve "Monitor PR #247 in org/repo"`
3. DevDev: loads repo, fetches diff, reviews, posts review automatically (no approval wait)
4. CI polls: `devdev status --json` until task reports idle
5. CI reads output: `devdev task log <id> --json` → structured review result
6. CI stops: `devdev down`

**Scenario C: One-shot (quick review)**

1. `devdev up && devdev send --auto-approve "Review PR #247 in org/repo" --json && devdev down`
2. Structured JSON output on stdout. No TUI, no long-running task. Closest to Phase 1's `devdev eval` but routed through the daemon.

**E2E test:** Scripted version of Scenario B using a test GitHub repo and the fake agent (extended to handle multi-turn sessions). Gated behind `DEVDEV_E2E`.

**Deterministic test:** Same flow with mock GitHub adapter and duplex-based fake agent. No network, no tokens, fast. Exercises both TUI rendering (via ratatui test backend) and headless NDJSON path.

---

## 5. How We Will Ensure the Spec Describes the Actual Work

Phase 1's failure mode: specs described aspirational designs that diverged from what was implementable. Capabilities were marked done based on tests that verified what was built, not compliance with the spec.

### Rule 1: Spec requirements are testable assertions

Every requirement in this spec must map to at least one test. If a requirement can't be turned into a test, it's not a requirement — it's a wish. Before implementation begins, the test list is written.

Example:
- Spec says: "VFS serialization preserves all state."
- Test: `fn checkpoint_roundtrip_preserves_tree()` — populate VFS with files, dirs, symlinks, permissions → serialize → deserialize → assert bitwise equality.
- Test: `fn checkpoint_roundtrip_preserves_shell_state()` — set env vars and cwd → serialize → deserialize → assert equality.

### Rule 2: Proof-of-concept before spec requirement

Phase 1 wrote "use mempack" and "use WASI mem_fs" as requirements before testing them. Phase 2 will not.

Before any spec requirement depends on a library capability:
1. Build a throwaway PoC that demonstrates the capability in our stack.
2. Record the PoC result in the spec as "Validated" or "Failed — using alternative."
3. Only then write the requirement.

Applies to: VFS serialization format, Copilot multi-session support, GitHub API review posting, TUI library suitability.

### Rule 3: Capabilities track spec requirements, not just code

Each capability item will include:
- A "Requirements" section listing spec requirements it covers (by number/name).
- An "Acceptance Tests" section listing tests that verify each requirement.
- A "Spec Compliance" checklist verified before marking the capability done.

No capability is marked done until every listed requirement has a passing test.

### Rule 4: Specs are updated on architectural decisions

When an implementation decision changes the design (e.g., "we can't use X, we'll use Y instead"), the spec is updated in the same PR. No "Resolved Questions" section that contradicts the "Requirements" section. One source of truth.

### Rule 5: Weekly spec review

Once per week, read the spec against the current implementation. File drift as issues. Fix immediately or track explicitly. The Phase 1 ACP contradiction survived for weeks because nobody re-read the spec after writing it.

---

## Capability Breakdown (Preliminary)

| ID | Name | Crate | Depends On | Description |
|----|------|-------|------------|-------------|
| P2-00 | VFS serialization | devdev-vfs | — | `serialize()`/`deserialize()` for checkpoint |
| P2-01 | Multi-repo VFS | devdev-vfs | — | Mount multiple repos under `/repos/<owner>/<name>/` |
| P2-02 | Daemon lifecycle | devdev-daemon (new) | P2-00 | `devdev up`, `devdev down`, IPC, checkpoint save/restore |
| P2-03 | Chat TUI + headless mode | devdev-tui (new) | P2-02 | TUI chat interface + NDJSON headless attach mode |
| P2-04 | Task manager + approval gate | devdev-tasks (new) | P2-02 | Task trait, registry, scheduler, persistence, ask/auto-approve/dry-run |
| P2-05 | GitHub adapter | devdev-integrations (new) | — | PR fetch, diff, comment, review posting |
| P2-06 | Session router | devdev-daemon | P2-04 | Map tasks → ACP sessions, crash recovery |
| P2-07 | MonitorPR task | devdev-tasks | P2-04, P2-05, P2-06 | First task: monitor a PR, review, draft comments |
| P2-08 | Engine cleanup | devdev-wasm, devdev-git | — | sed shim, git flag gaps, spec reconciliation |
| P2-09 | E2E PR shepherding | tests/ | P2-07 | Full flow: interactive (TUI), headless (NDJSON), and one-shot scenarios |

**Build order:** P2-00 → P2-01 → P2-08 (can parallel) → P2-02 → P2-05 (can parallel with P2-03) → P2-04 → P2-06 → P2-03 → P2-07 → P2-09.

---

## Open Questions (To Be Resolved Before Implementation)

| # | Question | Options | How We'll Resolve |
|---|----------|---------|-------------------|
| 1 | IPC mechanism for CLI ↔ daemon | Unix domain socket (Linux/Mac) + named pipe (Windows) vs. localhost TCP | Build PoC on all three platforms. Pick simplest cross-platform option. |
| 2 | Copilot CLI multi-session | Does one `copilot --acp` subprocess support multiple `session/new` calls concurrently? | Test with live Copilot CLI. If not, manage a subprocess pool. |
| 3 | Checkpoint format | bincode (fast, Rust-only) vs. msgpack (cross-language) vs. SQLite (queryable) | bincode unless we need cross-language access. PoC serialize a 500MB VFS, measure speed. |
| 4 | GitHub API auth scope | Does `GH_TOKEN` with Copilot scope also cover repo/PR API calls? | Test. If not, document required scopes. |
| 5 | TUI library | ratatui vs. cursive vs. crossterm raw | Build hello-world TUI with ratatui. If it handles our layout, use it. |
| 6 | VFS mount semantics | Hard mounts (path prefix) vs. overlay (layered) | Start with path prefix (`/repos/owner/name/`). Overlay is over-engineering for now. |
| 7 | Task polling interval | Fixed (30s) vs. adaptive (back off when quiet) vs. event-driven (webhooks) | Fixed for Phase 2. Adaptive is optimization. Webhooks are Phase 3. |
