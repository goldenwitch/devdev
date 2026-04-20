---
id: sandbox-integration
title: "Sandbox Integration & Evaluation Orchestrator"
status: not-started
type: composition
phase: 4
crate: devdev-cli
priority: P0
depends-on: [acp-hooks, vfs-loader, acp-client]
effort: M
---

# 13 — Sandbox Integration & Evaluation Orchestrator

Wire every crate into a single `evaluate()` function: load a repo into
the VFS, build the virtual shell and git, open an ACP session, send a
prompt, and collect the verdict + tool-call log. This is the core of
DevDev and the first capability where the user actually hears back from
an agent.

## Scope

**In:**
- Evaluation lifecycle: VFS → tool registry → virtual git → shell →
  sandbox handler → ACP client → prompt → teardown.
- A `Transport` seam so `evaluate()` drives either a real
  `copilot --acp --stdio` subprocess or a test-controlled
  `AsyncRead`/`AsyncWrite` pair.
- Prompt formatting: task + preferences + optional diff + optional
  focus paths into one plain string.
- Verdict collection: concatenation of every `agent_message_chunk`
  text received during the turn.
- Tool-call log: every successful `terminal/create` with its command,
  exit code, and duration.
- Resource cleanup: drop VFS, kill subprocess, release ACP tasks on
  completion or error.

**Out:**
- The daemon polling loop.
- Preference-file management / vibe-check authoring.
- Notification / approval UX.
- The Scout router.
- Real-agent end-to-end tests (owned by capability 14).

## Preconditions

This capability composes pieces already shipped:

- `devdev_vfs::MemFs` + `devdev_vfs::load_repo` (cap 00, 01).
- `devdev_wasm::WasmToolRegistry` implementing `ToolEngine` (cap 04).
- `devdev_git::VirtualRepo` + `devdev_git::VirtualGitRepo`
  (cap 05, 06). `dyn VirtualGit` is intentionally `!Send`.
- `devdev_shell::ShellSession` (cap 09).
- `devdev_acp::SandboxHandler`, `AcpClient`, `TraceLogger` (caps
  10–12). `AcpClient::initialize` already advertises
  `{ terminal: true, fs: { readTextFile: true, writeTextFile: true } }`.

One enabling edit in `devdev-acp` is a prerequisite:
`TraceEvent::TerminalCreated` gains `duration_ms: u64`, populated by
`SandboxHandler::on_terminal_create` around its `tokio::time::timeout`.
That is the single source of truth for tool-call duration.

## Interface

```rust
/// Tunable evaluation knobs. `session_timeout` is the outer wall-clock
/// budget; `cli_hang_timeout` is the idle-silence budget enforced by
/// `AcpClient`. If the former is shorter, it wins.
pub struct EvalConfig {
    pub workspace_limit: u64,        // VFS memory cap (default: 2 GiB)
    pub command_timeout: Duration,   // per-command (default: 30 s)
    pub session_timeout: Duration,   // whole evaluation (default: 10 min)
    pub cli_hang_timeout: Duration,  // idle silence before kill (default: 60 s)
    pub include_git: bool,           // load .git into VFS (default: true)
}

/// How `evaluate()` connects to the agent.
pub enum Transport {
    /// Production path: spawn the given program with the given args.
    SpawnProcess { program: String, args: Vec<String> },
    /// Test / embedding path: caller already has NDJSON pipes.
    Connected {
        reader: Box<dyn AsyncRead + Send + Unpin>,
        writer: Box<dyn AsyncWrite + Send + Unpin>,
    },
}

impl Transport {
    /// Canonical production transport: `copilot --acp --stdio`.
    pub fn copilot() -> Self;
}

pub struct EvalContext {
    pub task: String,
    pub diff: Option<String>,
    pub preferences: Vec<PreferenceFile>,
    pub focus_paths: Vec<String>,
}

pub struct PreferenceFile {
    pub name: String,
    pub content: String,
}

pub struct EvalResult {
    pub verdict: String,              // concatenated agent_message_chunks
    pub stop_reason: String,          // camelCase: "endTurn" | "maxTokens" | ...
    pub tool_calls: Vec<ToolCallLog>, // in the order they executed
    pub duration: Duration,           // wall clock
    pub is_git_repo: bool,            // false if no .git was loaded
    pub repo_stats: RepoStats,
}

pub struct ToolCallLog {
    pub command: String,
    pub exit_code: i32,
    pub duration: Duration,
}

pub struct RepoStats {
    pub files: u64,
    pub bytes: u64,
}

#[derive(thiserror::Error, Debug)]
pub enum EvalError {
    #[error("repo too large: {total} bytes, limit {limit}")]
    RepoTooLarge { total: u64, limit: u64 },
    #[error(transparent)]
    VfsLoad(#[from] devdev_vfs::LoadError),
    #[error(transparent)]
    Acp(devdev_acp::AcpError),
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("session exceeded {0:?}")]
    Timeout(Duration),
    #[error("agent subprocess exited unexpectedly")]
    CliCrashed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Run a single evaluation and return a verdict. Drops the VFS and
/// kills the subprocess before returning, regardless of outcome.
pub async fn evaluate(
    repo_path: &Path,
    config: EvalConfig,
    context: EvalContext,
    transport: Transport,
) -> Result<EvalResult, EvalError>;
```

### Integration seam

`Transport` is the single non-obvious design decision. The evaluator
takes a transport instead of unconditionally spawning the CLI so that
the acceptance suite here can drive it over `tokio::io::duplex` with a
scripted fake agent — no `copilot` binary, no network, no tokens. The
real path (`Transport::copilot()`) delegates to
`AcpClient::connect_process`; the test path uses
`AcpClient::connect_transport`. Both already exist in `devdev-acp`.

### Verdict rule

Copilot's ACP emits several `session/update` variants during a turn:
`agent_message_chunk`, `agent_thought_chunk`, `tool_call`,
`tool_call_update`, and `plan`. The verdict is the **concatenation in
arrival order of every `agent_message_chunk.text`** — nothing else.
Thoughts, tool metadata, and plans are tracing-only.

### `.git` behaviour

If the repo has no `.git` directory (or `include_git: false`), the
evaluator installs a stub `VirtualGit` that answers every command with
exit 1 and the stderr `"not a git repository"`. `EvalResult.is_git_repo`
is `false`. This is **not an error** — reviewing a loose file tree is
a legitimate use case.

## Orchestration flow

```
evaluate()
    │
    ├── 1. Build MemFs (workspace_limit cap)
    ├── 2. load_repo(repo_path) — RepoTooLarge on over-limit, fail fast
    ├── 3. Build WasmToolRegistry (lazy compile — no wasm work yet)
    ├── 4. If .git present: build VirtualRepo + VirtualGitRepo
    │       Else:          install StubGit
    ├── 5. Build SandboxHandler via FnOnce closure that owns the
    │       shell construction on the worker thread (see cap 12 rule)
    ├── 6. Install a FanoutTraceLogger over VerdictCollector +
    │       ToolCallCollector
    ├── 7. AcpClient::connect_process | connect_transport
    ├── 8. client.initialize() — capability negotiation
    ├── 9. If authMethods non-empty and no GH_TOKEN env:
    │       client.authenticate(methods)
    │       (cascade: EnvToken → Method(id) → AcpError::NoAuth)
    ├── 10. client.new_session({ cwd: "/" })
    ├── 11. format_prompt(&ctx) → String
    ├── 12. client.prompt(...) (idle timeout = cli_hang_timeout)
    │         wrapped in tokio::time::timeout(session_timeout)
    ├── 13. Collect verdict + tool-call log from the trace collectors
    ├── 14. client.shutdown() — kills child, aborts I/O tasks
    └── 15. MemFs drops when `evaluate` returns — memory freed
```

Steps 5 and 6 carry the subtle invariants:

- Because `ShellSession: !Send`, the handler must be built via
  `SandboxHandler::new(|| ShellSession::new(...), vfs)`. The closure
  runs on the dedicated shell-worker thread (cap 12's rule).
- `TraceLogger` is `Arc<dyn TraceLogger>`. The evaluator wraps a
  `FanoutTraceLogger { VerdictCollector, ToolCallCollector }` so both
  sinks are fed from the single hook stream without plumbing multiple
  trace channels through `SandboxHandler`.

## Prompt formatting

`format_prompt` produces one plain string — no template engine. A
golden test pins the exact shape. Preference files appear in
declaration order; empty optional sections are omitted.

```
You are reviewing code in {repo_name}. {task}

## Preferences

{for pref in preferences}
### {pref.name}
{pref.content}
{endfor}

## Changes to Review
{if diff}
```diff
{diff}
```
{endif}

{if focus_paths}
Focus on these files: {focus_paths.join(", ")}
{endif}

Evaluate the changes against the preferences. Report any violations
found. If no violations, say "No issues found."
```

## Authentication cascade

`AcpClient::authenticate` already cascades:

1. If `GH_TOKEN` or `GITHUB_TOKEN` is set → `AuthStrategy::EnvToken`;
   skip the `authenticate` call.
2. Else if the agent advertised any `authMethods` → send
   `authenticate(method)`; on RPC error →
   `EvalError::AuthenticationFailed`.
3. Else (no env token, no methods) → proceed unauthenticated. Real
   Copilot will refuse later; scripted test agents never need auth.

## Error handling

| Condition | Result |
|-----------|--------|
| Host path missing or not a directory | `EvalError::VfsLoad(_)` |
| Loader exceeds `workspace_limit` | `EvalError::RepoTooLarge { total, limit }` — no subprocess spawn |
| Subprocess spawn fails | `EvalError::Acp(AcpError::Spawn(_))` |
| `initialize` / `new_session` RPC error | `EvalError::Acp(_)` |
| `authenticate` RPC error | `EvalError::AuthenticationFailed(msg)` |
| Whole-session wall clock expires | `EvalError::Timeout(session_timeout)`; client is shut down. Verdict and tool log up to that point are discarded (future work: surface partial results). |
| Reader task EOFs mid-turn | `EvalError::CliCrashed` |

## Files

```
crates/devdev-cli/src/lib.rs          — re-exports
crates/devdev-cli/src/config.rs       — EvalConfig, EvalContext, Transport
crates/devdev-cli/src/eval.rs         — evaluate() orchestrator
crates/devdev-cli/src/prompt.rs       — format_prompt + PreferenceFile
crates/devdev-cli/src/verdict.rs      — VerdictCollector + ToolCallCollector + Fanout
crates/devdev-cli/src/stub_git.rs     — StubGit: impl VirtualGit
crates/devdev-cli/tests/acceptance_eval.rs
```

Prerequisite edit in `devdev-acp`:

```
crates/devdev-acp/src/trace.rs        — TraceEvent::TerminalCreated gains duration_ms
crates/devdev-acp/src/hooks.rs        — emit duration_ms
```

## Acceptance Criteria

All ten run against a scripted fake agent over `tokio::io::duplex`.
No external binary, no network, no `GH_TOKEN`.

- [ ] **AC-01 simple_happy_path** — agent emits one `agent_message_chunk`
  then `end_turn`; `verdict` equals the chunk text,
  `stop_reason == "endTurn"`, `tool_calls` is empty.
- [ ] **AC-02 tool_call_roundtrip** — fake agent sends
  `terminal/create { command: "echo", args: ["hello"] }`, then
  `terminal/output`, `terminal/wait_for_exit`; `tool_calls[0]` equals
  `{ command: "echo hello", exit_code: 0, duration: non-zero }`.
- [ ] **AC-03 fs_roundtrip** — agent calls `fs/write_text_file` for
  `/out.txt`, then `terminal/create cat /out.txt`; output carries
  the written bytes.
- [ ] **AC-04 verdict_is_chunk_concat** — agent sends `"alpha "`, tool
  call, `"beta"`, `end_turn`; verdict equals `"alpha beta"`.
- [ ] **AC-05 repo_too_large_fails_before_spawn** — `workspace_limit: 1`,
  tempdir with any content; returns `RepoTooLarge`. Verified by using
  `Transport::SpawnProcess { program: "__devdev_should_never_spawn__", … }`
  — we must never reach that path.
- [ ] **AC-06 not_a_git_repo_is_soft** — tempdir with no `.git`; agent
  calls `terminal/create git log`; output contains
  `"not a git repository"`, exit code 1; `is_git_repo == false`;
  evaluation succeeds.
- [ ] **AC-07 session_timeout_returns_timeout_error** —
  `session_timeout: 50 ms`; fake agent never responds to
  `session/prompt`; call returns `EvalError::Timeout` within 250 ms;
  `AcpClient::shutdown` was invoked (reader/writer `JoinHandle`s
  finished).
- [ ] **AC-08 authentication_failure_propagates** — fake agent advertises
  a method and returns `RpcError` to `authenticate`; evaluator returns
  `EvalError::AuthenticationFailed(...)` with the error message.
- [ ] **AC-09 tool_call_log_order_preserved** — fake agent issues three
  `terminal/create` calls; `tool_calls` length 3; order matches
  issuance.
- [ ] **AC-10 agent_disconnect_returns_cli_crashed** — fake agent drops
  the pipe mid-turn after one tool call; evaluator returns
  `EvalError::CliCrashed`.

Resource-leak check (shared across tests): each test keeps a `Weak` to
the `Arc<Mutex<MemFs>>` it passed in and asserts
`Weak::strong_count() == 0` once `evaluate` returns. Covers "VFS
memory is freed after evaluation."
