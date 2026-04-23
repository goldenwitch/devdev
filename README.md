# DevDev

**A portable, daemonized agent for the developer's brain.**

DevDev silently monitors your workflows — PRs, tickets, commits — and enforces your personal technical standards using a headless AI agent sandbox. It intervenes only when something violates your preferences, and always privately, always with your approval.

## How it works

1. **You describe your vibes.** DevDev interviews you and records your technical preferences as plain Markdown files. No YAML schemas, no config DSLs — just natural language documents.

2. **It watches silently.** A background daemon polls for state changes (new PRs, updated tickets). An idempotency ledger ensures it never nags about the same thing twice.

3. **It evaluates in a sandbox.** When something new comes in, DevDev loads the relevant code into a fully virtualized workspace — a pure in-memory filesystem with its own shell and toolchain. An AI agent analyzes the code against your preferences without touching your real filesystem.

4. **It asks before acting.** If a violation is found, you get a private notification with a drafted response (e.g., a PR comment). One click to approve, or ignore it. No surprise public comments. No team-wide mandates. (Unless you pass `--rude`.)

## The Sandbox

The core of DevDev is a fully portable virtual execution environment:

- **In-memory filesystem** — the target repo is loaded into memory. The agent reads, writes, and navigates files without any host disk I/O.
- **WASM-compiled coreutils** — standard Unix tools (`grep`, `find`, `cat`, `ls`, `sed`, etc.) are compiled to WebAssembly and execute against the virtual filesystem. The agent uses the same bash-like commands it was trained on.
- **Virtual git** — git operations (`diff`, `log`, `status`, `blame`) run natively against the in-memory repo using a git library, not a real git binary.
- **Shell parser** — pipes, redirects, globs, and environment variables work as expected. The agent doesn't know it's not in a real shell.
- **Copilot CLI via ACP** — the AI agent (Copilot CLI) is spawned as a subprocess using the Agent Communication Protocol — a structured JSON-based RPC interface. Tool-use commands are intercepted via protocol hooks and routed through the virtual engine. No PTY hacking, no terminal parsing.

Nothing escapes the sandbox. When the evaluation is done, the entire virtual workspace is dropped. Zero cleanup.

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

Remaining: session router (P2-06), PR monitor task (P2-07), full E2E (P2-09), real ACP session backend (stubbed as `NOT_WIRED`).

See [capabilities/](capabilities/) for the work-item graph and [spirit/](spirit/) for architecture specs.
