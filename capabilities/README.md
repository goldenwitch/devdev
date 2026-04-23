# Capabilities

Each file in this directory is a **buildable work item** — a self-contained capability that can be implemented, tested, and verified independently. Capabilities compose leaf-to-root: build the leaves first, then wire them together.

## Front Matter Schema

```yaml
---
id: string              # unique identifier, matches filename
title: string           # human-readable name
status: not-started     # not-started | in-progress | done | partial | superseded | obsolete
type: leaf | composition | build
phase: 1                # implementation phase (1-5)
crate: devdev-vfs       # target Rust crate (omit for build tasks)
priority: P0            # P0 (launch) | P1 (needed) | P2 (nice-to-have)
depends-on: []          # list of capability IDs that must be complete first
effort: M               # T-shirt size: S | M | L | XL
---
```

## Dependency Graph

> **Note (2026-04-22, post-Phase-3):** The graphs below are preserved for historical context. Phase 3 collapsed the original sandbox-engine crates (`devdev-vfs`, `devdev-wasm`, `devdev-git`, `devdev-shell`) into a single kernel-mount crate `devdev-workspace`, and Phase 4 added the WinFSP driver underneath it. `devdev-acp` survived consolidation unchanged — it's the agent-protocol layer, not part of the sandbox engine. See [Crate Map (current)](#crate-map-current) below for the live layout.

### Historical: Phase 1 — Sandbox Engine (Pre-Phase-3)

```
PHASE 1 ─ Foundation
  00-vfs-core ◄─────────────────────────────────────────────┐
      │                                                      │
PHASE 2 ─ Engines (parallel)                                │
      ├── 01-vfs-loader                                     │
      ├── 02-wasm-toolchain (build)                         │
      ├── 03-wasm-engine ─────┐                             │
      │                       ├── 04-tool-registry          │
      ├── 05-virtual-git-core ┤                             │
      │                       ├── 06-virtual-git-commands   │
      ├── 07-shell-parser ────┤   (pure, no deps)          │
      ├── 08-shell-builtins ──┤                             │
      │                       │                             │
PHASE 3 ─ Shell Composition   │                             │
      │                       ▼                             │
      │               09-shell-executor ◄───────────────────┤
      │                       │                             │
PHASE 4 ─ ACP Integration     │                             │
      ├── 10-acp-protocol ────┤   (pure, no deps)          │
      │         │             │                             │
      │         ▼             │                             │
      │   11-acp-client ──────┤                             │
      │                       ▼                             │
      │               12-acp-hooks ◄────────────────────────┘
      │                       │
      │                       ▼
      │               13-sandbox-integration ◄── 01-vfs-loader
      │
PHASE 5 ─ Validation
      └───────────── 14-test-harness
```

All caps 00–14 are `done`. Cap 00 originally targeted `devdev-vfs`; its code now lives in `devdev-workspace` (see capability front matter for the audit trail). Caps 10–12 still live in `devdev-acp`, which survived Phase 3.

## Phase 2 — Persistent Sandbox

```
LEAVES (no dependencies, can parallel)
  15-vfs-serialization ──────────┐  (superseded by Phase 3 — see file)
  16-multi-repo-vfs              │  (superseded by Phase 3 — see file)
  20-github-adapter              │
  23-engine-cleanup              │  (partial — see Postmortem epilogue)
                                 │
DAEMON                           │
  17-daemon-lifecycle ◄──────────┘
        │
        ├───────────────────────────────────┐
        │                                   │
  18-chat-tui-headless              19-task-manager
                                        │
                                  21-session-router  ◄── pending (next)
                                        │
                              22-monitor-pr-task ◄── 20-github-adapter (pending)
                                        │
                              24-e2e-pr-shepherding (pending)
```

Build order: 15 + 16 + 20 + 23 (parallel, **done**) → 17 (**done**) → 18 + 19 (parallel, **done**) → 21 → 22 → 24.

Remaining Phase 2 work: P2-06 session-router, P2-07 monitor-pr-task, P2-09 E2E, P2-10 idempotency-ledger.

## Phase 5 — Outline Pillars (Vibe Check + Scout + MCP)

Surfaced by the 2026-04-22 alignment reviews against `spirit/outline.md` and the P2-06 PoC. Cap 28 was added post-PoC when the `mcpCapabilities` advertisement in Copilot's `initialize` response revealed the real tool-injection surface.

```
P5-01 vibe-check ◄── P2-06 session-router
       │
       ▼
P5-02 scout-router ◄── P2-07 monitor-pr-task

P5-03 mcp-tool-injection ◄── P2-06 session-router
```

| ID | Capability | Outline ref |
|----|------------|-------------|
| P5-01 | [vibe-check](25-vibe-check.md) | §1 — Markdown preferences via scribe interview |
| P5-02 | [scout-router](26-scout-router.md) | §2 — lightweight LLM picks `.devdev/*.md` for the Heavy |
| P2-10 | [idempotency-ledger](27-idempotency-ledger.md) | §4 — never re-evaluate the same commit/ticket state twice |
| P5-03 | [mcp-tool-injection](28-mcp-tool-injection.md) | post-PoC — DevDev-specific tools exposed via MCP (the `--allow-all-tools` prod path bypasses ACP hooks; MCP is the replacement surface) |

## Parallel Work Streams

Within each phase, capabilities without mutual dependencies can be built concurrently:

| Phase | Parallel Streams |
|-------|-----------------|
| **1** | `00-vfs-core` (single track) |
| **2** | Stream A: `01-vfs-loader` / Stream B: `02-wasm-toolchain` + `03-wasm-engine` + `04-tool-registry` / Stream C: `05-virtual-git-core` + `06-virtual-git-commands` / Stream D: `07-shell-parser` + `08-shell-builtins` |
| **3** | `09-shell-executor` (joins all Phase 2 streams) |
| **4** | `10-acp-protocol` can start during Phase 2. `11-acp-client` once 10 is done. `12-acp-hooks` + `13-sandbox-integration` after shell executor. |
| **5** | `14-test-harness` (single track) |

## Crate Map (Current)

Live layout as of Phase 4 (2026-04-22):

| Crate | Capabilities | Notes |
|-------|--------------|-------|
| `devdev-workspace` | 00, 01, 03, 04, 05, 06, 07, 08, 09, 13, 15, 16, 23 | Phase 3 consolidation: real-FS mount via FUSE/WinFSP, replacing the per-engine crates listed in the historical map below. Phase 4 added the WinFSP driver. |
| `devdev-acp` | 10, 11, 12 | Survived Phase 3 unchanged \u2014 it's the agent-protocol layer (NDJSON, client, hooks), not part of the sandbox engine. |
| `devdev-cli` | 13, 14 | `devdev` binary, `evaluate()` pipeline, `up`/`down`/`status`/`send`/`task` subcommands. |
| `devdev-daemon` | 17, 21 (pending), 27 (pending), 28 (P5-03 pending) | Daemon lifecycle, IPC server, session router (P2-06 pending), idempotency ledger (P2-10 pending), MCP tool server (P5-03 pending). |
| `devdev-tui` | 18 | TUI + headless NDJSON. |
| `devdev-tasks` | 19, 22 (pending), 26 (P5-02 pending) | Task trait, scheduler, approval gate, MonitorPR (P2-07 pending), Scout router (P5-02 pending). |
| `devdev-integrations` | 20 | GitHub adapter. |
| `devdev-scenarios` | \u2014 | Test fixtures / scenarios shared across crates. |
| `tests/` (workspace root) | 24 (pending) | E2E shepherding harness. |

### Crate Map (Historical, Pre-Phase-3)

| Crate | Capabilities |
|-------|-------------|
| `devdev-vfs` | 00, 01, 15, 16 |
| `devdev-wasm` | 03, 04, 23 |
| `devdev-git` | 05, 06, 23 |
| `devdev-shell` | 07, 08, 09 |
| `devdev-acp` | 10, 11, 12 |
| `devdev-cli` | 13, 14 |
| `devdev-daemon` *(new)* | 17, 21 |
| `devdev-tui` *(new)* | 18 |
| `devdev-tasks` *(new)* | 19, 22 |
| `devdev-integrations` *(new)* | 20 |
| `tests/` | 24 |
| *(build infra)* | 02 |
