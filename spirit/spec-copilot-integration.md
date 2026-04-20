# Spec: Copilot Integration Layer (ACP)

**Status:** Draft — Updated with research findings (April 2026)
**Depends on:** Shell Parser (spec-shell-parser.md), WASM Tools (spec-wasm-tools.md), Virtual Git (spec-virtual-git.md)

---

## Purpose

Integrate with the GitHub Copilot CLI via the **Agent Communication Protocol (ACP)** — a structured, versioned RPC protocol. DevDev spawns the Copilot CLI as a subprocess in ACP mode, intercepts tool-use requests via the protocol's hook system, and routes them through the virtual execution engine. No PTY hacking, no terminal escape sequence parsing, no reverse engineering.

---

## Background: The Old Plan vs. Reality

The original design assumed we'd need to spoof a pseudo-terminal and reverse-engineer how `gh copilot` communicates tool calls. Research revealed that:

1. The old `gh-copilot` extension was **archived and deprecated** (Oct 2025).
2. The new **GitHub Copilot CLI** (GA, v1.0.26+) exposes a first-class programmatic interface: **ACP**.
3. ACP provides structured JSON-based RPC over stdio, with explicit hooks for tool-use interception — exactly what we need.

This eliminates the entire class of PTY-protocol-fragility risks.

---

## Requirements

### Copilot CLI Subprocess

- Spawn the Copilot CLI as a subprocess using `copilot --acp`.
- Communicate over **stdio** (stdin/stdout) using the ACP RPC protocol.
- No PTY required — ACP is a structured protocol, not a terminal session.
- Cross-platform: the Copilot CLI supports Linux, macOS, and Windows.

### ACP Protocol Integration

ACP exposes the following RPC methods relevant to DevDev:

**Session Management:**
- `session.create()` — create a new agent session
- `session.load()` — resume an existing session
- `session.list()` — list active sessions

**Tool Execution (the core):**
- `session.shell.exec(command)` — the agent requests a shell command. This is our interception point.
- `session.shell.kill(pid)` — the agent wants to kill a running command.
- `tools.list()` — discover available tools.

**Hooks (the interception mechanism):**
- `preToolUse` — fires **before** the CLI executes any tool. DevDev intercepts here, reroutes the command to the virtual engine, and returns the result. The CLI never touches the host OS.
- `postToolUse` — fires after tool execution. Useful for logging/auditing.
- `PermissionRequest` — fires when the agent wants to do something that requires approval. DevDev can auto-approve virtual operations.

**Output:**
- `--output-format json` produces JSONL (JSON Lines) output — structured, parseable, no escape sequences.
- Streaming is supported (token-by-token); can be disabled with `--stream off`.

### Tool Interception Flow

When the agent issues a tool-use command (e.g., `grep -r TODO src/`):

1. ACP fires the `preToolUse` hook with the command details (structured JSON).
2. DevDev receives the hook, extracts the command string.
3. DevDev routes the command to the **Shell Parser** → **WASM Tool Engine / Virtual Git** → **VFS**.
4. DevDev captures stdout, stderr, and exit code from virtual execution.
5. DevDev returns the result to the Copilot CLI through the ACP response.
6. The CLI continues reasoning with the tool output as if it ran normally.

The agent never executes anything on the host. DevDev is the sole execution backend.

### Permission Management

ACP provides fine-grained tool permission controls:
- `--available-tools X,Y,Z` — whitelist specific tools.
- `--excluded-tools A,B` — blacklist specific tools.
- `preToolUse` hook can programmatically deny, modify, or approve any tool call.
- `PermissionRequest` hook provides programmatic approval for sensitive actions.

DevDev should auto-approve all virtual tool operations (they're sandboxed — there's nothing to protect against) and **deny** any operations that would escape the sandbox (network calls, host filesystem access).

---

## Architecture

```
┌──────────────────────────────────────────────┐
│                  DevDev                       │
│                                              │
│  ┌──────────────────────────────────────┐    │
│  │        ACP Client                    │    │
│  │  (stdio RPC to Copilot CLI)          │    │
│  │                                      │    │
│  │  Hooks:                              │    │
│  │   preToolUse ──► Tool Interceptor    │    │
│  │   PermissionRequest ──► Auto-approve │    │
│  │   postToolUse ──► Audit Log          │    │
│  └────────────────────┬─────────────────┘    │
│                       │                      │
│            ┌──────────▼───────────┐          │
│            │   Shell Parser       │          │
│            │ (pipes, redirects,   │          │
│            │  globs, env vars)    │          │
│            └──────────┬───────────┘          │
│                       │                      │
│          ┌────────────┼────────────┐         │
│          ▼            ▼            ▼         │
│  ┌──────────┐  ┌──────────┐  ┌─────────┐   │
│  │WASM Tools│  │Virtual   │  │Builtins │   │
│  │(grep,cat)│  │Git       │  │(cd,pwd) │   │
│  └────┬─────┘  └────┬─────┘  └────┬────┘   │
│       └──────────────┼─────────────┘         │
│                      ▼                       │
│            ┌─────────────────────┐           │
│            │   In-Memory VFS     │           │
│            └─────────────────────┘           │
└──────────────────────────────────────────────┘
                       │
                  stdio (ACP)
                       │
              ┌────────▼────────┐
              │  copilot --acp  │
              │  (subprocess)   │
              └─────────────────┘
```

### Context Injection

DevDev sends evaluation context to the Copilot CLI through ACP session management:
- Create a session with `session.create()`.
- Inject the PR diff, preference file pointers, and task description as the initial prompt.
- The CLI's built-in context management (auto-compaction at 95% token limit) handles long sessions.

---

## Session Lifecycle

1. **Init:** Spawn `copilot --acp --output-format json`. Establish stdio RPC channel.
2. **Auth:** The CLI authenticates using the user's existing credentials. Supported methods:
   - **`GH_TOKEN` environment variable** (fine-grained PAT) — recommended for daemon mode.
   - **Device code flow (RFC 8628)** — automatic fallback for headless environments.
   - **Existing `gh auth` session** — works if the user has already authenticated.
3. **Prime:** Create a session via `session.create()`. Send evaluation context as the initial prompt.
4. **Loop:** The CLI reasons and issues tool calls → ACP `preToolUse` hook fires → DevDev executes virtually → returns result → CLI continues. Repeat until the CLI produces a final verdict.
5. **Collect:** Parse the CLI's final output from the JSONL stream.
6. **Teardown:** End the session. Kill the subprocess. Drop the VFS.

### Timeouts & Error Handling

- **Command timeout:** If virtual execution takes longer than 30 seconds, return a timeout error to the CLI via ACP.
- **CLI hang detection:** If the CLI produces no ACP messages for 60 seconds, assume it's stuck and terminate.
- **CLI crash:** If the subprocess exits unexpectedly, capture whatever JSONL output was produced and report the failure.
- **Token limit:** The CLI auto-compacts context at 95% token usage. DevDev should monitor for compaction events and log them.
- All timeouts should be configurable.

---

## Authentication in Daemon Mode

For unattended operation, DevDev needs to authenticate the Copilot CLI without human interaction:

| Method | Setup | Tradeoff |
|--------|-------|----------|
| **Fine-grained PAT via `GH_TOKEN`** | User creates a PAT with Copilot scope, sets env var | Simplest for daemons. Token must be rotated manually. |
| **Device code flow** | CLI prompts for one-time browser approval | Works headless but requires initial human setup. |
| **OAuth token from `gh auth`** | User runs `gh auth login` once on the machine | Relies on `gh` CLI state. May expire. |

Recommendation: Support all three. Default to `GH_TOKEN` if set, fall back to existing `gh auth` session, prompt for device code flow on first run.

**Important:** Classic PATs are NOT supported by the Copilot CLI — only fine-grained PATs work.

---

## CLI Modes Available

ACP is the primary integration mode, but the Copilot CLI offers others that may be useful:

| Mode | Command | Use Case for DevDev |
|------|---------|---------------------|
| **ACP** | `copilot --acp` | Primary: structured RPC control | 
| **Prompt** | `copilot -p "query" --output-format json` | Lightweight: single-turn evaluations |
| **Autopilot** | `copilot --autopilot` | Future: fully autonomous multi-step tasks |
| **Plan** | `copilot --plan` | Future: generate evaluation plan before executing |

For v1, ACP mode covers all requirements. Single-turn `--prompt` mode may be useful for the Scout (lightweight LLM) stage if it moves to Copilot.

---

## Design Notes

- The ACP subprocess is the **only** component that touches the host OS. Everything below the ACP client is pure virtual.
- The ACP client should be as thin as possible. Its job is protocol translation: parse JSON → extract command → delegate to shell parser → format result → respond via JSON.
- Logging/tracing through the ACP layer is critical for debugging. Every intercepted tool call, its virtual execution result, and the ACP messages exchanged should be loggable.
- The CLI supports **parallel tool execution** (multiple tool calls in a single turn). DevDev should handle these concurrently — each call routes to the shell parser independently.
- Streaming output is supported but optional. For daemon mode, batch output (`--stream off`) is simpler; for interactive debugging, streaming is more useful.

---

## Extensibility: MCP (Model Context Protocol)

The Copilot CLI supports **custom tool servers** via MCP (configured in `.mcp.json`). This is relevant for future DevDev extensions — for example, providing the agent with custom tools (a code quality scorer, a dependency analyzer) that execute inside the virtual workspace.

When DevDev needs to expose custom tools to the agent beyond coreutils and git, MCP is the integration point. The tool server runs inside DevDev (not externally) and operates on the VFS.

---

## Resolved Questions (from ACP Research, April 2026)

See `spirit/research-acp.md` for full protocol details.

1. **ACP protocol versioning:** ✅ Resolved. Protocol version is a `uint16`, currently **1**. Negotiated during `initialize`. Only bumped for breaking changes; non-breaking additions use capability flags.
2. **Hook response format:** ✅ Resolved. Two hook systems exist: file-based (`.github/hooks/*.json` — can only deny, not replace execution) and ACP client capabilities (`terminal/*`, `fs/*` methods — full execution control). DevDev uses the **client capabilities** approach, not file-based hooks.
3. **Concurrent tool calls:** ✅ Resolved. Messages are multiplexed on the single stdio NDJSON stream, correlated by `id`. Notifications are interleaved with request/response pairs. The client must handle this concurrently (see `11-acp-client` capability).
4. **Architecture:** ✅ Resolved. DevDev advertises `{ terminal: true, fs: { readTextFile: true, writeTextFile: true } }` during `initialize`. The agent sends `terminal/create` and `fs/*` requests which DevDev routes through the virtual engine. This is cleaner than `preToolUse` interception — DevDev becomes the execution backend transparently.

## Open Questions

1. **CLI version pinning:** Should DevDev bundle or pin to a specific Copilot CLI version? Recommendation: pin to tested version, document minimum.
2. **Terminal delegation verification:** Does Copilot CLI actually use `terminal/create` when the client advertises terminal capability, or does it still execute internally? Needs hands-on testing.
3. **Context window management:** How much context can we inject in the initial prompt? The CLI has auto-compaction at 95% token usage — does it handle large PR diffs gracefully?
---

## Integration Seam (Transport Split)

The evaluation orchestrator (cap 13) does **not** spawn `copilot`
directly. It takes a `Transport` value:

```rust
pub enum Transport {
    /// Production path: spawn `program` with `args`.
    /// `Transport::copilot()` returns `{ "copilot", ["--acp", "--stdio"] }`.
    SpawnProcess { program: String, args: Vec<String> },

    /// Test / embedding path: caller already has an NDJSON pipe.
    Connected {
        reader: Box<dyn AsyncRead + Send + Unpin>,
        writer: Box<dyn AsyncWrite + Send + Unpin>,
    },
}
```

`AcpClient` already exposes both entry points — `connect_process`
(subprocess) and `connect_transport<R, W>` (pre-connected pipes).
Splitting them at the orchestrator boundary has two direct
consequences:

- The acceptance suite for cap 13 drives the full pipeline over
  `tokio::io::duplex` with a scripted fake agent. No `copilot`
  binary, no network, no `GH_TOKEN`. Every orchestration edge
  case (auth failure, mid-turn disconnect, session timeout, tool
  order, verdict concatenation) becomes a fast, deterministic
  unit test.
- E2E tests that exercise the real CLI live in cap 14 behind
  `#[ignore]` + a `DEVDEV_E2E` env gate. They use
  `Transport::copilot()` unchanged.

The DevDev daemon and any future embeddings always use
`Transport::copilot()`. The `Connected` variant exists purely for
tests and future in-process embeddings (e.g. a Copilot extension
crate that speaks ACP without spawning a child). Nothing else in the
system needs to know transports exist.