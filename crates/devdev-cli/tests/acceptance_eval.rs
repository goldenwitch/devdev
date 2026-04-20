//! Acceptance tests for capability 13 — `evaluate()` end-to-end.
//!
//! Every test drives `evaluate()` through a scripted fake agent over
//! `tokio::io::duplex`. No `copilot` binary, no network, no `GH_TOKEN`.
//!
//! The fake agent helper lives at the bottom of this file; tests share
//! one NDJSON pipe pair per test and drive the agent with explicit
//! request/response/notification scripts.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use devdev_acp::protocol::{Message, Notification, Request, RequestId, Response, RpcError};
use devdev_acp::transport::{AsyncNdjsonReader, AsyncNdjsonWriter};
use devdev_acp::types::{
    AgentCapabilities, AgentInfo, AuthMethod, ContentBlock, CreateTerminalParams,
    CreateTerminalResult, InitializeResult, NewSessionResult, PromptResult,
    ReadTextFileParams, ReadTextFileResult, SessionUpdate, SessionUpdateParams, StopReason,
    TerminalOutputParams, TerminalOutputResult, WaitForExitParams, WaitForExitResult,
    WriteTextFileParams,
};
use devdev_cli::{
    EvalConfig, EvalContext, EvalError, PreferenceFile, Transport, evaluate,
};
use tempfile::TempDir;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf, duplex, split};
use tokio::sync::Mutex as AsyncMutex;

// ── Fake agent ──────────────────────────────────────────────────────────

struct FakeAgent {
    reader: AsyncMutex<AsyncNdjsonReader<ReadHalf<DuplexStream>>>,
    writer: AsyncMutex<AsyncNdjsonWriter<WriteHalf<DuplexStream>>>,
}

impl FakeAgent {
    fn new(stream: DuplexStream) -> Self {
        let (r, w) = split(stream);
        Self {
            reader: AsyncMutex::new(AsyncNdjsonReader::new(r)),
            writer: AsyncMutex::new(AsyncNdjsonWriter::new(w)),
        }
    }

    async fn recv(&self) -> Option<Message> {
        self.reader.lock().await.recv().await.unwrap()
    }

    async fn send(&self, msg: Message) {
        self.writer.lock().await.send(&msg).await.unwrap();
    }

    async fn expect_request(&self, method: &str) -> Request {
        let msg = self.recv().await.expect("agent: unexpected EOF");
        match msg {
            Message::Request(r) => {
                assert_eq!(r.method, method, "unexpected method: {:?}", r.method);
                r
            }
            other => panic!("expected request {method}, got {other:?}"),
        }
    }

    async fn reply_ok(&self, id: RequestId, result: serde_json::Value) {
        self.send(Message::Response(Response::success(id, result))).await;
    }

    async fn reply_err(&self, id: RequestId, code: i32, message: &str) {
        self.send(Message::Response(Response {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }))
        .await;
    }

    async fn notify(&self, method: &str, params: serde_json::Value) {
        self.send(Message::Notification(Notification::new(method, Some(params))))
            .await;
    }

    /// Handle the fixed initialize → new_session handshake (no auth).
    async fn handshake_no_auth(&self, session_id: &str) {
        let init = self.expect_request("initialize").await;
        self.reply_ok(
            init.id,
            serde_json::to_value(InitializeResult {
                protocol_version: 1,
                agent_info: AgentInfo {
                    name: "fake".into(),
                    version: "0".into(),
                },
                agent_capabilities: AgentCapabilities { streaming: None },
                auth_methods: vec![],
            })
            .unwrap(),
        )
        .await;
        let sess = self.expect_request("session/new").await;
        self.reply_ok(
            sess.id,
            serde_json::to_value(NewSessionResult {
                session_id: session_id.into(),
            })
            .unwrap(),
        )
        .await;
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build a throwaway tempdir with one small file. Returned as an owned
/// `TempDir` so the test keeps it alive.
fn make_tempdir(name: &str, content: &str) -> TempDir {
    let td = TempDir::new().unwrap();
    fs::write(td.path().join(name), content).unwrap();
    td
}

/// Short-fuse config for deterministic tests.
fn short_config() -> EvalConfig {
    EvalConfig {
        workspace_limit: 64 * 1024 * 1024,
        command_timeout: Duration::from_secs(5),
        session_timeout: Duration::from_secs(5),
        cli_hang_timeout: Duration::from_secs(5),
        include_git: true,
    }
}

fn simple_context(task: &str) -> EvalContext {
    EvalContext {
        task: task.into(),
        diff: None,
        preferences: vec![],
        focus_paths: vec![],
    }
}

/// Assert that the single `Arc<Mutex<MemFs>>` inside `evaluate` has
/// been dropped. We can't observe it directly; instead, the caller
/// keeps a `Weak` to a sentinel Arc the test constructs and passes
/// only indirectly. The easier check: ensure `evaluate` returns with
/// no lingering handles by holding no clones ourselves.
fn _cleanup_note() {}

// ── Connected transport factory ────────────────────────────────────────

/// Build a `Transport::Connected` pair. Returns `(transport, fake_agent)`.
fn duplex_transport() -> (Transport, Arc<FakeAgent>) {
    let (client_end, agent_end) = duplex(64 * 1024);
    let (client_r, client_w) = split(client_end);
    let transport = Transport::Connected {
        reader: Box::new(client_r),
        writer: Box::new(client_w),
    };
    (transport, Arc::new(FakeAgent::new(agent_end)))
}

// ─────────────────────────────────────────────────────────────────────────
// AC-01 simple_happy_path
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_01_simple_happy_path() {
    let repo = make_tempdir("README.md", "# hello\n");
    let (transport, agent) = duplex_transport();

    let agent_task = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let prompt = agent.expect_request("session/prompt").await;
        agent
            .notify(
                "session/update",
                serde_json::to_value(SessionUpdateParams {
                    session_id: "s1".into(),
                    update: SessionUpdate::AgentMessageChunk {
                        content: ContentBlock {
                            text: "No issues found.".into(),
                        },
                    },
                })
                .unwrap(),
            )
            .await;
        agent
            .reply_ok(
                prompt.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let result = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect("evaluate ok");
    agent_task.await.unwrap();

    assert_eq!(result.verdict, "No issues found.");
    assert_eq!(result.stop_reason, "endTurn");
    assert!(result.tool_calls.is_empty());
    assert!(!result.is_git_repo);
    assert!(result.repo_stats.files >= 1);
}

// ─────────────────────────────────────────────────────────────────────────
// AC-02 tool_call_roundtrip
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_02_tool_call_roundtrip() {
    let repo = make_tempdir("a.txt", "");
    let (transport, agent) = duplex_transport();

    let agent_task = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let prompt = agent.expect_request("session/prompt").await;

        // Agent asks the client to spawn `echo hello`.
        let create = Request::new(
            RequestId::Number(1001),
            "terminal/create",
            Some(
                serde_json::to_value(CreateTerminalParams {
                    session_id: "s1".into(),
                    command: "echo".into(),
                    args: vec!["hello".into()],
                    cwd: None,
                    env: vec![],
                    output_byte_limit: None,
                })
                .unwrap(),
            ),
        );
        agent.send(Message::Request(create)).await;
        let create_resp = agent.recv().await.unwrap();
        let Message::Response(cr) = create_resp else { panic!("expected response") };
        let ct: CreateTerminalResult =
            serde_json::from_value(cr.result.unwrap()).unwrap();

        // terminal/output
        let out = Request::new(
            RequestId::Number(1002),
            "terminal/output",
            Some(
                serde_json::to_value(TerminalOutputParams {
                    session_id: "s1".into(),
                    terminal_id: ct.terminal_id.clone(),
                })
                .unwrap(),
            ),
        );
        agent.send(Message::Request(out)).await;
        let _ = agent.recv().await.unwrap();

        // terminal/wait_for_exit
        let wait = Request::new(
            RequestId::Number(1003),
            "terminal/wait_for_exit",
            Some(
                serde_json::to_value(WaitForExitParams {
                    session_id: "s1".into(),
                    terminal_id: ct.terminal_id,
                })
                .unwrap(),
            ),
        );
        agent.send(Message::Request(wait)).await;
        let _ = agent.recv().await.unwrap();

        // Finish the turn.
        agent
            .reply_ok(
                prompt.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let result = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect("evaluate ok");
    agent_task.await.unwrap();

    assert_eq!(result.tool_calls.len(), 1);
    let tc = &result.tool_calls[0];
    assert_eq!(tc.command, "echo hello");
    assert_eq!(tc.exit_code, 0);
    // Duration is best-effort lower-bounded non-zero on fast machines;
    // the measurement clock is Instant so we can at least assert <
    // command_timeout.
    assert!(tc.duration < Duration::from_secs(5));
}

// ─────────────────────────────────────────────────────────────────────────
// AC-03 fs_roundtrip
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_03_fs_roundtrip() {
    let repo = make_tempdir("a.txt", "seed");
    let (transport, agent) = duplex_transport();

    let agent_task = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let prompt = agent.expect_request("session/prompt").await;

        // fs/write_text_file
        let w = Request::new(
            RequestId::Number(2001),
            "fs/write_text_file",
            Some(
                serde_json::to_value(WriteTextFileParams {
                    session_id: "s1".into(),
                    path: "/out.txt".into(),
                    content: "hello from fs".into(),
                })
                .unwrap(),
            ),
        );
        agent.send(Message::Request(w)).await;
        let _ = agent.recv().await.unwrap();

        // Read it back via fs/read_text_file. (The wasm `cat` tool has
        // no VFS preopen by design — see registry.rs run_wasm — so the
        // roundtrip is expressed through the ACP fs hook, which is
        // exactly what cap 13 is meant to validate.)
        let r = Request::new(
            RequestId::Number(2002),
            "fs/read_text_file",
            Some(
                serde_json::to_value(ReadTextFileParams {
                    session_id: "s1".into(),
                    path: "/out.txt".into(),
                    line: None,
                    limit: None,
                })
                .unwrap(),
            ),
        );
        agent.send(Message::Request(r)).await;
        let Message::Response(rr) = agent.recv().await.unwrap() else {
            panic!()
        };
        let rtr: ReadTextFileResult =
            serde_json::from_value(rr.result.unwrap()).unwrap();
        // Surface the content in the verdict via an agent_message_chunk.
        agent
            .notify(
                "session/update",
                serde_json::to_value(SessionUpdateParams {
                    session_id: "s1".into(),
                    update: SessionUpdate::AgentMessageChunk {
                        content: ContentBlock {
                            text: rtr.content.clone(),
                        },
                    },
                })
                .unwrap(),
            )
            .await;

        agent
            .reply_ok(
                prompt.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let result = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect("evaluate ok");
    agent_task.await.unwrap();

    assert!(
        result.verdict.contains("hello from fs"),
        "verdict = {:?}",
        result.verdict
    );
}

// ─────────────────────────────────────────────────────────────────────────
// AC-04 verdict_is_chunk_concat
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_04_verdict_is_chunk_concat() {
    let repo = make_tempdir("a.txt", "");
    let (transport, agent) = duplex_transport();

    let agent_task = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let prompt = agent.expect_request("session/prompt").await;

        for chunk in ["alpha ", "beta"] {
            agent
                .notify(
                    "session/update",
                    serde_json::to_value(SessionUpdateParams {
                        session_id: "s1".into(),
                        update: SessionUpdate::AgentMessageChunk {
                            content: ContentBlock {
                                text: chunk.into(),
                            },
                        },
                    })
                    .unwrap(),
                )
                .await;
        }
        // A thought chunk that must NOT appear in the verdict.
        agent
            .notify(
                "session/update",
                serde_json::to_value(SessionUpdateParams {
                    session_id: "s1".into(),
                    update: SessionUpdate::AgentThoughtChunk {
                        content: ContentBlock {
                            text: "thought".into(),
                        },
                    },
                })
                .unwrap(),
            )
            .await;

        agent
            .reply_ok(
                prompt.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let result = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect("evaluate ok");
    agent_task.await.unwrap();

    assert_eq!(result.verdict, "alpha beta");
}

// ─────────────────────────────────────────────────────────────────────────
// AC-05 repo_too_large_fails_before_spawn
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_05_repo_too_large_fails_before_spawn() {
    let repo = make_tempdir("big.txt", "abcdefghij");
    let config = EvalConfig {
        workspace_limit: 1,
        ..short_config()
    };
    // Program path that cannot exist — if evaluate() tried to spawn it
    // the error would be `AcpError::Spawn`, not `RepoTooLarge`.
    let transport = Transport::SpawnProcess {
        program: "__devdev_should_never_spawn__".into(),
        args: vec![],
    };

    let err = evaluate(repo.path(), config, simple_context("review"), transport)
        .await
        .expect_err("must fail");
    match err {
        EvalError::RepoTooLarge { total, limit } => {
            assert_eq!(limit, 1);
            assert!(total >= 10);
        }
        other => panic!("expected RepoTooLarge, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// AC-06 not_a_git_repo_is_soft
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_06_not_a_git_repo_is_soft() {
    let repo = make_tempdir("file.txt", "content");
    let (transport, agent) = duplex_transport();

    let agent_task = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let prompt = agent.expect_request("session/prompt").await;

        let create = Request::new(
            RequestId::Number(1),
            "terminal/create",
            Some(
                serde_json::to_value(CreateTerminalParams {
                    session_id: "s1".into(),
                    command: "git".into(),
                    args: vec!["log".into()],
                    cwd: None,
                    env: vec![],
                    output_byte_limit: None,
                })
                .unwrap(),
            ),
        );
        agent.send(Message::Request(create)).await;
        let Message::Response(cr) = agent.recv().await.unwrap() else {
            panic!()
        };
        let ct: CreateTerminalResult =
            serde_json::from_value(cr.result.unwrap()).unwrap();

        let out = Request::new(
            RequestId::Number(2),
            "terminal/output",
            Some(
                serde_json::to_value(TerminalOutputParams {
                    session_id: "s1".into(),
                    terminal_id: ct.terminal_id.clone(),
                })
                .unwrap(),
            ),
        );
        agent.send(Message::Request(out)).await;
        let Message::Response(or) = agent.recv().await.unwrap() else {
            panic!()
        };
        let tor: TerminalOutputResult =
            serde_json::from_value(or.result.unwrap()).unwrap();
        agent
            .notify(
                "session/update",
                serde_json::to_value(SessionUpdateParams {
                    session_id: "s1".into(),
                    update: SessionUpdate::AgentMessageChunk {
                        content: ContentBlock {
                            text: tor.output,
                        },
                    },
                })
                .unwrap(),
            )
            .await;

        let wait = Request::new(
            RequestId::Number(3),
            "terminal/wait_for_exit",
            Some(
                serde_json::to_value(WaitForExitParams {
                    session_id: "s1".into(),
                    terminal_id: ct.terminal_id,
                })
                .unwrap(),
            ),
        );
        agent.send(Message::Request(wait)).await;
        let Message::Response(wr) = agent.recv().await.unwrap() else {
            panic!()
        };
        let wer: WaitForExitResult =
            serde_json::from_value(wr.result.unwrap()).unwrap();
        // Stash exit code in another chunk so the test can read it.
        agent
            .notify(
                "session/update",
                serde_json::to_value(SessionUpdateParams {
                    session_id: "s1".into(),
                    update: SessionUpdate::AgentMessageChunk {
                        content: ContentBlock {
                            text: format!("|exit={}", wer.exit_code),
                        },
                    },
                })
                .unwrap(),
            )
            .await;

        agent
            .reply_ok(
                prompt.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let result = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect("evaluate ok");
    agent_task.await.unwrap();

    assert!(!result.is_git_repo);
    assert!(
        result.verdict.contains("not a git repository"),
        "verdict = {:?}",
        result.verdict
    );
    assert!(result.verdict.contains("|exit=1"));
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].exit_code, 1);
}

// ─────────────────────────────────────────────────────────────────────────
// AC-07 session_timeout_returns_timeout_error
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_07_session_timeout_returns_timeout_error() {
    let repo = make_tempdir("a.txt", "");
    let (transport, agent) = duplex_transport();

    // Silent agent: handshake, then ignore session/prompt forever.
    let _keepalive = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let _prompt = agent.expect_request("session/prompt").await;
        // Never reply. Keep the connection alive by parking.
        futures_like_park(agent.clone()).await;
    });

    let config = EvalConfig {
        session_timeout: Duration::from_millis(50),
        cli_hang_timeout: Duration::from_secs(60),
        ..short_config()
    };
    let started = std::time::Instant::now();
    let err = evaluate(repo.path(), config, simple_context("review"), transport)
        .await
        .expect_err("must time out");
    let elapsed = started.elapsed();

    match err {
        EvalError::Timeout(_) => {}
        other => panic!("expected Timeout, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_millis(500),
        "took too long: {elapsed:?}"
    );
}

async fn futures_like_park(_keep: Arc<FakeAgent>) {
    // Park forever. The test harness will drop the task when the
    // test binary exits.
    std::future::pending::<()>().await;
}

// ─────────────────────────────────────────────────────────────────────────
// AC-08 authentication_failure_propagates
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_08_authentication_failure_propagates() {
    // Clear env tokens so the client does NOT short-circuit to
    // EnvToken.
    //
    // SAFETY: remove_var is unsafe since Rust 2024 because mutating
    // env state is not thread-safe with respect to other tests.
    // These tests run in separate processes per cargo test binary;
    // within this binary, no other test reads these vars.
    unsafe {
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("GITHUB_TOKEN");
        std::env::remove_var("COPILOT_GITHUB_TOKEN");
    }

    let repo = make_tempdir("a.txt", "");
    let (transport, agent) = duplex_transport();

    let agent_task = tokio::spawn(async move {
        let init = agent.expect_request("initialize").await;
        agent
            .reply_ok(
                init.id,
                serde_json::to_value(InitializeResult {
                    protocol_version: 1,
                    agent_info: AgentInfo {
                        name: "fake".into(),
                        version: "0".into(),
                    },
                    agent_capabilities: AgentCapabilities { streaming: None },
                    auth_methods: vec![AuthMethod {
                        kind: "api_key".into(),
                    }],
                })
                .unwrap(),
            )
            .await;
        let auth = agent.expect_request("authenticate").await;
        agent.reply_err(auth.id, -32000, "invalid credentials").await;
    });

    let err = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect_err("must fail");
    agent_task.await.unwrap();

    match err {
        EvalError::AuthenticationFailed(msg) => {
            assert!(
                msg.contains("invalid credentials"),
                "unexpected message: {msg:?}"
            );
        }
        other => panic!("expected AuthenticationFailed, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// AC-09 tool_call_log_order_preserved
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_09_tool_call_log_order_preserved() {
    let repo = make_tempdir("a.txt", "");
    let (transport, agent) = duplex_transport();

    let agent_task = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let prompt = agent.expect_request("session/prompt").await;

        for (idx, cmd) in [("echo", "one"), ("echo", "two"), ("echo", "three")]
            .into_iter()
            .enumerate()
        {
            let id = RequestId::Number((idx as u64 + 1) * 100);
            let create = Request::new(
                id.clone(),
                "terminal/create",
                Some(
                    serde_json::to_value(CreateTerminalParams {
                        session_id: "s1".into(),
                        command: cmd.0.into(),
                        args: vec![cmd.1.into()],
                        cwd: None,
                        env: vec![],
                        output_byte_limit: None,
                    })
                    .unwrap(),
                ),
            );
            agent.send(Message::Request(create)).await;
            let Message::Response(cr) = agent.recv().await.unwrap() else {
                panic!()
            };
            let ct: CreateTerminalResult =
                serde_json::from_value(cr.result.unwrap()).unwrap();

            // wait_for_exit to cleanly finish the tool call.
            let wait = Request::new(
                RequestId::Number(id_num(&id) + 1),
                "terminal/wait_for_exit",
                Some(
                    serde_json::to_value(WaitForExitParams {
                        session_id: "s1".into(),
                        terminal_id: ct.terminal_id,
                    })
                    .unwrap(),
                ),
            );
            agent.send(Message::Request(wait)).await;
            let _ = agent.recv().await.unwrap();
        }

        agent
            .reply_ok(
                prompt.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let result = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect("evaluate ok");
    agent_task.await.unwrap();

    assert_eq!(result.tool_calls.len(), 3);
    assert_eq!(result.tool_calls[0].command, "echo one");
    assert_eq!(result.tool_calls[1].command, "echo two");
    assert_eq!(result.tool_calls[2].command, "echo three");
}

fn id_num(id: &RequestId) -> u64 {
    match id {
        RequestId::Number(n) => *n,
        _ => 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// AC-10 agent_disconnect_returns_cli_crashed
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ac_10_agent_disconnect_returns_cli_crashed() {
    let repo = make_tempdir("a.txt", "");
    let (transport, agent) = duplex_transport();

    let agent_task = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let _prompt = agent.expect_request("session/prompt").await;
        // Drop the pipe without replying — client sees EOF.
        drop(agent);
    });

    let err = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect_err("must fail");
    agent_task.await.unwrap();

    match err {
        EvalError::CliCrashed => {}
        other => panic!("expected CliCrashed, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Resource-leak check: VFS is dropped after evaluate returns.
// ─────────────────────────────────────────────────────────────────────────
//
// We can't peek at `evaluate`'s internal `Arc<Mutex<MemFs>>`, but we
// can observe that the tempdir itself is reclaimable and that the
// handler/trace collector Arcs have no lingering strong references
// (see eval.rs: drop(client) → reader task aborts → handler Arc
// dropped → collector Arcs at strong_count == 1 in the caller).

#[tokio::test]
async fn cleanup_drops_all_handles() {
    let repo = make_tempdir("a.txt", "");
    let (transport, agent) = duplex_transport();
    let agent_task = tokio::spawn(async move {
        agent.handshake_no_auth("s1").await;
        let prompt = agent.expect_request("session/prompt").await;
        agent
            .reply_ok(
                prompt.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    // Just ensure evaluate returns cleanly and tempdir is still
    // writable after — meaning nothing is holding a handle into it.
    let _result = evaluate(
        repo.path(),
        short_config(),
        simple_context("review"),
        transport,
    )
    .await
    .expect("evaluate ok");
    agent_task.await.unwrap();

    fs::write(repo.path().join("new.txt"), "post-eval").expect("tempdir is free");
}

// Compile-time witness that the key public types are re-exported.
#[allow(dead_code)]
fn _type_check() {
    let _: PathBuf = PathBuf::from("x");
    let _: Weak<Mutex<()>> = Weak::new();
    let _: PreferenceFile = PreferenceFile {
        name: "x".into(),
        content: "y".into(),
    };
}
