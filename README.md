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

Sandbox core implemented. 10 of 15 capabilities complete (VFS, WASM engine + tool registry, virtual git, shell parser + builtins, ACP protocol) with 170 passing tests, clippy-clean on `-D warnings`. Remaining: shell executor, ACP client + hooks, sandbox integration, and the end-to-end test harness.

See [capabilities/](capabilities/) for the work-item graph and [spirit/](spirit/) for architecture specs.
