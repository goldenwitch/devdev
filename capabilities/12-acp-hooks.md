---
id: acp-hooks
title: "ACP Hook Handlers (Terminal, FS, Permissions)"
status: done
type: composition
phase: 4
crate: devdev-acp
priority: P0
depends-on: [acp-client, shell-executor, vfs-core]
effort: M
---

# 12 — ACP Hook Handlers (Terminal, FS, Permissions)

> **Status note (2026-04-22, post-P2-06 PoC):** This capability was built on the assumption that every `terminal/create` and `fs/*` call from the Copilot CLI would route through `SandboxHandler`. The P2-06 PoC revealed the prod invocation is `copilot --acp --allow-all-tools`, which delegates all tool execution to Copilot's own internal tool bundle (shell, fs, web, etc.) running directly against the mounted workspace. **In that mode, the handlers below are never called.** The code still compiles, passes its tests, and remains in the tree as a **safety-net for a hypothetical `--strict-sandbox` profile** — same binary, no `--allow-all-tools`, every tool call forced through DevDev. No caller exercises that profile today. If DevDev ever needs DevDev-specific tools (task queries, ledger lookups), the path is MCP ([capability 28](28-mcp-tool-injection.md)), not these hooks.

Implement the `AcpHandler` trait — the business logic that runs when the Copilot CLI agent makes requests. This is where the sandbox enforcement happens: every `terminal/create` request routes through the virtual shell, every `fs/*` request routes through the VFS, and every permission request gets auto-approved (it's all virtual — nothing to protect against).

This capability is the critical junction where ACP meets the sandbox.

## Scope

**In:**
- `terminal/create` → parse command, execute via `ShellSession`, return output
- `terminal/output` → return buffered output from a previous terminal command
- `terminal/wait_for_exit` → return exit code (commands run synchronously, so already done)
- `terminal/kill` → no-op (commands already completed)
- `terminal/release` → cleanup terminal state
- `fs/read_text_file` → read from VFS
- `fs/write_text_file` → write to VFS
- `session/request_permission` → auto-approve virtual operations, deny sandbox escapes
- `session/update` → log/trace (agent messages, tool call updates, plans)
- Timeout enforcement: 30s per command execution

**Out:**
- ACP transport (that's `11-acp-client`)
- Shell parsing/execution internals (that's `09-shell-executor`)
- Session lifecycle orchestration (that's `13-sandbox-integration`)

## Interface

```rust
pub struct SandboxHandler {
    shell: Mutex<ShellSession>,
    vfs: Arc<RwLock<dyn VirtualFilesystem>>,
    terminals: Mutex<HashMap<String, TerminalState>>,
    config: HandlerConfig,
    trace_log: Arc<dyn TraceLogger>,
}

struct TerminalState {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i32,
    completed: bool,
}

pub struct HandlerConfig {
    pub command_timeout: Duration,   // default: 30s
    pub max_output_bytes: u64,       // default: 1 MB
}
```

## Handler Implementations

### `terminal/create`

```rust
async fn on_terminal_create(&self, params: CreateTerminalParams) -> Result<CreateTerminalResult> {
    // 1. Build command string from params.command + params.args
    let cmd = format_command(&params.command, &params.args);
    
    // 2. Execute via ShellSession with timeout
    let result = tokio::time::timeout(
        self.config.command_timeout,
        tokio::task::spawn_blocking(|| self.shell.lock().execute(&cmd))
    ).await??;
    
    // 3. Store result in terminal state
    let terminal_id = generate_terminal_id();
    self.terminals.lock().insert(terminal_id.clone(), TerminalState {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        completed: true,
    });
    
    // 4. Log
    self.trace_log.tool_executed(&cmd, &result);
    
    Ok(CreateTerminalResult { terminal_id })
}
```

### `terminal/output`

Return the buffered stdout from the stored terminal state. Respect `params.output_byte_limit`.

### `terminal/wait_for_exit`

Commands execute synchronously, so by the time `terminal/create` returns, the command is done. Return the stored exit code immediately.

### `fs/read_text_file`

```rust
async fn on_fs_read(&self, params: ReadTextFileParams) -> Result<ReadTextFileResult> {
    let content = self.vfs.read().read(Path::new(&params.path))?;
    let text = String::from_utf8_lossy(&content);
    
    // Handle line/limit params
    let lines: Vec<&str> = text.lines().collect();
    let start = params.line.unwrap_or(1) as usize - 1;
    let limit = params.limit.unwrap_or(lines.len() as u32) as usize;
    let selected: String = lines[start..min(start + limit, lines.len())].join("\n");
    
    Ok(ReadTextFileResult { content: selected })
}
```

### `fs/write_text_file`

```rust
async fn on_fs_write(&self, params: WriteTextFileParams) -> Result<()> {
    self.vfs.write().write(
        Path::new(&params.path),
        params.content.as_bytes(),
    )?;
    Ok(())
}
```

### `session/request_permission`

```rust
async fn on_permission_request(&self, params: PermissionRequestParams) -> PermissionResponse {
    // Everything runs in the sandbox — auto-approve
    let allow_option = params.options.iter()
        .find(|o| matches!(o.kind, PermissionKind::AllowOnce))
        .or_else(|| params.options.first());
    
    PermissionResponse {
        outcome: PermissionOutcome::Selected {
            option_id: allow_option.map(|o| o.option_id.clone()).unwrap_or_default(),
        },
    }
}
```

### `session/update`

Log all updates. Key events to trace:
- `AgentMessageChunk` → accumulate agent response text
- `ToolCall` → log tool name, status, raw input
- `ToolCallUpdate` → log status transitions, completion content
- `Plan` → log plan entries

## Security: Sandbox Escape Detection

The handler should detect and block operations that could escape the sandbox:

| Escape vector | Detection | Response |
|---------------|-----------|----------|
| Network commands (`curl`, `wget`, `nc`) | Check command name in `terminal/create` | Return error: "Network access not available in sandbox" |
| Absolute host paths in `fs/*` | Check if path starts with VFS root | Return error or remap to VFS |
| Very large output | `output_byte_limit` enforcement | Truncate with warning |

Note: In practice, the WASM tools can't make network calls (WASI doesn't allow it), and the VFS doesn't have host paths. The `fs/*` handler is the main vector to watch — the agent might request `/etc/passwd` or similar.

## Files

```
crates/devdev-acp/src/hooks.rs       — SandboxHandler implementing AcpHandler
crates/devdev-acp/src/terminal.rs    — terminal state management
crates/devdev-acp/src/trace.rs       — TraceLogger trait + implementations
```

## Acceptance Criteria

- [ ] `terminal/create` with `grep -rn TODO src/` → executes in shell, returns terminal_id
- [ ] `terminal/output` for that terminal → returns grep results
- [ ] `terminal/wait_for_exit` → returns exit code 0
- [ ] `fs/read_text_file` for existing VFS file → returns content
- [ ] `fs/read_text_file` with `line` and `limit` → returns correct slice
- [ ] `fs/write_text_file` → file exists in VFS afterward
- [ ] `session/request_permission` → auto-approves with `allow_once`
- [ ] Command exceeding 30s timeout → error returned (not hang)
- [ ] All `session/update` notifications are logged
- [ ] `terminal/create` with `curl http://...` → error (if curl exists) or command-not-found (if it doesn't)
