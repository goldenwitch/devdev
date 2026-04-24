---
id: chat-tui-headless
title: "Chat TUI & Headless Mode"
status: done
type: composition
phase: 2
crate: devdev-tui
priority: P0
depends-on: [daemon-lifecycle]
effort: L
---

# P2-03 — Chat TUI & Headless Mode

**New crate: `devdev-tui`.** Two modes of human interaction with the daemon: a terminal UI for interactive use, and a headless NDJSON pipe for CI/scripting/embedding. Both share the same IPC client.

## Scope

**In:**
- **TUI mode** (`devdev` bare command):
  - Single-pane scrollable chat. User types at bottom, messages scroll up.
  - Agent responses stream in token-by-token (as daemon relays `agent_message_chunk` events).
  - Status bar: daemon status, active tasks count, loaded repos.
  - Approval prompts: when a task wants to take an external action, TUI shows "[Approve / Reject]" inline.
  - Exit: `Ctrl+C` or `/quit` disconnects TUI. Daemon keeps running.
- **Headless mode** (`devdev attach --headless`):
  - Reads NDJSON from stdin, writes NDJSON to stdout.
  - Same IPC connection as TUI, same message semantics.
  - Approval prompts emitted as `{"type": "approval_request", "action": "post_review", "details": {...}}`.
  - Approvals sent as `{"type": "approval_response", "approve": true}`.
  - If no approval response within configurable timeout → action dropped (fail-safe).
- **Shared IPC client** (`devdev-tui::ipc_client`):
  - Connect to daemon socket.
  - Send messages (user text, task commands, approval responses).
  - Receive messages (agent text chunks, status updates, approval requests).
  - Used by both TUI renderer and headless stdin/stdout bridge.

**Out:**
- Multi-pane layout, tabs, split views (v2+).
- Syntax highlighting of code in agent responses (v2+ — raw markdown for now).
- Image rendering.
- Mouse support beyond basic scroll.
- Web UI, Electron, VS Code extension.

## PoC Requirement (Spec Rule 2)

Before committing to ratatui:

1. Build a throwaway TUI: input box at bottom, scrollable text area above, status bar.
2. Test on Windows Terminal, macOS Terminal.app, Linux xterm.
3. Verify: text input works, scrollback works, Ctrl+C handling works, Unicode renders.

**PoC Result:** _Not yet run._

## Interface

### IPC Client (shared by TUI and headless)

```rust
pub struct DaemonConnection {
    conn: IpcConnection,
}

impl DaemonConnection {
    /// Connect to running daemon.
    pub async fn connect(data_dir: &Path) -> Result<Self, ConnectError>;

    /// Enter attach/streaming mode.
    pub async fn attach(&mut self) -> Result<(), ConnectError>;

    /// Send a user message.
    pub async fn send_message(&mut self, text: &str) -> Result<(), ConnectError>;

    /// Send an approval response.
    pub async fn send_approval(&mut self, approve: bool) -> Result<(), ConnectError>;

    /// Receive next event from daemon (agent text, approval request, status update).
    pub async fn recv_event(&mut self) -> Result<DaemonEvent, ConnectError>;
}

pub enum DaemonEvent {
    /// Agent text chunk (stream incrementally).
    AgentText { text: String, done: bool },
    /// Agent finished a complete response.
    AgentDone { full_text: String },
    /// Task wants to take an external action and needs approval.
    ApprovalRequest { action: String, details: serde_json::Value },
    /// Status update (task created, task finished, repo loaded, etc.).
    StatusUpdate { message: String },
    /// Error from daemon.
    Error { message: String },
}
```

### Headless NDJSON Protocol

**Stdin (user → daemon):**
```json
{"type": "message", "text": "Monitor PR #247 in org/repo"}
{"type": "approval_response", "approve": true}
{"type": "message", "text": "What did you find?"}
```

**Stdout (daemon → user):**
```json
{"type": "agent_text", "text": "Loading org/repo into workspace...", "done": false}
{"type": "agent_text", "text": " Fetching PR #247 diff...", "done": false}
{"type": "agent_done", "full_text": "Loading org/repo into workspace... Fetching PR #247 diff..."}
{"type": "approval_request", "action": "post_review", "details": {"repo": "org/repo", "pr": 247}}
{"type": "status", "message": "Task t-1 created: Monitoring PR #247"}
```

### TUI Layout

```
┌──────────────────────────────────────┐
│ DevDev ─ 2 tasks ─ 1 repo           │  ← Status bar
├──────────────────────────────────────┤
│                                      │
│ [user] Monitor PR #247 in org/repo   │
│                                      │
│ [agent] Loading org/repo into        │  ← Scrollable
│ workspace... Fetching PR #247 diff.. │     chat area
│                                      │
│ [agent] I found two issues:          │
│ 1. parse_config() doesn't validate...│
│                                      │
│ ⚡ Post review to org/repo#247?      │  ← Approval prompt
│   [Y]es  [N]o  [D]ry-run            │
│                                      │
├──────────────────────────────────────┤
│ > _                                  │  ← Input line
└──────────────────────────────────────┘
```

## Implementation Notes

- **ratatui** with **crossterm** backend for cross-platform terminal handling.
- **Async event loop:** `tokio::select!` over terminal input events (crossterm) and daemon IPC events. No blocking.
- **Streaming text:** Agent text arrives as chunks. Buffer and re-render the current message line on each chunk. Mark complete on `AgentDone`.
- **Chat history:** `Vec<ChatMessage>` with scroll offset. Render the visible window. Up/Down or mouse scroll to view history.
- **Input line:** Simple line editor. Enter to send. Up arrow for input history (last 50 commands).
- **Approval UX:** Inline prompt with Y/N/D keybinds. Pressing a key sends the approval response over IPC immediately.
- **Headless mode:** Trivially simple — `tokio::io::stdin()` → parse NDJSON → `DaemonConnection::send_*()`, `DaemonConnection::recv_event()` → serialize NDJSON → `tokio::io::stdout()`. No terminal manipulation.
- **Graceful exit:** TUI catches Ctrl+C, sends detach to daemon, restores terminal, exits. Daemon keeps running.

## Files

```
crates/devdev-tui/Cargo.toml
crates/devdev-tui/src/lib.rs            — re-exports
crates/devdev-tui/src/ipc_client.rs     — DaemonConnection, DaemonEvent
crates/devdev-tui/src/tui.rs            — ratatui rendering, event loop
crates/devdev-tui/src/headless.rs       — stdin/stdout NDJSON bridge
crates/devdev-tui/src/chat.rs           — ChatMessage, chat history model
crates/devdev-cli/src/main.rs           — `devdev` (bare) launches TUI, `devdev attach --headless` launches headless
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-03-1 | §3.2 | TUI: scrollable chat, input line, status bar |
| SR-03-2 | §3.2 | Agent responses stream token-by-token |
| SR-03-3 | §3.2 | Headless NDJSON on stdin/stdout |
| SR-03-4 | §3.2 | Headless is integration surface for CI, editors, scripts, testing |
| SR-03-5 | §3.2 | TUI and headless share same IPC client |
| SR-03-6 | §2 Principle 6 | Every HITL path has headless equivalent |
| SR-03-7 | §3.3 (approval policy) | Approval prompts appear in TUI and headless |
| SR-03-8 | §4 (TUI row) | Messages round-trip without corruption |
| SR-03-9 | §4 (Headless attach row) | Full conversation works over pipes |

## Acceptance Tests

### TUI (via ratatui test backend)

- [ ] `tui_renders_status_bar` — verify status bar shows task count and repo list
- [ ] `tui_user_input_dispatched` — type text, press Enter → `send_message` called on IPC client
- [ ] `tui_agent_text_streams` — receive AgentText chunks → display updates incrementally
- [ ] `tui_agent_done_shows_complete` — receive AgentDone → full message visible in history
- [ ] `tui_scroll_history` — 50 messages → scroll up shows older messages
- [ ] `tui_approval_prompt_renders` — receive ApprovalRequest → inline prompt visible
- [ ] `tui_approval_y_sends_approve` — press Y on approval → `send_approval(true)` sent
- [ ] `tui_approval_n_sends_reject` — press N on approval → `send_approval(false)` sent
- [ ] `tui_ctrl_c_disconnects` — Ctrl+C → TUI exits, daemon keeps running

### Headless

- [ ] `headless_message_roundtrip` — pipe `{"type":"message","text":"hello"}` to stdin → receive AgentDone on stdout
- [ ] `headless_approval_flow` — receive approval_request on stdout, send `{"type":"approval_response","approve":true}` on stdin → action proceeds
- [ ] `headless_approval_timeout_drops` — receive approval_request, send nothing → action dropped after timeout
- [ ] `headless_json_schema_valid` — all stdout lines are valid JSON matching the documented schema
- [ ] `headless_concurrent_messages` — send multiple messages rapidly → responses arrive in order
- [ ] `headless_stdin_eof_disconnects` — close stdin → headless mode exits cleanly

### IPC Client

- [ ] `ipc_client_connect_to_daemon` — connect, send status request, get response
- [ ] `ipc_client_attach_streaming` — enter attach mode, exchange messages bidirectionally
- [ ] `ipc_client_reconnect_on_error` — daemon connection drops → error returned (no hang)

## Spec Compliance Checklist

- [ ] SR-03-1 through SR-03-9: all requirements covered
- [ ] PoC result recorded for ratatui
- [ ] All acceptance tests passing
