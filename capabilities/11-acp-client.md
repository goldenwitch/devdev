---
id: acp-client
title: "ACP Client & Subprocess Management"
status: done
type: leaf
phase: 4
crate: devdev-acp
priority: P0
depends-on: [acp-protocol]
effort: M
---

# 11 — ACP Client & Subprocess Management

Spawn the Copilot CLI as a subprocess, manage the stdio JSON-RPC channel, and handle the bidirectional message flow. This is the substrate for ACP communication — it handles transport, not business logic.

## Scope

**In:**
- Spawn `copilot --acp --stdio` subprocess
- Async stdio management: write requests to stdin, read responses/notifications from stdout
- Request/response correlation by ID
- Handle interleaved messages: during a blocking `session/prompt` call, the agent sends notifications (`session/update`) and requests (`session/request_permission`, `terminal/create`, `fs/*`) that must be processed concurrently
- Authentication cascade: `GH_TOKEN` → `gh auth` → device code flow
- Subprocess lifecycle: spawn, health check, graceful shutdown, force kill

**Out:**
- Business logic for handling terminal/fs/permission requests (that's `12-acp-hooks`)
- Session orchestration (that's `13-sandbox-integration`)

## Interface

```rust
/// Callback dispatcher for agent-initiated requests.
/// The ACP client calls these when the agent sends requests.
#[async_trait]
pub trait AcpHandler: Send + Sync {
    async fn on_permission_request(&self, params: PermissionRequestParams) -> PermissionResponse;
    async fn on_terminal_create(&self, params: CreateTerminalParams) -> Result<CreateTerminalResult>;
    async fn on_terminal_output(&self, params: TerminalOutputParams) -> Result<TerminalOutputResult>;
    async fn on_terminal_wait(&self, params: WaitForExitParams) -> Result<WaitForExitResult>;
    async fn on_terminal_kill(&self, params: KillTerminalParams) -> Result<()>;
    async fn on_terminal_release(&self, params: ReleaseTerminalParams) -> Result<()>;
    async fn on_fs_read(&self, params: ReadTextFileParams) -> Result<ReadTextFileResult>;
    async fn on_fs_write(&self, params: WriteTextFileParams) -> Result<()>;
    async fn on_session_update(&self, params: SessionUpdateParams);
}

/// The ACP client — manages the subprocess and message routing.
pub struct AcpClient {
    // subprocess handle, stdin writer, stdout reader, pending requests
}

impl AcpClient {
    /// Spawn the Copilot CLI and initialize the ACP connection.
    pub async fn connect(handler: Arc<dyn AcpHandler>) -> Result<Self, AcpError>;
    
    /// Send initialize and negotiate capabilities.
    pub async fn initialize(&mut self) -> Result<InitializeResult>;
    
    /// Authenticate (if needed).
    pub async fn authenticate(&mut self) -> Result<()>;
    
    /// Create a new session.
    pub async fn new_session(&mut self, params: NewSessionParams) -> Result<NewSessionResult>;
    
    /// Send a prompt and block until the turn completes.
    /// During this call, agent-initiated requests are dispatched to the handler.
    pub async fn prompt(&mut self, params: PromptParams) -> Result<PromptResult>;
    
    /// Cancel an in-progress prompt.
    pub async fn cancel(&mut self, session_id: &str) -> Result<()>;
    
    /// Gracefully shut down: kill subprocess.
    pub async fn shutdown(self) -> Result<()>;
}
```

## Message Flow Architecture

The key complexity: `prompt()` sends a `session/prompt` request and waits for the response. But between sending and receiving, the agent sends **interleaved** messages on the same stdio stream:

```
Client → Agent:  session/prompt (id: 5)
Agent → Client:  session/update (notification — agent thinking)
Agent → Client:  session/update (notification — tool_call pending)
Agent → Client:  session/request_permission (id: 100 — needs approval)
Client → Agent:  permission response (id: 100 — approved)
Agent → Client:  terminal/create (id: 200 — execute command)
Client → Agent:  terminal/create response (id: 200 — terminal_id)
Agent → Client:  session/update (notification — tool_call completed)
Agent → Client:  session/prompt response (id: 5 — turn complete)
```

### Implementation

Use tokio for async I/O. Two tasks:

1. **Reader task:** Continuously reads NDJSON lines from stdout. For each message:
   - If it's a **response** (has `id`, has `result` or `error`): match to a pending request by ID, wake the waiting future.
   - If it's a **notification** (`session/update`): dispatch to handler.
   - If it's an **agent request** (`session/request_permission`, `terminal/*`, `fs/*`): dispatch to handler, send the handler's return value as a response back to the agent via stdin.

2. **Writer task:** Serializes outgoing messages (client requests + responses to agent requests) to stdin as NDJSON.

Pending request tracking: `HashMap<RequestId, oneshot::Sender<Response>>`.

## Authentication

```rust
async fn authenticate(&mut self) -> Result<()> {
    // 1. Check if GH_TOKEN or GITHUB_TOKEN or COPILOT_GITHUB_TOKEN is set
    //    If so, the CLI picks it up automatically — no auth RPC needed.
    
    // 2. Try session/new. If it works, auth is good.
    
    // 3. If session/new returns error -32000 (auth required):
    //    a. Check initialize response for auth_methods.
    //    b. Try device code flow (interactive fallback).
}
```

## Timeouts & Error Handling

| Condition | Behavior |
|-----------|----------|
| CLI subprocess exits unexpectedly | `AcpError::SubprocessCrashed(exit_code)` |
| No messages from CLI for 60s | `AcpError::Timeout` — kill subprocess |
| Agent request handler returns error | Send JSON-RPC error response to agent |
| stdin write fails | `AcpError::BrokenPipe` — subprocess likely dead |
| Malformed NDJSON from stdout | Log warning, skip line, continue |

All timeouts configurable via `AcpClientConfig`.

## Files

```
crates/devdev-acp/src/client.rs     — AcpClient, spawn, message routing, reader/writer tasks
crates/devdev-acp/src/handler.rs    — AcpHandler trait (async_trait)
crates/devdev-acp/src/auth.rs       — env-token cascade + AuthStrategy
crates/devdev-acp/src/transport.rs  — async NDJSON reader/writer over AsyncRead/AsyncWrite
```

Implementation notes:
- `connect_transport(reader, writer, ...)` takes any `AsyncRead + AsyncWrite` pair — used by tests via `tokio::io::duplex`. `connect_process(program, args, ...)` is the real path that spawns `copilot --acp --stdio` with `kill_on_drop`.
- Reader + writer run as separate `tokio::spawn` tasks. Outgoing messages flow through an `mpsc` channel so agent-initiated request handlers (spawned on the reader side) can enqueue responses without racing an in-flight client request.
- Pending request correlation: `Arc<Mutex<HashMap<RequestId, oneshot::Sender<Response>>>>` + atomic `u64` counter. On reader EOF, every pending waiter is woken with an `internal_error("agent disconnected")` response.
- Device-code auth flow is intentionally out of P0 scope — `authenticate()` only handles env-token short-circuit + a simple `authenticate` RPC with an advertised method. Interactive fallback is a P1 follow-up.
- Idle timeout (default 60s) applies to `prompt()` via `call_with_timeout`. Other RPCs use the shorter request timeout (default 30s).

## Acceptance Criteria

- [ ] Spawn `copilot --acp --stdio`, send `initialize`, receive response with capabilities
- [ ] `new_session()` returns a session ID
- [ ] `prompt()` sends request, receives `session/update` notifications during wait, returns `PromptResult`
- [ ] Agent `terminal/create` request dispatches to handler and response flows back
- [ ] Agent `fs/read_text_file` request dispatches to handler
- [ ] Request/response correlation: multiple in-flight requests resolve to correct waiters
- [ ] Subprocess crash → `AcpError::SubprocessCrashed`
- [ ] 60s silence → timeout, subprocess killed
- [ ] Auth with `GH_TOKEN` env var → session created without interactive flow
