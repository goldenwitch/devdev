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
- `tools.list()` — discover available tools.

**Client Capabilities (the interception mechanism):**
DevDev advertises `{ terminal: true, fs: { readTextFile: true, writeTextFile: true } }` during `initialize`. The agent sends `terminal/create` and `fs/*` requests which DevDev routes through the virtual engine. This is cleaner than `preToolUse` hook interception — DevDev becomes the execution backend transparently.

- `terminal/create`, `terminal/kill` — the agent requests terminal operations. DevDev routes them to the Shell Parser → WASM/Virtual Git → VFS.
- `fs/readTextFile`, `fs/writeTextFile` — file operations routed through VFS.

**Output:**
- `--output-format json` produces JSONL (JSON Lines) output — structured, parseable, no escape sequences.
- Streaming is supported (token-by-token); can be disabled with `--stream off`.

### Tool Interception Flow

When the agent issues a tool-use command (e.g., `grep -r TODO src/`):

1. The agent sends a `terminal/create` request via ACP with the command details.
2. DevDev receives the request and extracts the command string.
3. DevDev routes the command to the **Shell Parser** → **WASM Tool Engine / Virtual Git** → **VFS**.
4. DevDev captures stdout, stderr, and exit code from virtual execution.
5. DevDev returns the result to the Copilot CLI through the ACP response.
6. The CLI continues reasoning with the tool output as if it ran normally.

The agent never executes anything on the host. DevDev is the sole execution backend.

### Permission Management

ACP provides fine-grained tool permission controls:
- `--available-tools X,Y,Z` — whitelist specific tools.
- `--excluded-tools A,B` — blacklist specific tools.
- Client capabilities allow DevDev to control which operations are available at the protocol level.

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
│  │  Client Capabilities:                │    │
│  │   terminal/* ──► Tool Interceptor    │    │
│  │   fs/*       ──► VFS Operations      │    │
│  │                                      │    │
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

1. **Init:** Spawn `copilot --acp --allow-all-tools`. Establish stdio NDJSON RPC channel. (The `--allow-all-tools` flag skips Copilot's interactive permission prompts and is required for non-interactive daemon use; `--output-format json` from earlier drafts of this doc is not the right flag — `--acp` implies NDJSON-over-stdio.)
2. **Auth:** The CLI authenticates using the user's existing credentials. Supported methods, in practical preference order:
   - **Existing `gh auth` session** — the Copilot CLI reuses `gh auth login` credentials transparently. If the user is already logged in to a Copilot-enabled account, no further setup is needed (validated 2026-04-22 via the P2-06 PoC).
   - **`GH_TOKEN` / `GITHUB_TOKEN` environment variable** — either a fine-grained PAT with Copilot scope, or a gh-CLI OAuth token (e.g. `GH_TOKEN=$(gh auth token)`).
   - **Device code flow (RFC 8628)** — interactive fallback for first-time setup on a fresh machine.
3. **Prime:** Create a session via `session/new`. Send evaluation context as the initial prompt.
4. **Loop:** On `--allow-all-tools`, Copilot runs its own tools (shell, fs, web) directly against the mounted workspace; DevDev observes progress via `session/update` notifications and surfaces text chunks as responses. (Under a hypothetical `--strict-sandbox` mode — no `--allow-all-tools` — tool calls instead route back via ACP `terminal/*` + `fs/*` client capabilities; see [capability 12](capabilities/12-acp-hooks.md).)
5. **Collect:** Assemble `agent_message_chunk` text across the turn; terminate on `stopReason: endTurn`.
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
| **OAuth via `gh auth`** | User runs `gh auth login` once; the Copilot CLI reuses the token transparently. Alternatively export `GH_TOKEN=$(gh auth token)` for scripts. | Lowest friction. Validated 2026-04-22. May expire per gh-CLI's refresh cadence. |
| **Fine-grained PAT via `GH_TOKEN`** | User creates a PAT with Copilot scope, sets env var. | Deterministic for daemons and CI. Token must be rotated manually. |
| **Device code flow** | CLI prompts for one-time browser approval. | Works headless but requires initial human setup. |

Recommendation: default to existing `gh auth` session, fall back to `GH_TOKEN` if set, prompt for device code flow on a fresh machine.

**Important:** Classic PATs are NOT supported by the Copilot CLI — only fine-grained PATs work. gh-CLI OAuth tokens (`gho_*` prefix) *are* accepted.

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

1. **CLI version pinning:** Should DevDev bundle or pin to a specific Copilot CLI version? Recommendation: pin to tested version, document minimum. (PoC validated against 1.0.34.)
2. ~~**Terminal delegation verification:** Does Copilot CLI actually use `terminal/create` when the client advertises terminal capability, or does it still execute internally?~~ ✅ **Resolved (2026-04-22, P2-06 PoC):** When launched with `--allow-all-tools`, Copilot runs its own internal tool bundle directly against the mounted workspace and does *not* route through ACP `terminal/create`. The ACP client capabilities path (see [capability 12](capabilities/12-acp-hooks.md)) only engages under a `--strict-sandbox` profile that is not currently used. DevDev-specific tools should be exposed via MCP ([capability 28](capabilities/28-mcp-tool-injection.md)) instead.
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