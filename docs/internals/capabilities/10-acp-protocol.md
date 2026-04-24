---
id: acp-protocol
title: "ACP Protocol Types & Serialization"
status: done
type: leaf
phase: 2
crate: devdev-acp
priority: P0
depends-on: []
effort: M
---

# 10 — ACP Protocol Types & Serialization

Pure data types and (de)serialization for the Agent Client Protocol — JSON-RPC 2.0 over NDJSON. No I/O, no subprocess management, no business logic. This crate is the protocol vocabulary that all ACP code speaks.

Based on the [ACP specification](https://agentclientprotocol.com/protocol/overview) and Copilot CLI-specific extensions documented in `spirit/research-acp.md`.

## Scope

**In:**
- JSON-RPC 2.0 message types (request, response, notification, error)
- All ACP method parameter and result types
- Copilot-specific extension types (`session.shell.exec`, `tools.list`, etc.)
- NDJSON reader/writer (serialize to line-delimited JSON)
- Serde derives for all types

**Out:**
- Subprocess management (that's `11-acp-client`)
- Hook handling logic (that's `12-acp-hooks`)
- Any network or file I/O

## Protocol Overview

**Transport:** NDJSON (newline-delimited JSON) over stdio or TCP.
**Encoding:** JSON-RPC 2.0 — each message is `{"jsonrpc": "2.0", ...}\n`.

### Message Types

```rust
/// JSON-RPC 2.0 envelope
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,  // always "2.0"
    pub id: RequestId,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(u64),
    String(String),
}

#[derive(Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}
```

### Client → Agent Methods

| Method | Params | Result |
|--------|--------|--------|
| `initialize` | `InitializeParams` | `InitializeResult` |
| `authenticate` | `AuthenticateParams` | `AuthenticateResult` |
| `session/new` | `NewSessionParams` | `NewSessionResult` |
| `session/prompt` | `PromptParams` | `PromptResult` |
| `session/load` | `LoadSessionParams` | `()` |
| `session/list` | `()` | `ListSessionsResult` |
| `session/set_mode` | `SetModeParams` | `()` |
| `session/cancel` | `CancelParams` | *(notification, no response)* |

### Agent → Client Methods

| Method | Params | Result |
|--------|--------|--------|
| `session/request_permission` | `PermissionRequestParams` | `PermissionResponse` |
| `session/update` | `SessionUpdateParams` | *(notification, no response)* |
| `fs/read_text_file` | `ReadTextFileParams` | `ReadTextFileResult` |
| `fs/write_text_file` | `WriteTextFileParams` | `()` |
| `terminal/create` | `CreateTerminalParams` | `CreateTerminalResult` |
| `terminal/output` | `TerminalOutputParams` | `TerminalOutputResult` |
| `terminal/wait_for_exit` | `WaitForExitParams` | `WaitForExitResult` |
| `terminal/kill` | `KillTerminalParams` | `()` |
| `terminal/release` | `ReleaseTerminalParams` | `()` |

### Key Param/Result Types

```rust
// --- Initialize ---
pub struct InitializeParams {
    pub protocol_version: u16,
    pub client_capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

pub struct ClientCapabilities {
    pub fs: Option<FsCapabilities>,
    pub terminal: Option<bool>,
}

pub struct FsCapabilities {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

pub struct InitializeResult {
    pub protocol_version: u16,
    pub agent_info: AgentInfo,
    pub agent_capabilities: AgentCapabilities,
    pub auth_methods: Vec<AuthMethod>,
}

// --- Session ---
pub struct NewSessionParams {
    pub cwd: String,
    pub mcp_servers: Vec<McpServerConfig>,  // see cap 28 for schema
}

// Tagged union on `type`; cap-28 PoC (2026-04-22) verified Copilot CLI
// rejects a flat {name, url}. Headers are array-of-{name,value}.
pub enum McpServerConfig {
    Http  { name: String, url: String, headers: Vec<McpHeader> },
    Sse   { name: String, url: String, headers: Vec<McpHeader> },
    Stdio { name: String, command: String, args: Vec<String>, env: Vec<McpHeader> },
}

pub struct PromptParams {
    pub session_id: String,
    pub prompt: Vec<PromptContent>,
}

pub enum PromptContent {
    Text { text: String },
    Resource { resource: Resource },
}

pub struct PromptResult {
    pub stop_reason: StopReason,
}

pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

// --- Permission ---
pub struct PermissionRequestParams {
    pub session_id: String,
    pub tool_call: ToolCallInfo,
    pub options: Vec<PermissionOption>,
}

pub struct PermissionOption {
    pub option_id: String,
    pub kind: PermissionKind,
    pub name: String,
}

pub enum PermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

pub struct PermissionResponse {
    pub outcome: PermissionOutcome,
}

pub enum PermissionOutcome {
    Selected { option_id: String },
    Cancelled,
}

// --- Terminal ---
pub struct CreateTerminalParams {
    pub session_id: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<EnvVar>,
    pub output_byte_limit: Option<u64>,
}

pub struct CreateTerminalResult {
    pub terminal_id: String,
}

// --- FS ---
pub struct ReadTextFileParams {
    pub session_id: String,
    pub path: String,
    pub line: Option<u32>,
    pub limit: Option<u32>,
}

pub struct WriteTextFileParams {
    pub session_id: String,
    pub path: String,
    pub content: String,
}

// --- Session Updates (notifications) ---
pub struct SessionUpdateParams {
    pub session_id: String,
    pub update: SessionUpdate,
}

pub enum SessionUpdate {
    AgentMessageChunk { content: ContentBlock },
    AgentThoughtChunk { content: ContentBlock },
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    Plan { entries: Vec<PlanEntry> },
    // ... remaining variants
}

pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: ToolCallKind,
    pub status: ToolCallStatus,
    pub raw_input: Option<serde_json::Value>,
}

pub enum ToolCallKind {
    Read, Edit, Delete, Move, Search, Execute, Think, Fetch, Other,
}

pub enum ToolCallStatus {
    Pending, InProgress, Completed, Failed,
}
```

### NDJSON I/O

```rust
pub struct NdjsonWriter<W: Write> { writer: W }
pub struct NdjsonReader<R: BufRead> { reader: R }

impl<W: Write> NdjsonWriter<W> {
    pub fn send(&mut self, msg: &Message) -> io::Result<()>;
}

impl<R: BufRead> NdjsonReader<R> {
    pub fn recv(&mut self) -> io::Result<Message>;
}
```

## Implementation Notes

- **Serde rename_all:** ACP uses `camelCase` for JSON fields. Use `#[serde(rename_all = "camelCase")]` on all structs.
- **Serde tag for enums:** `SessionUpdate` variants are distinguished by a `sessionUpdate` field. Use `#[serde(tag = "sessionUpdate")]` with appropriate rename mappings.
- **Copilot-specific extensions:** Methods like `session.shell.exec` use dot notation (not slash). Handle both in the method dispatcher.
- **Request ID generation:** Client should use monotonically increasing `u64` IDs.
- **Error codes:** `-32000` = auth required, `-32601` = method not found, standard JSON-RPC codes otherwise.

## Files

```
crates/devdev-acp/src/protocol.rs   — Message, Request, Response, Notification
crates/devdev-acp/src/types.rs      — All param/result structs and enums
crates/devdev-acp/src/ndjson.rs     — NdjsonReader, NdjsonWriter
```

## Acceptance Criteria

- [ ] Round-trip: serialize `InitializeParams` → JSON string → deserialize → identical struct
- [ ] Round-trip all `SessionUpdate` variants through JSON
- [ ] NdjsonWriter produces one JSON object per line, newline-terminated
- [ ] NdjsonReader parses a multi-line NDJSON stream into individual Messages
- [ ] `PermissionRequestParams` deserializes from the exact JSON in the ACP spec
- [ ] `CreateTerminalParams` deserializes from the exact JSON in the ACP spec
- [ ] Unknown fields in JSON are ignored (forward compatibility with newer protocol versions)
- [ ] `RequestId` handles both numeric and string IDs
- [ ] Error response with code `-32000` deserializes correctly
