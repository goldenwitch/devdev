# Postmortem: DevDev Phase 1

**Date:** April 19, 2026
**Scope:** Sandbox engine build (capabilities 00–14)
**Stats:** ~10,200 lines of Rust across 6 crates, 209 tests (191 active, 18 gated), 15 capability items completed.

---

## The Vision

Two things:

1. **A persistent, lightweight virtual unix environment for agents.** In 2026, every coding agent is RL-trained on unix tools — bash, grep, cat, git. The industry's current answer is one Docker container per developer. That's heavy, slow to start, expensive to run, and wasteful at idle. DevDev replaces the container with a portable, in-memory sandbox that starts in milliseconds, uses only RAM, and stays running as a daemon. It's the execution backend agents were trained to expect, without the infrastructure tax.

2. **Simple applications of that sandbox.** Give developers a way to holistically handle the remaining work that AI hasn't made easier yet — the stuff that still requires human judgment, taste, and context accumulated over time.

The outline is explicit: "A portable, **daemonized** agent for the developer's brain. DevDev silently **monitors workflows** and uses a headless Copilot CLI sandbox to enforce personal technical boundaries." The words "daemonized," "long running session," and "monitors workflows" (plural) are load-bearing.

---

## What We Actually Built

A single-shot evaluation CLI.

```
devdev eval --repo . --task "Review this code" --preferences .devdev/ --json
```

One repo in. One conversation. One verdict out. Everything destroyed on completion. The `evaluate()` function creates a MemFs, loads one repository, runs one ACP conversation with Copilot, collects a verdict, and tears down the entire sandbox.

The sandbox engine underneath is solid:

| Layer | Crate | What It Does |
|-------|-------|--------------|
| Foundation | `devdev-vfs` | In-memory filesystem (BTreeMap, POSIX ops, 2 GiB cap) |
| Tools | `devdev-wasm` | 13 WASM coreutils + 3 native tools (grep/find/diff) |
| Tools | `devdev-git` | 9 read-only git commands via libgit2 |
| Execution | `devdev-shell` | Bash-subset parser, pipeline engine, 7 builtins |
| Protocol | `devdev-acp` | JSON-RPC 2.0 ACP client, sandbox handler, thread-pinned shell worker |
| Orchestration | `devdev-cli` | `evaluate()` pipeline, CLI binary, fake agent for testing |

All of this works. An agent can be spawned, explore a repo via the virtual shell, and produce structured output. The test suite covers 209 scenarios. Clippy is clean.

---

## The Central Error

**We built a disposable sandbox when the whole point was a persistent one.**

The vision says: daemon, long-running, monitors workflows. The implementation says: `fn evaluate() -> EvalResult` — a function call with a beginning and an end. The VFS is created, populated, used once, and dropped. The shell session lives for one conversation. The ACP client connects, exchanges messages, and disconnects.

This isn't a subtle drift. The outline's §4 ("The Silent Watcher") describes a background process that polls for state changes, maintains an idempotency ledger, and intervenes asynchronously. What we built is a CLI that a human runs manually, waits for, and reads the output of.

The sandbox *engine* — VFS, shell, tools, git, ACP — doesn't have this problem. Nothing in those crates assumes a one-shot lifecycle. `MemFs` can live as long as you want. `ShellSession` is stateful across commands by design. `AcpClient` can manage multiple sessions. The wrong layer is `evaluate()` and the CLI wrapper around it. That's where the one-shot assumption was baked in.

**How this happened:** The capability plan (00–14) was organized bottom-up by technical component, not by product behavior. Each capability was a crate or a feature within a crate. "Build the VFS" → "build the WASM engine" → "build the shell" → "build the git layer" → "build the ACP client" → "wire them together." The wiring step (cap 13, "sandbox integration") asked "how do we connect these pieces?" and the easiest answer was a function that creates everything, runs a conversation, and cleans up. A function, not a service.

Nobody asked "what is the lifecycle of the sandbox?" because the capability plan didn't have a capability for lifecycle management. The daemon, the polling loop, the session persistence — these were in the outline but never made it into the capability breakdown. They were treated as "future work" that would sit on top of the sandbox, rather than as architectural constraints that should shape the sandbox's design.

---

## Secondary Drift: Temp Directories

### Git uses host disk

The spec called for `mempack` — libgit2's in-memory object database. The implementation writes `.git/` and the working tree to a temp directory and calls `Repository::open()`. Every evaluation touches host disk.

### WASM tools use host disk

The spec assumed Wasmtime had a built-in `mem_fs` for WASI preopens. It doesn't. The implementation materializes the entire VFS to a temp directory per WASM invocation, runs the module, and syncs changes back. O(VFS size) in disk I/O per tool call.

### This matters less than it seems

The agent can't tell the difference. The sandbox's correctness guarantee — the agent can't affect the host — holds regardless. Temp directories are created, used, and cleaned up deterministically. For the one-shot model this is fine. For a persistent daemon, the repeated disk I/O becomes a performance concern, but it's an optimization problem, not an architecture problem.

**Where the specs went wrong:** Both specs presented theoretical library capabilities as validated implementation plans. "libgit2 supports mempack" is true. "We can use mempack to load a repo from a VFS snapshot" was never tested. The spec should have distinguished "researched" from "validated."

---

## Secondary Drift: Missing Tools

### sed and awk don't exist

The outline lists sed as a core utility. The WASM spec resolves that `sd` (Rust) should replace sed and the `awk` crate should replace awk. Neither was built. The shim table is empty. An agent running `sed 's/foo/bar/' file.txt` gets exit 127.

**How this happened:** The spec treated "resolving which library to use" as equivalent to "completing the work." The decision was made but never tracked as a build task. The capability files focused on uutils coreutils, and sed/awk required separate compilation from different source projects.

### git flag gaps

P0 spec requires `--since`, `--follow`, and path filtering (`git diff -- src/`). None implemented. P1 requires `--graph`. Not implemented. Capabilities were marked done based on tests that verified what was built, not coverage against the spec's requirement lists.

---

## The ACP Spec Contradicts Itself

The Requirements section describes `preToolUse` hooks and `session.shell.exec()` as the interception mechanism. The Resolved Questions section (written later) says the opposite: use client capabilities (`terminal/*`, `fs/*`), not hooks.

The implementation follows the resolved answer — correct. But the Requirements section was never reconciled. The spec is a living document that was updated incrementally without propagating decisions backward.

---

## Where the Spirit Was Poorly Defined

### 1. The Scout is a sketch

"Lightweight LLM evaluates an incoming event and generates file pointers." Which LLM? Local or API? What's the prompt? What's the output schema? What's the latency budget? This is a major architectural decision (cost, latency, infrastructure dependencies) described in two sentences.

### 2. Daemon architecture is a bullet point

"Background polling loop checks for state changes." Polling what — GitHub API? Webhooks? Local git? What's the event schema? Polling interval? Rate limits? Ledger storage format? Process restart resilience? Concurrent evaluation handling? Backpressure?

### 3. The UX is one sentence

"Drafted PR comment requiring 1-click approval." This is the entire user-facing surface of the product. Where does the notification appear? How does the user review it? Can they edit it? What happens on ignore?

### 4. No session lifecycle spec

The outline says "daemonized" and "long running session." The implementation assumes one evaluation = one lifetime. The spec never defines what "session" means at the product level. Is a session one evaluation? One day? One repo? The developer's entire workflow? Can the agent accumulate context over time? Does the VFS persist between tasks?

### 5. Spec said "pure in-memory" without validating the tech

The VFS spec requires "built-in in-memory filesystem implementations" from the WASM runtime. The git spec requires `mempack` and `Repository::from_odb()`. Neither was validated against Wasmtime or git2-rs in our configuration before being written as requirements. The implementation correctly worked around both, but the specs set up expectations they couldn't deliver on.

---

## Lessons

### 1. Lifecycle is an architectural constraint, not a feature to add later

The difference between "sandbox that lives for one function call" and "sandbox that lives for hours as a daemon" shapes every design decision: resource management, state persistence, error recovery, connection handling, memory growth, garbage collection. We designed the engine without answering this question, and the default (one-shot) won by inertia.

### 2. Product behavior should drive the capability plan

The capability plan was organized by technical component: VFS, WASM, git, shell, ACP, CLI. The product behaviors — daemon lifecycle, session persistence, event polling, preference routing, notification UX — were absent from the plan entirely. They were in the outline but never decomposed into capabilities. If it's not in the plan, it doesn't get built.

### 3. Research findings are not implementation plans

"libgit2 supports mempack" and "Wasmtime supports WASI" are both true and both insufficient. The gap between "the library advertises this" and "this works for our specific use case" was consistently underestimated. Going forward: build a minimal proof-of-concept before writing a spec requirement around a library capability.

### 4. Resolved questions must propagate

When a spec's Resolved Questions section changes the architecture, the rest of the spec must be rewritten to match. Otherwise the spec is a trap — the implementer reads the Requirements section, builds the wrong thing, and only discovers the correction buried in a different section.

### 5. Acceptance tests should verify spec compliance, not just implementation correctness

Tests were written to verify what was built. Missing features (sed, --since, --follow) were never caught because no test asserted their presence. Derive acceptance criteria from the spec's requirement lists before implementation begins, not after.

### 6. The sandbox engine is reusable

Despite the lifecycle error, the core crates are sound. MemFs, ShellSession, WasmToolRegistry, VirtualGit, AcpClient, SandboxHandler — none of these assume a one-shot model. The `evaluate()` function and the CLI wrapper are the only code that needs to be rearchitected. The engine is the hard part, and it's done.

---

## Current State

**What works:** A developer can run `devdev eval --repo . --task "..." --preferences .devdev/` and get a structured verdict from a Copilot agent that explored the codebase in a sandboxed virtual environment.

**What's wrong at the architecture level:** The sandbox is disposable (one-shot function) when it should be persistent (long-running daemon). The orchestration layer assumes one repo, one task, one lifetime.

**What's missing from the tools:** sed/awk, git flags (--since, --follow, path filtering, --graph), working-tree-aware git status.

**What's missing from the product:** Everything above the sandbox — daemon, session persistence, event routing, Scout, preference creation, notification UX, PR integration.

**What's missing from the specs:** Rigorous design for session lifecycle, daemon architecture, Scout, and UX surface.

---

*Values: Empiricism. Brevity. Wit.*

*The empiricism was applied to the infrastructure. The brevity was applied to the product design. The wit was in the outline all along — we just stopped reading it.*
