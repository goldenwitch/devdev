# Research: Copilot CLI Agent Client Protocol (ACP)

**Date:** 2026-04-15  
**Status:** Comprehensive — open-spec protocol fully documented; Copilot-specific details from changelog, issues, SDK, and official docs.

---

## Table of Contents

1. [Protocol Identity & Provenance](#1-protocol-identity--provenance)
2. [Transport & Wire Format](#2-transport--wire-format)
3. [JSON-RPC Method Catalog](#3-json-rpc-method-catalog)
4. [Message Schemas (Concrete JSON)](#4-message-schemas-concrete-json)
5. [Session Lifecycle](#5-session-lifecycle)
6. [Tool Calls & Permission Requests](#6-tool-calls--permission-requests)
7. [Hooks System (File-Based)](#7-hooks-system-file-based)
8. [SDK Hooks (Programmatic)](#8-sdk-hooks-programmatic)
9. [Authentication](#9-authentication)
10. [Copilot-Specific ACP Extensions](#10-copilot-specific-acp-extensions)
11. [SDKs & Libraries](#11-sdks--libraries)
12. [Error Handling](#12-error-handling)
13. [Protocol Versioning & Capabilities](#13-protocol-versioning--capabilities)
14. [Architecture Decision: How DevDev Should Integrate](#14-architecture-decision-how-devdev-should-integrate)
15. [Sources & Confidence Levels](#15-sources--confidence-levels)

---

## 1. Protocol Identity & Provenance

- **ACP** = **Agent Client Protocol** — an open standard for communication between AI agent backends and client frontends (IDEs, editors, CLI wrappers).
- Originated from [Zed Industries](https://agentclientprotocol.com/), analogous to LSP but for AI agents.
- Spec repo: `github.com/zed-industries/agent-client-protocol` → now at `agentclientprotocol.com`
- Copilot CLI added `--acp` flag in **v0.0.397** (late 2025), announced public preview **Jan 28, 2026**.
- Copilot CLI: npm package `@github/copilot` — v1.0.26 as of Apr 14, 2026. 3M+ weekly downloads.
- ACP adopters: Copilot CLI, Gemini CLI, Claude Code (via adapter), Zed, JetBrains, Neovim (CodeCompanion), Emacs (Agent Shell).

---

## 2. Transport & Wire Format

### Wire Protocol
- **JSON-RPC 2.0** over **NDJSON** (newline-delimited JSON).
- Each message is a single JSON object on one line, terminated by `\n`.

### Transport Modes

**stdio (the only mode in current Copilot CLI, recommended for embedding):**
```bash
copilot --acp
```
The `--acp` flag puts the CLI in ACP mode over its inherited stdin/stdout — there is no separate `--stdio` flag. Write JSON-RPC (NDJSON) to stdin, read from stdout. stderr is for logs (pass through or discard).

> **PoC finding (2026-04-22):** Non-interactive use additionally requires `--allow-all-tools` (or `COPILOT_ALLOW_ALL=1`); without it the CLI blocks on a permission prompt even for text-only turns. Validated against `GitHub Copilot CLI 1.0.34`, protocol version `1`.

**TCP mode (research-era documentation — not exposed by current CLI):**
```bash
copilot --acp --port 3000
```
ACP server binds to **localhost only** (v1.0.26 security fix). Useful for multi-process architectures.

### Message Types
1. **Methods** (request → response): Have `id`, `method`, `params` → response has matching `id`, `result`/`error`.
2. **Notifications** (one-way): Have `method`, `params` but NO `id`. No response expected.

---

## 3. JSON-RPC Method Catalog

### Agent-Side Methods (Client → Agent)

| Method | Type | Required | Description |
|--------|------|----------|-------------|
| `initialize` | Request | **Baseline** | Negotiate protocol version + capabilities |
| `authenticate` | Request | If needed | Authenticate with agent (OAuth, PAT) |
| `session/new` | Request | **Baseline** | Create new conversation session |
| `session/prompt` | Request | **Baseline** | Send user message to agent |
| `session/cancel` | Notification | **Baseline** | Cancel ongoing prompt turn |
| `session/load` | Request | Optional | Resume existing session (requires `loadSession` capability) |
| `session/list` | Request | Optional | List available sessions (requires `sessionCapabilities.list`) |
| `session/set_mode` | Request | Optional | Switch agent mode (e.g., "ask", "architect", "code") |
| `session/set_config_option` | Request | Optional | Change session config (model, reasoning effort) |

### Client-Side Methods (Agent → Client)

| Method | Type | Required | Description |
|--------|------|----------|-------------|
| `session/request_permission` | Request | **Baseline** | Ask user to approve/deny tool call |
| `session/update` | Notification | **Baseline** | Stream session updates (messages, tool calls, plans) |
| `fs/read_text_file` | Request | Optional | Read file from client filesystem |
| `fs/write_text_file` | Request | Optional | Write file to client filesystem |
| `terminal/create` | Request | Optional | Create terminal and execute command |
| `terminal/output` | Request | Optional | Get terminal output |
| `terminal/wait_for_exit` | Request | Optional | Wait for terminal command to finish |
| `terminal/kill` | Request | Optional | Kill terminal process |
| `terminal/release` | Request | Optional | Release terminal resources |

### Copilot-Specific Methods (from changelog, not in ACP base spec)

| Method | Version Added | Description |
|--------|--------------|-------------|
| `session.shell.exec` | v1.0.4 | Execute shell command in session context |
| `session.shell.kill` | v1.0.4 | Kill running shell command |
| `tools.list` | v0.0.407 | Query available built-in tools |
| `exitPlanMode.request` | v0.0.422 | Exit plan mode programmatically |
| `mcp.config.list` | v1.0.15 | List MCP server configurations |
| `mcp.config.add` | v1.0.15 | Add MCP server |
| `mcp.config.update` | v1.0.15 | Update MCP server config |
| `mcp.config.remove` | v1.0.15 | Remove MCP server |

> **Note:** The `session.shell.exec` / `session.shell.kill` methods use **dot notation**, not the ACP standard **slash notation**. These may be Copilot-internal extensions exposed over the same JSON-RPC transport.

---

## 4. Message Schemas (Concrete JSON)

### 4.1 Initialize

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 0,
  "method": "initialize",
  "params": {
    "protocolVersion": 1,
    "clientCapabilities": {
      "fs": { "readTextFile": false, "writeTextFile": false },
      "terminal": false
    },
    "clientInfo": {
      "name": "devdev",
      "version": "0.1.0"
    }
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 0,
  "result": {
    "protocolVersion": 1,
    "agentInfo": {
      "name": "copilot-cli",
      "version": "1.0.26"
    },
    "agentCapabilities": {
      "loadSession": true,
      "mcpCapabilities": { "http": false, "sse": false },
      "promptCapabilities": { "image": true, "audio": false, "embeddedContext": true },
      "sessionCapabilities": { "list": {} }
    },
    "authMethods": [
      {
        "id": "github-oauth",
        "name": "GitHub OAuth",
        "description": "Authenticate via GitHub OAuth device flow"
      }
    ]
  }
}
```

### 4.2 Session/New

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "session/new",
  "params": {
    "cwd": "/home/user/project",
    "mcpServers": [
      {
        "name": "my-tools",
        "command": "/path/to/mcp-server",
        "args": ["--stdio"],
        "env": [{ "name": "API_KEY", "value": "..." }]
      }
    ]
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "sessionId": "sess_abc123def456",
    "configOptions": [
      {
        "type": "select",
        "currentValue": "gpt-5",
        "options": [
          { "value": "gpt-5", "name": "GPT-5" },
          { "value": "claude-sonnet-4.5", "name": "Claude Sonnet 4.5" }
        ]
      }
    ],
    "modes": {
      "currentModeId": "agent",
      "availableModes": [
        { "id": "agent", "name": "Agent" },
        { "id": "plan", "name": "Plan" }
      ]
    }
  }
}
```

### 4.3 Session/Prompt

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "session/prompt",
  "params": {
    "sessionId": "sess_abc123def456",
    "prompt": [
      { "type": "text", "text": "Review this PR for security issues" },
      {
        "type": "resource",
        "resource": {
          "uri": "file:///project/src/auth.rs",
          "mimeType": "text/x-rust",
          "text": "fn validate_token(token: &str) -> bool { ... }"
        }
      }
    ]
  }
}
```

**Response (when turn completes):**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "stopReason": "end_turn"
  }
}
```

`stopReason` values: `"end_turn"`, `"max_tokens"`, `"max_turn_requests"`, `"refusal"`, `"cancelled"`

### 4.4 Session/Update Notifications (Agent → Client)

**Agent message chunk (streaming text):**
```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_abc123def456",
    "update": {
      "sessionUpdate": "agent_message_chunk",
      "content": { "type": "text", "text": "I'll analyze your code..." }
    }
  }
}
```

**Tool call initiated:**
```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_abc123def456",
    "update": {
      "sessionUpdate": "tool_call",
      "toolCallId": "call_001",
      "title": "Running grep -rn 'unsafe' src/",
      "kind": "execute",
      "status": "pending",
      "rawInput": { "command": "grep -rn 'unsafe' src/" }
    }
  }
}
```

**Tool call status update (in progress):**
```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_abc123def456",
    "update": {
      "sessionUpdate": "tool_call_update",
      "toolCallId": "call_001",
      "status": "in_progress"
    }
  }
}
```

**Tool call completed:**
```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_abc123def456",
    "update": {
      "sessionUpdate": "tool_call_update",
      "toolCallId": "call_001",
      "status": "completed",
      "content": [
        {
          "type": "content",
          "content": { "type": "text", "text": "src/auth.rs:42: unsafe { ... }" }
        }
      ]
    }
  }
}
```

**Plan update:**
```json
{
  "jsonrpc": "2.0",
  "method": "session/update",
  "params": {
    "sessionId": "sess_abc123def456",
    "update": {
      "sessionUpdate": "plan",
      "entries": [
        { "content": "Check for unsafe code", "priority": "high", "status": "in_progress" },
        { "content": "Review error handling", "priority": "medium", "status": "pending" }
      ]
    }
  }
}
```

**Session update types (full list):**
- `user_message_chunk` — streaming user message replay
- `agent_message_chunk` — streaming agent text
- `agent_thought_chunk` — streaming reasoning/chain-of-thought
- `tool_call` — new tool call initiated
- `tool_call_update` — tool call status/content update
- `plan` — execution plan
- `available_commands_update` — slash commands changed
- `current_mode_update` — session mode changed
- `config_option_update` — config options changed
- `session_info_update` — session metadata (title) changed

### 4.5 Session/Request_Permission (Agent → Client)

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 100,
  "method": "session/request_permission",
  "params": {
    "sessionId": "sess_abc123def456",
    "toolCall": {
      "toolCallId": "call_001",
      "title": "Execute: rm -rf dist/",
      "kind": "execute",
      "status": "pending",
      "rawInput": { "command": "rm -rf dist/" }
    },
    "options": [
      { "optionId": "allow_once", "kind": "allow_once", "name": "Allow once" },
      { "optionId": "allow_always", "kind": "allow_always", "name": "Always allow" },
      { "optionId": "reject_once", "kind": "reject_once", "name": "Reject" },
      { "optionId": "reject_always", "kind": "reject_always", "name": "Always reject" }
    ]
  }
}
```

**Response (approve):**
```json
{
  "jsonrpc": "2.0",
  "id": 100,
  "result": {
    "outcome": {
      "outcome": "selected",
      "optionId": "allow_once"
    }
  }
}
```

**Response (cancel — required when client sends session/cancel):**
```json
{
  "jsonrpc": "2.0",
  "id": 100,
  "result": {
    "outcome": { "outcome": "cancelled" }
  }
}
```

### 4.6 Session/Cancel

**Notification (Client → Agent):**
```json
{
  "jsonrpc": "2.0",
  "method": "session/cancel",
  "params": {
    "sessionId": "sess_abc123def456"
  }
}
```

### 4.7 Session/Load

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "session/load",
  "params": {
    "sessionId": "sess_789xyz",
    "cwd": "/home/user/project",
    "mcpServers": []
  }
}
```

Agent replays conversation via `session/update` notifications, then responds:
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": null
}
```

### 4.8 Session/Set_Mode

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "session/set_mode",
  "params": {
    "sessionId": "sess_abc123def456",
    "modeId": "plan"
  }
}
```

### 4.9 File System Methods (Agent → Client, Optional)

**fs/read_text_file:**
```json
{
  "jsonrpc": "2.0",
  "id": 200,
  "method": "fs/read_text_file",
  "params": {
    "sessionId": "sess_abc123def456",
    "path": "/home/user/project/src/main.rs",
    "line": 1,
    "limit": 100
  }
}
```

**fs/write_text_file:**
```json
{
  "jsonrpc": "2.0",
  "id": 201,
  "method": "fs/write_text_file",
  "params": {
    "sessionId": "sess_abc123def456",
    "path": "/home/user/project/output.txt",
    "content": "Analysis results..."
  }
}
```

### 4.10 Terminal Methods (Agent → Client, Optional)

**terminal/create:**
```json
{
  "jsonrpc": "2.0",
  "id": 300,
  "method": "terminal/create",
  "params": {
    "sessionId": "sess_abc123def456",
    "command": "grep",
    "args": ["-rn", "unsafe", "src/"],
    "cwd": "/home/user/project",
    "env": [],
    "outputByteLimit": 1048576
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 300,
  "result": { "terminalId": "term_001" }
}
```

---

## 5. Session Lifecycle

```
Client                              Agent (Copilot CLI)
  |                                     |
  |--- initialize ---------------------->|
  |<-- initialize response (caps) ------|
  |                                     |
  |--- authenticate (if needed) -------->|
  |<-- authenticate response ------------|
  |                                     |
  |--- session/new --------------------->|
  |<-- session/new response (sessionId) -|
  |                                     |
  |--- session/prompt ------------------>|   ← BLOCKS until turn complete
  |<-- session/update (agent_message) ---|   ← Streamed notifications
  |<-- session/update (tool_call) -------|
  |<-- session/request_permission -------|   ← Agent asks permission 
  |--- permission response ------------->|   ← Client approves/denies
  |<-- session/update (tool_call_update)-|
  |<-- session/update (agent_message) ---|
  |<-- session/prompt response ----------|   ← Turn ends
  |                                     |
  |--- session/prompt (next turn) ------>|   ← Continue conversation
  |    ...                              |
  |                                     |
  |  (kill subprocess to end)           |
```

**Key insight for DevDev:** The `session/prompt` call **blocks** until the agent finishes its entire turn (which may include multiple tool calls, LLM round-trips, etc.). During this time, `session/update` notifications and `session/request_permission` requests arrive **interleaved** on the same JSON-RPC stream. The client must handle these concurrently while waiting for the final `session/prompt` response.

---

## 6. Tool Calls & Permission Requests

### How Tool Calls Work in ACP

1. Agent decides to call a tool (e.g., `bash`, `edit`, `view`, `create`).
2. Agent sends `session/update` with `"sessionUpdate": "tool_call"`, status `"pending"`.
3. Agent sends `session/request_permission` to ask client to approve.
4. Client responds with `"selected"` + option ID or `"cancelled"`.
5. If approved, agent sends `session/update` with `"tool_call_update"`, status `"in_progress"`.
6. Agent executes the tool.
7. Agent sends `session/update` with `"tool_call_update"`, status `"completed"` + content.
8. Agent sends tool result back to LLM for next reasoning step.

### Permission Option Kinds
- `allow_once` — Allow this operation only this time
- `allow_always` — Allow and remember the choice
- `reject_once` — Reject this time
- `reject_always` — Reject and remember

### Permission Request Fields (from SDK)
The SDK's `PermissionRequest` object reveals the internal structure:
```typescript
{
  kind: "shell" | "write" | "read" | "mcp" | "custom-tool" | "url" | "memory" | "hook",
  toolCallId: string,
  toolName: string,
  fileName?: string,        // for "write" kind
  fullCommandText?: string  // for "shell" kind
}
```

### Permission Result Kinds (from SDK)
- `"approved"` — Allow the tool
- `"denied-interactively-by-user"` — User explicitly denied
- `"denied-no-approval-rule-and-could-not-request-from-user"` — No rule, no user
- `"denied-by-rules"` — Denied by policy rule
- `"denied-by-content-exclusion-policy"` — Content exclusion
- `"no-result"` — Leave unanswered (protocol v1 only)

### ToolCallUpdate Fields
```typescript
{
  toolCallId: string,       // required - links to original tool_call
  status?: "pending" | "in_progress" | "completed" | "failed",
  title?: string,
  kind?: "read" | "edit" | "delete" | "move" | "search" | "execute" | "think" | "fetch" | "switch_mode" | "other",
  content?: ToolCallContent[],
  locations?: ToolCallLocation[],
  rawInput?: object,
  rawOutput?: object
}
```

### ToolCallContent Types
- `content` — Standard content block (text, image, resource)
- `diff` — File modification `{ path, oldText, newText }`
- `terminal` — Embedded terminal by ID `{ terminalId }`

---

## 7. Hooks System (File-Based)

File-based hooks live in `.github/hooks/*.json` and execute as shell scripts. They are **separate** from the ACP protocol — they run inside the Copilot CLI process, not over the ACP wire.

### Configuration Format
```json
{
  "version": 1,
  "hooks": {
    "sessionStart": [...],
    "sessionEnd": [...],
    "userPromptSubmitted": [...],
    "preToolUse": [...],
    "postToolUse": [...],
    "agentStop": [...],
    "subagentStop": [...],
    "errorOccurred": [...]
  }
}
```

### preToolUse Hook Input (stdin JSON)
```json
{
  "timestamp": 1704614600000,
  "cwd": "/path/to/project",
  "toolName": "bash",
  "toolArgs": "{\"command\":\"rm -rf dist\",\"description\":\"Clean build directory\"}"
}
```

### preToolUse Hook Output (stdout JSON)
```json
{
  "permissionDecision": "allow",
  "permissionDecisionReason": "Safe operation"
}
```
- `permissionDecision`: `"allow"` | `"deny"` | `"ask"` (only `"deny"` is currently enforced)

### postToolUse Hook Input
```json
{
  "timestamp": 1704614700000,
  "cwd": "/path/to/project",
  "toolName": "bash",
  "toolArgs": "{\"command\":\"npm test\"}",
  "toolResult": {
    "resultType": "success",
    "textResultForLlm": "All tests passed (15/15)"
  }
}
```
`resultType`: `"success"` | `"failure"` | `"denied"`

### sessionStart Hook Input
```json
{
  "timestamp": 1704614400000,
  "cwd": "/path/to/project",
  "source": "new",
  "initialPrompt": "Create a new feature"
}
```
`source`: `"new"` | `"resume"` | `"startup"`

### sessionEnd Hook Input
```json
{
  "timestamp": 1704618000000,
  "cwd": "/path/to/project",
  "reason": "complete"
}
```
`reason`: `"complete"` | `"error"` | `"abort"` | `"timeout"` | `"user_exit"`

### errorOccurred Hook Input
```json
{
  "timestamp": 1704614800000,
  "cwd": "/path/to/project",
  "error": { "message": "Network timeout", "name": "TimeoutError", "stack": "..." }
}
```

### Hook Environment Variables
Hooks receive these env vars:
- `PLUGIN_ROOT`, `COPILOT_PLUGIN_ROOT`, `CLAUDE_PLUGIN_ROOT` — plugin directory
- `CLAUDE_PROJECT_DIR` — project directory
- `COPILOT_CLI=1` — set in all subprocesses

---

## 8. SDK Hooks (Programmatic)

The `@github/copilot-sdk` provides programmatic hooks that integrate at a higher level than file-based hooks. These are relevant if DevDev uses the SDK rather than raw ACP.

```typescript
hooks: {
  onPreToolUse: async (input, invocation) => {
    // input.toolName: string
    // input.toolArgs: object
    return {
      permissionDecision: "allow" | "deny" | "ask",
      modifiedArgs: { ... },           // optionally modify arguments
      additionalContext: "string",     // extra context for model
    };
  },

  onPostToolUse: async (input, invocation) => {
    // input.toolName, input.toolArgs, input.toolResult
    return {
      additionalContext: "Post-execution notes",
    };
  },

  onUserPromptSubmitted: async (input, invocation) => {
    // input.prompt: string
    return {
      modifiedPrompt: "modified prompt text",
    };
  },

  onSessionStart: async (input, invocation) => {
    // input.source: "startup" | "resume" | "new"
    return {
      additionalContext: "Session context...",
    };
  },

  onSessionEnd: async (input, invocation) => {
    // input.reason: string
  },

  onErrorOccurred: async (input, invocation) => {
    // input.error, input.errorContext
    return {
      errorHandling: "retry" | "skip" | "abort",
    };
  },
}
```

---

## 9. Authentication

### Methods (in priority order)
1. **`githubToken` option** — Pass token directly to SDK/CLI
2. **Environment variables** — `GH_TOKEN` > `GITHUB_TOKEN` > `COPILOT_GITHUB_TOKEN`
3. **`copilot login`** — Interactive OAuth device flow
4. **`gh auth` session** — Reuse existing GitHub CLI auth
5. **ACP terminal-auth** — ACP-specific auth flow (added v0.0.401)

### PAT Requirements
- Personal Access Token needs **"Copilot Requests"** permission scope.

### ACP Auth Flow
- Agent may return error code `-32000` (auth required) on `session/new`
- Agent advertises `authMethods` in `initialize` response
- Client calls `authenticate` with chosen method ID
- After successful auth, `session/new` succeeds

### For DevDev (Daemon Mode)
Use `COPILOT_GITHUB_TOKEN` env var or `GH_TOKEN`. Do NOT rely on interactive auth flows.

---

## 10. Copilot-Specific ACP Extensions

These are features Copilot CLI adds beyond the base ACP spec, discovered primarily from the changelog:

| Feature | Version | Details |
|---------|---------|---------|
| ACP server mode | v0.0.397 | `--acp` flag |
| Permission flags in ACP | v0.0.400 | `--allow-all`, `--allow-all-tools`, `--yolo` work with `--acp` |
| Model changes in ACP | v0.0.400 | Model switching supported |
| ACP terminal-auth | v0.0.401 | Authentication flow for ACP clients |
| Agent/plan session modes | v0.0.402 | `session/set_mode` for agent vs plan mode |
| MCP config applies to ACP | v0.0.402 | MCP servers available in ACP sessions |
| Model info with usage multiplier | v0.0.403 | Config options include cost info |
| `tools.list` RPC | v0.0.407 | Query available built-in tools |
| Session loading in ACP | v0.0.410 | `session/load` to resume sessions |
| ACP model reasoning effort | v0.0.421 | Configure low/medium/high/xhigh |
| `--output-format json` JSONL | v0.0.422 | JSON Lines output in prompt mode |
| `exitPlanMode.request` | v0.0.422 | Protocol method to exit plan mode |
| `session.shell.exec/kill` | v1.0.4 | Shell execution over ACP |
| `mcp.config.*` RPCs | v1.0.15 | Manage MCP servers at runtime |
| PermissionRequest hook | v1.0.16 | File-based hook for permissions |
| `permissionDecision: "allow"` | v1.0.18 | Suppresses tool approval prompt |
| `modifiedArgs` + `additionalContext` | v1.0.24 | preToolUse can modify args and inject context |
| ACP clients provide MCP servers | v1.0.25 | Client-side MCP servers |
| Localhost-only binding | v1.0.26 | Security: TCP ACP binds only to 127.0.0.1 |

### --output-format json

When using `--output-format json` in prompt mode (not ACP mode), Copilot CLI emits JSONL. This is a **separate** mechanism from ACP — useful for simple scripting but not for the bidirectional RPC that DevDev needs.

---

## 11. SDKs & Libraries

### Official Copilot SDK (`@github/copilot-sdk`)
- **npm:** `@github/copilot-sdk` v0.2.2 (110K weekly downloads)
- **Repo:** `github.com/github/copilot-sdk`
- **Languages:** Node.js/TypeScript (primary), Python, Go, .NET, Java
- **Architecture:** Wraps the Copilot CLI subprocess. Handles JSON-RPC, session management, hooks, permissions.
- **Key classes:** `CopilotClient` (connection), `CopilotSession` (conversation)

### ACP Protocol Libraries (from Zed)
- **TypeScript:** `@agentclientprotocol/sdk` — used in the official docs example
- **Kotlin:** Available at agentclientprotocol.com/libraries/kotlin
- **Rust:** Not yet available as an official library (as of research date)

### Multi-Language SDKs (Copilot SDK Blog, Apr 2, 2026)
```
Node.js/TypeScript: npm install @github/copilot-sdk
Python:            pip install github-copilot-sdk
Go:                go get github.com/github/copilot-sdk/go
.NET:              dotnet add package GitHub.Copilot.SDK
Java:              Maven
```

### For DevDev (Rust)
No official Rust SDK exists. Options:
1. **Implement raw JSON-RPC client in Rust** — parse NDJSON, handle the ACP methods directly. The protocol is simple enough.
2. **Use the Go SDK from Rust via FFI** — overly complex.
3. **Wrap the `@agentclientprotocol/sdk` TypeScript lib** — unnecessary indirection.
4. **Wait for a Rust ACP library** — uncertain timeline.

**Recommendation: Option 1.** The protocol is well-documented JSON-RPC 2.0. A Rust implementation needs: an NDJSON reader/writer, serde structs for the ~20 message types, and async handling for interleaved requests/notifications.

---

## 12. Error Handling

### JSON-RPC Error Codes
| Code | Meaning |
|------|---------|
| `-32700` | Parse error (invalid JSON) |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32603` | Internal error |
| `-32000` | **Authentication required** (ACP-specific) |
| `-32002` | Resource not found |

### Copilot CLI Error Behavior (from Issue #222)
- Unauthenticated requests return `{ "code": -32603 }` (Copilot originally used wrong code; should be `-32000` per ACP spec — reported by josevalim, fixed in later versions)

### Cancellation Error Handling
When `session/cancel` is sent:
- Agent MUST catch internal exceptions from aborted operations
- Agent MUST respond to `session/prompt` with `stopReason: "cancelled"` (not an error)
- Client MUST respond to all pending `session/request_permission` with `"cancelled"` outcome
- Agent MAY send final `session/update` notifications before the response

---

## 13. Protocol Versioning & Capabilities

### Protocol Version
- Type: `uint16`
- Current version: **1**
- Only bumped for **breaking changes**. Non-breaking additions use capabilities.

### Capability Negotiation
During `initialize`, client and agent exchange capabilities:

**Client capabilities:**
```json
{
  "fs": {
    "readTextFile": true,   // client can serve file reads
    "writeTextFile": true   // client can serve file writes
  },
  "terminal": true          // client supports terminal/* methods
}
```

**Agent capabilities:**
```json
{
  "loadSession": true,
  "mcpCapabilities": { "http": true, "sse": false },
  "promptCapabilities": { "image": true, "audio": false, "embeddedContext": true },
  "sessionCapabilities": { "list": {} }
}
```

### Extensibility
- Custom methods: prefix with `_` (e.g., `_devdev/execute`)
- Custom metadata: use `_meta` field on any object
- Custom capabilities: advertise during initialization

---

## 14. Architecture Decision: How DevDev Should Integrate

### Option A: ACP Client with `session/request_permission` (RECOMMENDED)

DevDev spawns `copilot --acp --allow-all-tools` and implements an ACP client:

1. Intercept **`session/request_permission`** — this fires for every tool call.
   - Inspect `toolCall.rawInput` to get the command.
   - Auto-approve or deny based on DevDev policy.
   - **Limitation:** Cannot modify the tool execution itself — can only approve/deny.

2. Use **`terminal/*` client capabilities** — advertise terminal support:
   - Agent sends `terminal/create` with command details.
   - DevDev routes the command through the virtual engine.
   - Returns output via `terminal/output`.
   - **This is the cleanest interception point** — DevDev becomes the execution backend.

3. Use **`fs/*` client capabilities** — advertise filesystem support:
   - Agent sends `fs/read_text_file` / `fs/write_text_file`.
   - DevDev serves from VFS.

### Option B: SDK with Custom Tools + Hooks

Use the Copilot SDK (Node.js/Go/Python wrapper) instead of raw ACP:

1. SDK `onPreToolUse` hook can **modify arguments** and add context.
2. SDK `onPermissionRequest` can approve/deny with detailed reasons.
3. SDK custom tools (`defineTool`) can **replace built-in tools**.
4. **Problem:** SDK is not available in Rust. Would require a polyglot architecture.

### Option C: File-Based Hooks Only

Put hook scripts in `.github/hooks/` that route to DevDev:

1. `preToolUse` hook receives command, calls DevDev virtual engine, returns result.
2. **Problem:** Can only `deny` — cannot **replace** execution with virtual output.
3. **Problem:** No bidirectional communication — hooks are fire-and-forget shell scripts.

### Option D: MCP Server as Execution Backend

DevDev starts an MCP server that provides virtual tools. Pass this to `session/new`:

1. Agent discovers DevDev tools via MCP.
2. Agent calls DevDev tools instead of built-in `bash`/`edit`/`view`.
3. **Problem:** Agent may still use built-in tools. No guarantee of exclusive routing.

### Recommended Architecture for DevDev

**Option A with terminal/fs capabilities** is the cleanest fit:

```
DevDev (Rust ACP Client)
  │
  ├── Spawn: copilot --acp --allow-all-tools
  │
  ├── initialize: advertise { terminal: true, fs: { readTextFile: true, writeTextFile: true } }
  │
  ├── session/new: create session with cwd pointing to VFS root
  │
  ├── session/prompt: send PR diff + preferences
  │
  ├── Handle interleaved messages:
  │   ├── session/update → stream to log / display
  │   ├── session/request_permission → auto-approve virtual ops, deny escapes
  │   ├── terminal/create → route to ShellSession.execute()
  │   ├── terminal/output → return buffered output
  │   ├── terminal/wait_for_exit → return when command completes
  │   ├── terminal/kill → abort command
  │   ├── terminal/release → cleanup
  │   ├── fs/read_text_file → VFS read
  │   └── fs/write_text_file → VFS write
  │
  └── session/prompt response → extract verdict
```

**Key advantage:** By implementing terminal and filesystem capabilities, DevDev becomes the **execution environment** for the agent. The agent doesn't know or care that it's running in a sandbox — it uses standard ACP methods, and DevDev serves everything from the virtual engine.

**Important caveat:** Need to verify through testing whether Copilot CLI actually uses `terminal/create` and `fs/*` ACP methods when the client advertises those capabilities, or if it still executes tools internally. This needs hands-on testing with `--acp` mode.

---

## 15. Sources & Confidence Levels

| Source | URL | Confidence |
|--------|-----|------------|
| ACP Spec — Overview | agentclientprotocol.com/protocol/overview | ✅ Authoritative |
| ACP Spec — Schema | agentclientprotocol.com/protocol/schema | ✅ Authoritative |
| ACP Spec — Session Setup | agentclientprotocol.com/protocol/session-setup | ✅ Authoritative |
| ACP Spec — Prompt Turn | agentclientprotocol.com/protocol/prompt-turn | ✅ Authoritative |
| Copilot CLI ACP Docs | docs.github.com/.../acp-server | ✅ Official |
| Copilot CLI Hooks Ref | docs.github.com/.../hooks-configuration | ✅ Official |
| About Hooks | docs.github.com/.../about-hooks | ✅ Official |
| Copilot SDK npm | npmjs.com/package/@github/copilot-sdk | ✅ Official |
| Copilot SDK Blog | github.blog/changelog/2026-04-02-copilot-sdk-in-public-preview | ✅ Official |
| Issue #222 | github.com/github/copilot-cli/issues/222 | ✅ GitHub Staff (devm33) |
| Issue #845 | github.com/github/copilot-cli/issues/845 | ✅ Detailed test code |
| Changelog.md | github.com/github/copilot-cli/blob/main/changelog.md | ✅ Official |
| Copilot-specific ACP internals | Inferred from changelog entries | ⚠️ Inferred — may not be public API |

### What IS vs ISN'T Publicly Documented

**Fully documented:**
- ACP base protocol (all methods, schemas, lifecycle)
- Copilot CLI `--acp` flag and transport modes
- File-based hooks (input/output JSON formats)
- SDK API (CopilotClient, CopilotSession, permissions, hooks, tools)
- Authentication methods

**Partially documented (changelog only, no formal schema docs):**
- `session.shell.exec` / `session.shell.kill` RPC methods
- `tools.list` RPC
- `exitPlanMode.request` method
- `mcp.config.*` RPCs
- Model/reasoning effort config options format
- Exact Copilot CLI `agentCapabilities` response

**Not documented (needs hands-on testing):**
- Whether Copilot CLI actually delegates to `terminal/create` when client advertises terminal capability, or executes internally
- Whether `fs/read_text_file` / `fs/write_text_file` are used by Copilot CLI ACP vs just reading files directly
- Exact `rawInput` schema for each built-in tool (bash, edit, view, create, etc.)
- The internal tool registry (what `tools.list` returns)
- Detailed `session/update` notification frequency and ordering guarantees

---

## Appendix: Quick-Reference for `devdev-acp` Crate Implementation

### Minimal Rust ACP Client Skeleton

```rust
// Types needed in protocol.rs:
enum AcpRequest {
    Initialize(InitializeParams),
    Authenticate(AuthenticateParams),
    SessionNew(NewSessionParams),
    SessionPrompt(PromptParams),
    // Client → Agent
}

enum AcpNotification {
    SessionCancel { session_id: String },
    // Client → Agent  
}

enum AgentRequest {
    RequestPermission(RequestPermissionParams),
    FsReadTextFile(ReadTextFileParams),
    FsWriteTextFile(WriteTextFileParams),
    TerminalCreate(CreateTerminalParams),
    TerminalOutput(TerminalOutputParams),
    TerminalWaitForExit(WaitForExitParams),
    TerminalKill(KillTerminalParams),
    TerminalRelease(ReleaseTerminalParams),
    // Agent → Client
}

enum AgentNotification {
    SessionUpdate { session_id: String, update: SessionUpdate },
    // Agent → Client
}

enum SessionUpdate {
    AgentMessageChunk { content: ContentBlock },
    AgentThoughtChunk { content: ContentBlock },
    UserMessageChunk { content: ContentBlock },
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    Plan { entries: Vec<PlanEntry> },
    AvailableCommandsUpdate { available_commands: Vec<AvailableCommand> },
    CurrentModeUpdate { current_mode_id: String },
    ConfigOptionUpdate { config_options: Vec<SessionConfigOption> },
    SessionInfoUpdate { title: Option<String> },
}

enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}
```

### Spawn Command
```rust
let child = Command::new("copilot")
    .args(["--acp", "--stdio"])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())  // or pipe for logging
    .spawn()?;
```

### NDJSON I/O
```rust
// Write: serialize to JSON + newline
fn send(writer: &mut impl Write, msg: &JsonRpcMessage) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, msg)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

// Read: line-by-line JSON parsing
fn recv(reader: &mut impl BufRead) -> io::Result<JsonRpcMessage> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(serde_json::from_str(&line)?)
}
```
