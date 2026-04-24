# DevDev

**A portable, daemonized agent for the developer's brain.**

DevDev silently monitors your workflows — PRs, tickets, commits — and enforces your personal technical standards using a headless AI agent sandbox. It intervenes only when something violates your preferences, and always privately, always with your approval.

## How it works

1. **You describe your vibes.** DevDev interviews you and records your technical preferences as plain Markdown files. No YAML schemas, no config DSLs — just natural language documents.

2. **It watches silently.** A background daemon polls for state changes (new PRs, updated tickets). An idempotency ledger ensures it never nags about the same thing twice.

3. **It evaluates in a sandbox.** When something new comes in, DevDev loads the relevant code into a fully virtualized workspace — a pure in-memory filesystem with its own shell and toolchain. An AI agent analyzes the code against your preferences without touching your real filesystem.

4. **It asks before acting.** If a violation is found, you get a private notification with a drafted response (e.g., a PR comment). One click to approve, or ignore it. No surprise public comments. No team-wide mandates. (Unless you pass `--rude`.)

## The Sandbox

The core of DevDev is a portable virtual workspace — an in-memory filesystem surfaced as a real OS mount so the agent's native tools work unchanged:

- **In-memory filesystem, real-OS surface** — a bounded, inode-centric in-memory `Fs` is mounted at a host path via FUSE (Linux) or WinFSP (Windows). All file state lives in DevDev's memory; the kernel just presents it.
- **Native host tools** — because the workspace is a real mount, the agent runs the host's own `grep`, `find`, `cat`, `ls`, `sed`, `git`, etc. under a curated PTY environment. No re-implementations, no WASM shims.
- **Copilot CLI via ACP** — the AI agent (Copilot CLI) is spawned as a subprocess using the Agent Communication Protocol — a structured JSON-based RPC over stdio. Prod invocation is `copilot --acp --allow-all-tools`: the CLI runs its tool bundle directly against the mount and DevDev observes work via session updates. DevDev-specific tools (task queries, preference lookups) are surfaced via an injected MCP server; see [capability 28](docs/internals/capabilities/28-mcp-tool-injection.md).

Nothing outside the mount is visible to the agent. When the workspace is dropped, its memory goes with it — the host filesystem is never touched.

> **Architecture note.** Phases 1–2 built a pure in-memory sandbox with WASM-compiled coreutils, a bash-subset parser, and an in-memory libgit2. Phase 3 (2026-04-22) consolidated those four crates (`devdev-vfs`/`-wasm`/`-git`/`-shell`) into a single `devdev-workspace` crate that delegates tool execution to the host via a FUSE/WinFSP mount. The design specs under `spirit/spec-virtual-*.md` and `spirit/spec-{wasm,shell}-*.md` describe the deleted architecture and are marked historical.

## Project Structure

```
spirit/          — Architecture specs and design documents
  outline.md     — Experience specification (what users can do)
  spec-*.md      — Technical specifications (how it works)
```

Specs cover: virtual filesystem, WASM tool engine, shell parser, virtual git, and Copilot CLI integration (ACP).

## Status

**Phase 4 complete (2026-04-22)** — Windows + Linux parity.

- **Phase 1** (caps 00–14): sandbox engine — VFS, WASM tool registry, virtual git, shell parser/executor, ACP protocol + client + hooks, sandbox integration, test harness.
- **Phase 2** (P2-00 – P2-05, P2-08): persistent architecture — `devdev up/down/status` daemon, checkpoint save/restore, chat TUI/headless mode, task scheduler + approval gate, GitHub adapter, engine cleanup.
- **Phase 3**: consolidation — collapsed four in-memory engine crates (`devdev-vfs`/`-wasm`/`-git`/`-shell`) into a single kernel-mount `devdev-workspace` crate; everything now runs on a real OS via FUSE (Linux) or WinFSP (Windows). `devdev-acp` (agent-protocol layer) was not part of the sandbox engine and survived unchanged.
- **Phase 4**: Windows WinFSP driver (hand-rolled MIT FFI, no GPL wrapper) with drive-letter mounts, delay-loaded `winfsp-x64.dll`, coarse-guard dispatcher, and round-trip smoke tests.

390+ tests passing across the workspace; `cargo clippy --workspace --all-targets -- -D warnings` clean on both OSes.

Remaining: PR monitor task (P2-07) — infrastructure exists but `MonitorPrTask`'s review callback is a placeholder that returns an empty string; full E2E (P2-09); vibe-check (P5-01), scout-router (P5-02), idempotency-ledger (P2-10). The real ACP session backend is live (proven by gated `live_mcp` tests against Copilot CLI 1.0.34); cap 28 (MCP tool injection) is done.

See [capabilities/](capabilities/) for the work-item graph and [spirit/](spirit/) for architecture specs.
