//! [`SandboxHandler`] — the concrete [`AcpHandler`] implementation that
//! routes ACP terminal/fs/permission requests into the DevDev sandbox.
//!
//! Design invariants:
//!
//! * Everything the agent sees is virtual. `terminal/*` runs against
//!   [`ShellSession`]. `fs/*` reads/writes [`MemFs`] through the same
//!   `Arc<Mutex<_>>` the shell holds, so a write is visible to the next
//!   `cat` in the same session.
//! * Sandbox escape attempts (known network commands) are rejected at
//!   [`SandboxHandler::on_terminal_create`] with a typed [`RpcError`]
//!   instead of reaching the shell.
//! * Commands run synchronously inside [`tokio::task::spawn_blocking`]
//!   and are wrapped by a caller-configurable timeout. A timed-out
//!   command returns [`error_codes::INTERNAL_ERROR`] and is not
//!   recorded in the terminal map, so `terminal/output` later fails
//!   with "terminal not found" rather than blocking forever.
//! * Output capture is bounded by [`HandlerConfig::max_output_bytes`];
//!   larger buffers are truncated and reported with `truncated: true`.

use std::cmp::min;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex as AsyncMutex;

use devdev_shell::ShellSession;
use devdev_vfs::MemFs;

use crate::handler::{AcpHandler, HandlerResult};
use crate::protocol::{RpcError, error_codes};
use crate::terminal::ShellWorker;
use crate::trace::{NoopTraceLogger, TraceEvent, TraceLogger};
use crate::types::{
    CreateTerminalParams, CreateTerminalResult, KillTerminalParams, PermissionKind,
    PermissionOutcome, PermissionRequestParams, PermissionResponse, ReadTextFileParams,
    ReadTextFileResult, ReleaseTerminalParams, SessionUpdateParams, TerminalOutputParams,
    TerminalOutputResult, WaitForExitParams, WaitForExitResult, WriteTextFileParams,
};

/// Default per-command wall clock.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Default max bytes returned for any single `terminal/output` or
/// `fs/read_text_file` call.
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 1 << 20; // 1 MiB

/// Commands known to reach the real network. Rejected at `terminal/create`
/// before they can touch the shell dispatcher.
const BLOCKED_COMMANDS: &[&str] = &[
    "curl", "wget", "nc", "ncat", "ssh", "scp", "rsync", "ftp", "sftp", "telnet", "ping",
];

/// Configuration knobs exposed to embedders. Finite everywhere.
#[derive(Debug, Clone)]
pub struct HandlerConfig {
    pub command_timeout: Duration,
    pub max_output_bytes: u64,
}

impl Default for HandlerConfig {
    fn default() -> Self {
        Self {
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

/// State recorded for a single `terminal_id`.
#[derive(Debug, Clone)]
struct TerminalState {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i32,
    truncated: bool,
}

/// Default queue depth for the [`ShellWorker`] command channel.
pub const DEFAULT_SHELL_CHANNEL_DEPTH: usize = 16;

/// Concrete ACP handler backed by the DevDev shell and VFS.
///
/// Construct with [`SandboxHandler::new`] and hand an `Arc<SandboxHandler>`
/// to [`crate::client::AcpClient::connect_transport`].
///
/// The `ShellSession` is pinned to a dedicated worker thread because
/// `dyn VirtualGit` is intentionally `!Send`. All `terminal/create`
/// requests are forwarded over an mpsc channel to that worker.
pub struct SandboxHandler {
    shell: ShellWorker,
    vfs: Arc<std::sync::Mutex<MemFs>>,
    terminals: AsyncMutex<HashMap<String, TerminalState>>,
    config: HandlerConfig,
    trace: Arc<dyn TraceLogger>,
    next_id: AtomicU64,
}

impl SandboxHandler {
    /// Build a new handler. The `build_shell` closure constructs the
    /// `ShellSession` on the pinned worker thread (see [`ShellWorker`]
    /// for why). The `vfs` here must be a clone of the same
    /// `Arc<Mutex<MemFs>>` the closure will hand to `ShellSession::new`
    /// so that `fs/write_text_file` is visible to the next
    /// `terminal/create` in the same session.
    pub fn new<F>(build_shell: F, vfs: Arc<std::sync::Mutex<MemFs>>) -> Self
    where
        F: FnOnce() -> ShellSession + Send + 'static,
    {
        Self::with_config(build_shell, vfs, HandlerConfig::default())
    }

    pub fn with_config<F>(
        build_shell: F,
        vfs: Arc<std::sync::Mutex<MemFs>>,
        config: HandlerConfig,
    ) -> Self
    where
        F: FnOnce() -> ShellSession + Send + 'static,
    {
        Self {
            shell: ShellWorker::spawn(build_shell, DEFAULT_SHELL_CHANNEL_DEPTH),
            vfs,
            terminals: AsyncMutex::new(HashMap::new()),
            config,
            trace: Arc::new(NoopTraceLogger),
            next_id: AtomicU64::new(1),
        }
    }

    /// Replace the trace logger. Default is a no-op.
    pub fn with_trace(mut self, trace: Arc<dyn TraceLogger>) -> Self {
        self.trace = trace;
        self
    }

    pub fn config(&self) -> &HandlerConfig {
        &self.config
    }

    fn next_terminal_id(&self) -> String {
        let n = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!("term-{n}")
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn format_command(command: &str, args: &[String]) -> String {
    if args.is_empty() {
        command.to_owned()
    } else {
        let mut s = String::with_capacity(command.len() + args.iter().map(|a| a.len() + 1).sum::<usize>());
        s.push_str(command);
        for a in args {
            s.push(' ');
            s.push_str(a);
        }
        s
    }
}

fn is_blocked(command: &str) -> bool {
    let head = command.split_whitespace().next().unwrap_or(command);
    let name = std::path::Path::new(head)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(head);
    BLOCKED_COMMANDS.contains(&name)
}

fn truncate_to(bytes: &[u8], cap: usize) -> (Vec<u8>, bool) {
    if bytes.len() > cap {
        (bytes[..cap].to_vec(), true)
    } else {
        (bytes.to_vec(), false)
    }
}

fn rpc_invalid_params(msg: impl Into<String>) -> RpcError {
    RpcError {
        code: error_codes::INVALID_PARAMS,
        message: msg.into(),
        data: None,
    }
}

fn rpc_internal(msg: impl Into<String>) -> RpcError {
    RpcError {
        code: error_codes::INTERNAL_ERROR,
        message: msg.into(),
        data: None,
    }
}

// ── handler impl ─────────────────────────────────────────────────────────

#[async_trait]
impl AcpHandler for SandboxHandler {
    async fn on_permission_request(
        &self,
        params: PermissionRequestParams,
    ) -> HandlerResult<PermissionResponse> {
        // Everything is virtual. Pick an AllowOnce option, else first option.
        let option_id = params
            .options
            .iter()
            .find(|o| matches!(o.kind, PermissionKind::AllowOnce))
            .or_else(|| params.options.first())
            .map(|o| o.option_id.clone())
            .unwrap_or_default();

        self.trace.record(TraceEvent::PermissionGranted {
            tool_call_id: params.tool_call.tool_call_id.clone(),
            option_id: option_id.clone(),
        });

        Ok(PermissionResponse {
            outcome: PermissionOutcome::Selected { option_id },
        })
    }

    async fn on_terminal_create(
        &self,
        params: CreateTerminalParams,
    ) -> HandlerResult<CreateTerminalResult> {
        let cmd = format_command(&params.command, &params.args);

        if is_blocked(&params.command) {
            let reason = format!(
                "network command `{}` is blocked in the sandbox",
                params.command
            );
            self.trace.record(TraceEvent::TerminalRejected {
                command: cmd.clone(),
                reason: reason.clone(),
            });
            return Err(rpc_internal(reason));
        }

        // Bound on output per this terminal. Each side (stdout+stderr) gets
        // the full budget; the per-call `terminal/output` truncates again.
        let max_bytes = params
            .output_byte_limit
            .unwrap_or(self.config.max_output_bytes)
            .min(self.config.max_output_bytes) as usize;

        let timeout = self.config.command_timeout;

        // Dispatch to the pinned shell worker. timeout() races the reply
        // against the wall clock; on timeout we do NOT cancel the worker
        // (it stays pinned and keeps serving the next request) but we do
        // report the failure up to the agent.
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(timeout, self.shell.execute(cmd.clone()))
            .await
            .map_err(|_| {
                let reason = format!("command timed out after {}s", timeout.as_secs());
                self.trace.record(TraceEvent::TerminalRejected {
                    command: cmd.clone(),
                    reason: reason.clone(),
                });
                rpc_internal(reason)
            })?
            .ok_or_else(|| rpc_internal("shell worker has shut down"))?;
        let duration_ms = started.elapsed().as_millis() as u64;

        let (stdout, out_trunc) = truncate_to(&result.stdout, max_bytes);
        let (stderr, err_trunc) = truncate_to(&result.stderr, max_bytes);
        let truncated = out_trunc || err_trunc;

        let terminal_id = self.next_terminal_id();
        self.terminals.lock().await.insert(
            terminal_id.clone(),
            TerminalState {
                stdout,
                stderr,
                exit_code: result.exit_code,
                truncated,
            },
        );

        self.trace.record(TraceEvent::TerminalCreated {
            terminal_id: terminal_id.clone(),
            command: cmd,
            exit_code: result.exit_code,
            duration_ms,
        });

        Ok(CreateTerminalResult { terminal_id })
    }

    async fn on_terminal_output(
        &self,
        params: TerminalOutputParams,
    ) -> HandlerResult<TerminalOutputResult> {
        let terminals = self.terminals.lock().await;
        let state = terminals
            .get(&params.terminal_id)
            .ok_or_else(|| rpc_invalid_params(format!("terminal not found: {}", params.terminal_id)))?;
        // Merge stdout and stderr, stdout first, matching what an agent
        // would see from a real PTY.
        let mut bytes = state.stdout.clone();
        bytes.extend_from_slice(&state.stderr);
        let output = String::from_utf8_lossy(&bytes).into_owned();
        Ok(TerminalOutputResult {
            output,
            truncated: state.truncated,
        })
    }

    async fn on_terminal_wait(
        &self,
        params: WaitForExitParams,
    ) -> HandlerResult<WaitForExitResult> {
        let terminals = self.terminals.lock().await;
        let state = terminals
            .get(&params.terminal_id)
            .ok_or_else(|| rpc_invalid_params(format!("terminal not found: {}", params.terminal_id)))?;
        Ok(WaitForExitResult {
            exit_code: state.exit_code,
        })
    }

    async fn on_terminal_kill(&self, _params: KillTerminalParams) -> HandlerResult<()> {
        // Commands are synchronous; by the time the agent sends kill,
        // they have already finished. No-op.
        Ok(())
    }

    async fn on_terminal_release(&self, params: ReleaseTerminalParams) -> HandlerResult<()> {
        self.terminals.lock().await.remove(&params.terminal_id);
        Ok(())
    }

    async fn on_fs_read(
        &self,
        params: ReadTextFileParams,
    ) -> HandlerResult<ReadTextFileResult> {
        let path = std::path::PathBuf::from(&params.path);
        let content = {
            let vfs = self.vfs.lock().expect("vfs mutex poisoned");
            vfs.read(&path)
                .map_err(|e| rpc_invalid_params(format!("fs/read: {}: {e}", params.path)))?
        };

        let text = String::from_utf8_lossy(&content).into_owned();
        let lines: Vec<&str> = text.lines().collect();

        let start = params.line.map(|n| n.saturating_sub(1) as usize).unwrap_or(0);
        let start = min(start, lines.len());
        let limit = params
            .limit
            .map(|n| n as usize)
            .unwrap_or(lines.len().saturating_sub(start));
        let end = min(start.saturating_add(limit), lines.len());

        let mut selected = lines[start..end].join("\n");
        if end > 0 && end == lines.len() && text.ends_with('\n') {
            selected.push('\n');
        }

        let cap = self.config.max_output_bytes as usize;
        let mut truncated = false;
        if selected.len() > cap {
            selected.truncate(cap);
            truncated = true;
        }

        self.trace.record(TraceEvent::FsRead {
            path: params.path,
            bytes: selected.len(),
        });

        Ok(ReadTextFileResult {
            content: selected,
            truncated,
        })
    }

    async fn on_fs_write(&self, params: WriteTextFileParams) -> HandlerResult<()> {
        let path = std::path::PathBuf::from(&params.path);
        let bytes = params.content.len();
        {
            let mut vfs = self.vfs.lock().expect("vfs mutex poisoned");
            vfs.write(&path, params.content.as_bytes())
                .map_err(|e| rpc_invalid_params(format!("fs/write: {}: {e}", params.path)))?;
        }
        self.trace.record(TraceEvent::FsWrite {
            path: params.path,
            bytes,
        });
        Ok(())
    }

    async fn on_session_update(&self, params: SessionUpdateParams) {
        self.trace.record_session_update(&params);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_commands_match_bare_names() {
        assert!(is_blocked("curl"));
        assert!(is_blocked("wget"));
        assert!(is_blocked("/usr/bin/curl"));
        assert!(!is_blocked("cat"));
        assert!(!is_blocked("echo"));
        assert!(!is_blocked("my-curl-wrapper"));
    }

    #[test]
    fn truncate_to_caps() {
        let (out, trunc) = truncate_to(b"hello", 3);
        assert_eq!(out, b"hel");
        assert!(trunc);
        let (out, trunc) = truncate_to(b"hi", 10);
        assert_eq!(out, b"hi");
        assert!(!trunc);
    }

    #[test]
    fn format_command_joins_with_spaces() {
        assert_eq!(format_command("echo", &[]), "echo");
        assert_eq!(
            format_command("echo", &["hello".into(), "world".into()]),
            "echo hello world"
        );
    }
}
