//! Acceptance tests for Cap 12 — ACP Hook Handlers.
//!
//! Drives [`SandboxHandler`] directly (no JSON-RPC transport) so each
//! test exercises exactly one acceptance criterion from
//! `capabilities/12-acp-hooks.md`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devdev_acp::{
    AcpHandler, CollectingTraceLogger, CreateTerminalParams, HandlerConfig,
    KillTerminalParams, PermissionKind, PermissionOption, PermissionOutcome,
    PermissionRequestParams, ReadTextFileParams, ReleaseTerminalParams, SandboxHandler,
    SessionUpdate, SessionUpdateParams, TerminalOutputParams, ToolCallInfo, TraceEvent,
    WaitForExitParams, WriteTextFileParams,
};
use devdev_git::{GitResult, VirtualGit};
use devdev_shell::ShellSession;
use devdev_vfs::MemFs;
use devdev_wasm::{ToolEngine, ToolResult};

// ── Fakes (mirrored from shell/tests, trimmed to what hooks exercise) ────

struct FakeTools;

impl ToolEngine for FakeTools {
    fn execute(
        &self,
        command: &str,
        args: &[String],
        stdin: &[u8],
        _env: &HashMap<String, String>,
        _cwd: &str,
        fs: &mut MemFs,
    ) -> ToolResult {
        match command {
            "echo" => ToolResult {
                stdout: format!("{}\n", args.join(" ")).into_bytes(),
                stderr: Vec::new(),
                exit_code: 0,
            },
            "cat" => {
                let mut out = Vec::new();
                if args.is_empty() {
                    out.extend_from_slice(stdin);
                } else {
                    for a in args {
                        let abs = std::path::PathBuf::from(if a.starts_with('/') {
                            a.clone()
                        } else {
                            format!("/{a}")
                        });
                        match fs.read(&abs) {
                            Ok(data) => out.extend_from_slice(&data),
                            Err(e) => {
                                return ToolResult {
                                    stdout: Vec::new(),
                                    stderr: format!("cat: {a}: {e}\n").into_bytes(),
                                    exit_code: 1,
                                };
                            }
                        }
                    }
                }
                ToolResult {
                    stdout: out,
                    stderr: Vec::new(),
                    exit_code: 0,
                }
            }
            "sleep" => {
                let secs: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                std::thread::sleep(Duration::from_secs(secs));
                ToolResult {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                }
            }
            other => ToolResult {
                stdout: Vec::new(),
                stderr: format!("command not found: {other}\n").into_bytes(),
                exit_code: 127,
            },
        }
    }

    fn available_tools(&self) -> Vec<&str> {
        vec!["cat", "echo", "sleep"]
    }

    fn has_tool(&self, name: &str) -> bool {
        self.available_tools().contains(&name)
    }
}

struct FakeGit;

impl VirtualGit for FakeGit {
    fn execute(&self, args: &[String], _cwd: &str) -> GitResult {
        GitResult::ok(format!("git-fake: {}\n", args.join(" ")).into_bytes())
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn make_handler() -> (Arc<SandboxHandler>, Arc<Mutex<MemFs>>, Arc<CollectingTraceLogger>) {
    make_handler_with_config(HandlerConfig::default())
}

fn make_handler_with_config(
    config: HandlerConfig,
) -> (Arc<SandboxHandler>, Arc<Mutex<MemFs>>, Arc<CollectingTraceLogger>) {
    let vfs = Arc::new(Mutex::new(MemFs::new()));
    let vfs_for_shell = vfs.clone();
    let trace: Arc<CollectingTraceLogger> = Arc::new(CollectingTraceLogger::new());
    let handler = SandboxHandler::with_config(
        move || {
            let tools: Arc<dyn ToolEngine> = Arc::new(FakeTools);
            let git: Arc<Mutex<dyn VirtualGit>> = Arc::new(Mutex::new(FakeGit));
            ShellSession::new(vfs_for_shell, tools, git)
        },
        vfs.clone(),
        config,
    )
    .with_trace(trace.clone());
    (Arc::new(handler), vfs, trace)
}

fn new_session_id() -> String {
    "sess-1".to_owned()
}

// ── Acceptance Criteria ──────────────────────────────────────────────────

/// AC: `terminal/create` with a valid command executes via the shell
/// and returns a terminal_id.
#[tokio::test]
async fn terminal_create_returns_id() {
    let (h, _vfs, _trace) = make_handler();
    let r = h
        .on_terminal_create(CreateTerminalParams {
            session_id: new_session_id(),
            command: "echo".into(),
            args: vec!["hello".into()],
            cwd: None,
            env: Vec::new(),
            output_byte_limit: None,
        })
        .await
        .expect("terminal/create ok");
    assert!(r.terminal_id.starts_with("term-"));
}

/// AC: `terminal/output` returns the recorded stdout.
#[tokio::test]
async fn terminal_output_returns_stdout() {
    let (h, _vfs, _trace) = make_handler();
    let id = h
        .on_terminal_create(CreateTerminalParams {
            session_id: new_session_id(),
            command: "echo".into(),
            args: vec!["world".into()],
            cwd: None,
            env: Vec::new(),
            output_byte_limit: None,
        })
        .await
        .unwrap()
        .terminal_id;
    let out = h
        .on_terminal_output(TerminalOutputParams {
            session_id: new_session_id(),
            terminal_id: id,
        })
        .await
        .unwrap();
    assert_eq!(out.output, "world\n");
    assert!(!out.truncated);
}

/// AC: `terminal/wait_for_exit` returns the stored exit code.
#[tokio::test]
async fn terminal_wait_returns_exit_code() {
    let (h, _vfs, _trace) = make_handler();
    let id = h
        .on_terminal_create(CreateTerminalParams {
            session_id: new_session_id(),
            command: "echo".into(),
            args: vec!["ok".into()],
            cwd: None,
            env: Vec::new(),
            output_byte_limit: None,
        })
        .await
        .unwrap()
        .terminal_id;
    let wait = h
        .on_terminal_wait(WaitForExitParams {
            session_id: new_session_id(),
            terminal_id: id,
        })
        .await
        .unwrap();
    assert_eq!(wait.exit_code, 0);
}

/// AC: unknown terminal_id surfaces as INVALID_PARAMS.
#[tokio::test]
async fn terminal_output_unknown_id_errors() {
    let (h, _vfs, _trace) = make_handler();
    let err = h
        .on_terminal_output(TerminalOutputParams {
            session_id: new_session_id(),
            terminal_id: "term-missing".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("terminal not found"));
}

/// AC: `terminal/kill` is a no-op (commands already completed).
#[tokio::test]
async fn terminal_kill_is_noop() {
    let (h, _vfs, _trace) = make_handler();
    h.on_terminal_kill(KillTerminalParams {
        session_id: new_session_id(),
        terminal_id: "anything".into(),
    })
    .await
    .unwrap();
}

/// AC: `terminal/release` drops the stored state.
#[tokio::test]
async fn terminal_release_removes_state() {
    let (h, _vfs, _trace) = make_handler();
    let id = h
        .on_terminal_create(CreateTerminalParams {
            session_id: new_session_id(),
            command: "echo".into(),
            args: vec!["x".into()],
            cwd: None,
            env: Vec::new(),
            output_byte_limit: None,
        })
        .await
        .unwrap()
        .terminal_id;

    h.on_terminal_release(ReleaseTerminalParams {
        session_id: new_session_id(),
        terminal_id: id.clone(),
    })
    .await
    .unwrap();

    let err = h
        .on_terminal_output(TerminalOutputParams {
            session_id: new_session_id(),
            terminal_id: id,
        })
        .await
        .unwrap_err();
    assert!(err.message.contains("not found"));
}

/// AC: network commands (curl, wget, …) are rejected before touching
/// the shell.
#[tokio::test]
async fn network_commands_are_blocked() {
    let (h, _vfs, trace) = make_handler();
    let err = h
        .on_terminal_create(CreateTerminalParams {
            session_id: new_session_id(),
            command: "curl".into(),
            args: vec!["http://example.com".into()],
            cwd: None,
            env: Vec::new(),
            output_byte_limit: None,
        })
        .await
        .unwrap_err();
    assert!(err.message.contains("curl"));
    // Trace recorded the rejection.
    assert!(trace
        .events()
        .iter()
        .any(|e| matches!(e, TraceEvent::TerminalRejected { .. })));
}

/// AC: wall-clock timeout returns an error instead of hanging.
#[tokio::test]
async fn command_timeout_returns_error() {
    let config = HandlerConfig {
        command_timeout: Duration::from_millis(50),
        ..HandlerConfig::default()
    };
    let (h, _vfs, _trace) = make_handler_with_config(config);
    let err = h
        .on_terminal_create(CreateTerminalParams {
            session_id: new_session_id(),
            command: "sleep".into(),
            args: vec!["5".into()],
            cwd: None,
            env: Vec::new(),
            output_byte_limit: None,
        })
        .await
        .unwrap_err();
    assert!(err.message.contains("timed out"));
}

/// AC: `fs/read_text_file` returns VFS contents.
#[tokio::test]
async fn fs_read_returns_content() {
    let (h, vfs, _trace) = make_handler();
    vfs.lock()
        .unwrap()
        .write(std::path::Path::new("/hello.txt"), b"alpha\nbeta\ngamma\n")
        .unwrap();

    let r = h
        .on_fs_read(ReadTextFileParams {
            session_id: new_session_id(),
            path: "/hello.txt".into(),
            line: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(r.content, "alpha\nbeta\ngamma\n");
    assert!(!r.truncated);
}

/// AC: `fs/read_text_file` honours `line` and `limit` (1-based line).
#[tokio::test]
async fn fs_read_line_and_limit() {
    let (h, vfs, _trace) = make_handler();
    vfs.lock()
        .unwrap()
        .write(
            std::path::Path::new("/lines.txt"),
            b"one\ntwo\nthree\nfour\nfive\n",
        )
        .unwrap();

    let r = h
        .on_fs_read(ReadTextFileParams {
            session_id: new_session_id(),
            path: "/lines.txt".into(),
            line: Some(2),
            limit: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(r.content, "two\nthree");
}

/// AC: `fs/write_text_file` makes the file readable via the VFS.
#[tokio::test]
async fn fs_write_persists_to_vfs() {
    let (h, vfs, _trace) = make_handler();
    h.on_fs_write(WriteTextFileParams {
        session_id: new_session_id(),
        path: "/note.txt".into(),
        content: "jotted\n".into(),
    })
    .await
    .unwrap();

    let bytes = vfs
        .lock()
        .unwrap()
        .read(std::path::Path::new("/note.txt"))
        .unwrap();
    assert_eq!(bytes, b"jotted\n");
}

/// AC: a write via the hook is visible to a subsequent terminal command.
#[tokio::test]
async fn fs_write_then_shell_cat_roundtrip() {
    let (h, _vfs, _trace) = make_handler();
    h.on_fs_write(WriteTextFileParams {
        session_id: new_session_id(),
        path: "/greet.txt".into(),
        content: "hi\n".into(),
    })
    .await
    .unwrap();

    let id = h
        .on_terminal_create(CreateTerminalParams {
            session_id: new_session_id(),
            command: "cat".into(),
            args: vec!["/greet.txt".into()],
            cwd: None,
            env: Vec::new(),
            output_byte_limit: None,
        })
        .await
        .unwrap()
        .terminal_id;

    let out = h
        .on_terminal_output(TerminalOutputParams {
            session_id: new_session_id(),
            terminal_id: id,
        })
        .await
        .unwrap();
    assert_eq!(out.output, "hi\n");
}

/// AC: `session/request_permission` auto-approves with an AllowOnce option.
#[tokio::test]
async fn permission_request_auto_approves() {
    let (h, _vfs, trace) = make_handler();
    let resp = h
        .on_permission_request(PermissionRequestParams {
            session_id: new_session_id(),
            tool_call: ToolCallInfo {
                tool_call_id: "tc-1".into(),
                title: "run grep".into(),
                kind: None,
            },
            options: vec![
                PermissionOption {
                    option_id: "reject".into(),
                    kind: PermissionKind::RejectOnce,
                    name: "No".into(),
                },
                PermissionOption {
                    option_id: "allow".into(),
                    kind: PermissionKind::AllowOnce,
                    name: "Yes".into(),
                },
            ],
        })
        .await
        .unwrap();
    match resp.outcome {
        PermissionOutcome::Selected { option_id } => assert_eq!(option_id, "allow"),
        PermissionOutcome::Cancelled => panic!("expected Selected"),
    }
    assert!(trace
        .events()
        .iter()
        .any(|e| matches!(e, TraceEvent::PermissionGranted { .. })));
}

/// AC: `session/update` notifications are logged through the TraceLogger.
#[tokio::test]
async fn session_update_is_traced() {
    let (h, _vfs, trace) = make_handler();
    h.on_session_update(SessionUpdateParams {
        session_id: new_session_id(),
        update: SessionUpdate::AgentMessageChunk {
            content: devdev_acp::ContentBlock {
                text: "hi".into(),
            },
        },
    })
    .await;

    let events = trace.events();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        TraceEvent::SessionUpdate { kind, .. } if kind == "agentMessageChunk"
    ));
}
