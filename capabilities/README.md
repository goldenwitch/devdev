# Capabilities

Each file in this directory is a **buildable work item** — a self-contained capability that can be implemented, tested, and verified independently. Capabilities compose leaf-to-root: build the leaves first, then wire them together.

## Front Matter Schema

```yaml
---
id: string              # unique identifier, matches filename
title: string           # human-readable name
status: not-started     # not-started | in-progress | complete
type: leaf | composition | build
phase: 1                # implementation phase (1-5)
crate: devdev-vfs       # target Rust crate (omit for build tasks)
priority: P0            # P0 (launch) | P1 (needed) | P2 (nice-to-have)
depends-on: []          # list of capability IDs that must be complete first
effort: M               # T-shirt size: S | M | L | XL
---
```

## Dependency Graph

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

## Phase 2 — Persistent Sandbox

```
LEAVES (no dependencies, can parallel)
  15-vfs-serialization ──────────┐
  16-multi-repo-vfs              │
  20-github-adapter              │
  23-engine-cleanup              │
                                 │
DAEMON                           │
  17-daemon-lifecycle ◄──────────┘  (needs 15 for checkpoint)
        │
        ├───────────────────────────────────┐
        │                                   │
  18-chat-tui-headless              19-task-manager
                                        │
                                  21-session-router
                                        │
                              22-monitor-pr-task ◄── 20-github-adapter
                                        │
                              24-e2e-pr-shepherding
```

Build order: 15 + 16 + 20 + 23 (parallel) → 17 → 18 + 19 (parallel) → 21 → 22 → 24

## Parallel Work Streams

Within each phase, capabilities without mutual dependencies can be built concurrently:

| Phase | Parallel Streams |
|-------|-----------------|
| **1** | `00-vfs-core` (single track) |
| **2** | Stream A: `01-vfs-loader` / Stream B: `02-wasm-toolchain` + `03-wasm-engine` + `04-tool-registry` / Stream C: `05-virtual-git-core` + `06-virtual-git-commands` / Stream D: `07-shell-parser` + `08-shell-builtins` |
| **3** | `09-shell-executor` (joins all Phase 2 streams) |
| **4** | `10-acp-protocol` can start during Phase 2. `11-acp-client` once 10 is done. `12-acp-hooks` + `13-sandbox-integration` after shell executor. |
| **5** | `14-test-harness` (single track) |

## Crate Map

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
