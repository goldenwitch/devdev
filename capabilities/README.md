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
| `devdev-vfs` | 00, 01 |
| `devdev-wasm` | 03, 04 |
| `devdev-git` | 05, 06 |
| `devdev-shell` | 07, 08, 09 |
| `devdev-acp` | 10, 11, 12 |
| `devdev-cli` | 13, 14 |
| *(build infra)* | 02 |
