---
id: session-router
title: "Session Router"
status: not-started
type: composition
phase: 2
crate: devdev-daemon
priority: P0
depends-on: [task-manager]
effort: M
---

# P2-06 — Session Router

Maps tasks to ACP agent sessions. Each task gets its own logical session with accumulated context. All sessions multiplex over one Copilot CLI subprocess. If the subprocess crashes, the router restarts it and recreates sessions — tasks are durable, sessions are not.

## Scope

**In:**
- `SessionRouter`: manages a pool of logical ACP sessions, one per task.
- `SessionHandle`: a lightweight reference a task uses to send prompts and receive responses.
- Session lifecycle: create on task creation, destroy on task completion/cancellation.
- Session multiplexing: one `AcpClient` instance (one subprocess), multiple `session/new` calls.
- Crash recovery: detect subprocess exit, restart, recreate all active sessions, resume tasks.
- Context injection: each session's initial prompt includes the task's context (repo path, PR diff, prior observations).
- Interactive/chat session: one session for the TUI/headless user interaction (not tied to a task).

**Out:**
- Subprocess pool (multiple Copilot CLI processes). Start with one. Pool if multi-session doesn't work.
- Session migration across daemon restarts (sessions die on restart; tasks resume with fresh sessions from checkpoint state).
- Custom system prompts or fine-tuning.

## PoC Requirement (Spec Rule 2)

Critical: Does one `copilot --acp --stdio` subprocess support multiple concurrent `session/new` calls?

1. Start one subprocess.
2. Send `session/new` twice with different session IDs.
3. Send prompts to both sessions concurrently.
4. Verify both respond independently.

If this fails: fall back to one subprocess per session (subprocess pool).

**PoC Result:** _Not yet run._

## Interface

```rust
pub struct SessionRouter {
    client: Arc<AcpClient>,
    sessions: Mutex<HashMap<String, SessionState>>,  // task_id → session
    transport: Transport,
    handler_factory: Arc<dyn Fn(String) -> Arc<dyn AcpHandler> + Send + Sync>,
}

struct SessionState {
    session_id: String,
    task_id: String,
    context: SessionContext,
    created_at: Instant,
}

pub struct SessionContext {
    pub system_prompt: String,      // task-specific instructions
    pub repo_paths: Vec<PathBuf>,   // repos loaded for this task
    pub prior_observations: Vec<String>,  // accumulated context from prior polls
}

pub struct SessionHandle {
    task_id: String,
    router: Arc<SessionRouter>,
}

impl SessionRouter {
    pub async fn new(transport: Transport, handler_factory: impl Fn(String) -> Arc<dyn AcpHandler> + Send + Sync + 'static) -> Result<Self, RouterError>;

    /// Create a new session for a task.
    pub async fn create_session(&self, task_id: &str, context: SessionContext) -> Result<SessionHandle, RouterError>;

    /// Destroy a session (task completed/cancelled).
    pub async fn destroy_session(&self, task_id: &str) -> Result<(), RouterError>;

    /// Create the interactive session (for TUI/headless chat).
    pub async fn create_interactive_session(&self) -> Result<SessionHandle, RouterError>;

    /// Handle subprocess crash: restart and recreate all sessions.
    pub async fn recover(&self) -> Result<(), RouterError>;
}

impl SessionHandle {
    /// Send a prompt to the agent and collect the response.
    pub async fn send_prompt(&self, prompt: &str) -> Result<AgentResponse, RouterError>;

    /// Send a prompt and stream response chunks.
    pub async fn send_prompt_streaming(&self, prompt: &str) -> Result<ResponseStream, RouterError>;
}

pub struct AgentResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCallLog>,
    pub stop_reason: String,
}

pub struct ResponseStream {
    receiver: mpsc::Receiver<ResponseChunk>,
}

pub enum ResponseChunk {
    Text(String),
    ToolCall(ToolCallLog),
    Done { stop_reason: String },
}

#[derive(thiserror::Error, Debug)]
pub enum RouterError {
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("ACP error: {0}")]
    Acp(#[from] devdev_acp::AcpError),
    #[error("subprocess crashed, recovery in progress")]
    SubprocessCrashed,
    #[error("multi-session not supported, using subprocess pool")]
    MultiSessionUnsupported,
}
```

## Implementation Notes

- **AcpClient reuse:** The existing `AcpClient` supports `session/new`. The router wraps it to manage session-task mapping.
- **Handler per session:** Each session needs its own `SandboxHandler` because each task operates on different VFS paths. The `handler_factory` closure creates per-session handlers.
- **Crash detection:** The `AcpClient` reader task will observe EOF when the subprocess dies. Surface this as a channel message. The router subscribes and triggers `recover()`.
- **Recovery sequence:** (1) Detect crash, (2) restart subprocess via `AcpClient::connect_process`, (3) `session/new` for every active session, (4) re-inject context prompts. Tasks don't know recovery happened — their next `send_prompt` just works.
- **Interactive session:** The TUI/headless chat uses a dedicated session with a general-purpose system prompt. This session is always active while a user is attached.
- **Streaming:** `send_prompt_streaming` returns a `ResponseStream` that yields `Text` chunks as they arrive (for TUI token-by-token display). `send_prompt` is a convenience that collects all chunks into a single string.
- **Concurrency:** Multiple tasks may `send_prompt` concurrently. The ACP client handles request multiplexing via its existing `pending` map with `RequestId` → `oneshot::Sender`.

## Files

```
crates/devdev-daemon/src/router.rs      — SessionRouter, SessionHandle, SessionState
crates/devdev-daemon/src/router.rs      — ResponseStream, AgentResponse
crates/devdev-daemon/src/recovery.rs    — Crash detection, subprocess restart, session recreation
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-06-1 | §3.5 | Sessions created per-task, not per-daemon |
| SR-06-2 | §3.5 | One Copilot CLI subprocess, multiple logical sessions |
| SR-06-3 | §3.5 | Crash recovery: restart subprocess, recreate sessions |
| SR-06-4 | §3.5 | Tasks are durable, sessions are not |
| SR-06-5 | §3.5 | Each task injects its context into its session prompt |
| SR-06-6 | §4 (Session Router row) | Tasks maintain agent sessions |
| SR-06-7 | §4 (Session Router row) | Crash recovery doesn't lose task state |
| SR-06-8 | Open Question #2 | PoC: validate multi-session support |

## Acceptance Tests

- [ ] `create_session_for_task` — create session → SessionHandle returned, session visible in router
- [ ] `send_prompt_gets_response` — create session, send prompt → AgentResponse with non-empty text (using fake agent)
- [ ] `send_prompt_streaming_yields_chunks` — create session, send prompt streaming → multiple Text chunks followed by Done
- [ ] `multiple_sessions_independent` — create two sessions, send different prompts → responses are independent (not cross-contaminated)
- [ ] `destroy_session_cleans_up` — create then destroy → session gone, subsequent send_prompt errors
- [ ] `interactive_session_works` — create interactive session → send/receive works like task sessions
- [ ] `crash_recovery_restarts_subprocess` — kill subprocess → router detects, restarts, sessions recreated
- [ ] `crash_recovery_task_send_succeeds_after` — crash → recovery → task sends prompt → response arrives
- [ ] `crash_recovery_preserves_task_state` — task had accumulated context → after recovery, context is re-injected
- [ ] `concurrent_sends_from_multiple_tasks` — 5 tasks send prompts concurrently → all get correct responses
- [ ] `multi_session_poc_validated` — (PoC) one subprocess, two session/new calls, both respond (if fails, switch to subprocess pool)

## Spec Compliance Checklist

- [ ] SR-06-1 through SR-06-8: all requirements covered
- [ ] PoC result recorded for multi-session
- [ ] All acceptance tests passing
