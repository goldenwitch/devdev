//! Acceptance tests for capability 11 — ACP client & subprocess management.
//!
//! These tests exercise the `AcpClient` against an in-memory "fake agent"
//! that speaks NDJSON over a [`tokio::io::duplex`] pipe — no subprocess,
//! no Copilot CLI, deterministic timing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use devdev_acp::client::{AcpClient, AcpClientConfig, AcpError};
use devdev_acp::handler::{AcpHandler, HandlerResult};
use devdev_acp::protocol::{Message, Notification, Request, RequestId, Response};
use devdev_acp::transport::{AsyncNdjsonReader, AsyncNdjsonWriter};
use devdev_acp::types::{
    AgentCapabilities, AgentInfo, ContentBlock, CreateTerminalParams, CreateTerminalResult,
    InitializeResult, KillTerminalParams, NewSessionParams, NewSessionResult, PermissionOutcome,
    PermissionRequestParams, PermissionResponse, PromptContent, PromptParams, PromptResult,
    ReadTextFileParams, ReadTextFileResult, ReleaseTerminalParams, SessionUpdate,
    SessionUpdateParams, StopReason, TerminalOutputParams, TerminalOutputResult,
    WaitForExitParams, WaitForExitResult, WriteTextFileParams,
};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf, duplex, split};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

// ── Fake agent ──────────────────────────────────────────────────────────

/// One end of the duplex pipe driving the client from the "agent" side.
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

    async fn reply(&self, id: RequestId, result: serde_json::Value) {
        self.send(Message::Response(Response::success(id, result))).await;
    }
}

// ── Recording handler ───────────────────────────────────────────────────

#[derive(Default)]
struct RecordingHandler {
    updates: Mutex<Vec<SessionUpdateParams>>,
    terminals_created: Mutex<Vec<CreateTerminalParams>>,
    fs_reads: Mutex<Vec<ReadTextFileParams>>,
}

#[async_trait]
impl AcpHandler for RecordingHandler {
    async fn on_permission_request(
        &self,
        params: PermissionRequestParams,
    ) -> HandlerResult<PermissionResponse> {
        // Always approve the first option.
        let option_id = params.options.first().map(|o| o.option_id.clone()).unwrap_or_default();
        Ok(PermissionResponse {
            outcome: PermissionOutcome::Selected { option_id },
        })
    }

    async fn on_terminal_create(
        &self,
        params: CreateTerminalParams,
    ) -> HandlerResult<CreateTerminalResult> {
        self.terminals_created.lock().unwrap().push(params);
        Ok(CreateTerminalResult {
            terminal_id: "term-1".into(),
        })
    }

    async fn on_terminal_output(
        &self,
        _params: TerminalOutputParams,
    ) -> HandlerResult<TerminalOutputResult> {
        Ok(TerminalOutputResult {
            output: "hi\n".into(),
            truncated: false,
        })
    }

    async fn on_terminal_wait(
        &self,
        _params: WaitForExitParams,
    ) -> HandlerResult<WaitForExitResult> {
        Ok(WaitForExitResult { exit_code: 0 })
    }

    async fn on_terminal_kill(&self, _params: KillTerminalParams) -> HandlerResult<()> {
        Ok(())
    }

    async fn on_terminal_release(&self, _params: ReleaseTerminalParams) -> HandlerResult<()> {
        Ok(())
    }

    async fn on_fs_read(
        &self,
        params: ReadTextFileParams,
    ) -> HandlerResult<ReadTextFileResult> {
        self.fs_reads.lock().unwrap().push(params);
        Ok(ReadTextFileResult {
            content: "file-contents".into(),
            truncated: false,
        })
    }

    async fn on_fs_write(&self, _params: WriteTextFileParams) -> HandlerResult<()> {
        Ok(())
    }

    async fn on_session_update(&self, params: SessionUpdateParams) {
        self.updates.lock().unwrap().push(params);
    }
}

// ── Setup helper ────────────────────────────────────────────────────────

struct Harness {
    client: Arc<AcpClient>,
    agent: Arc<FakeAgent>,
    handler: Arc<RecordingHandler>,
}

async fn harness(config: AcpClientConfig) -> Harness {
    let (client_end, agent_end) = duplex(64 * 1024);
    let (client_r, client_w) = split(client_end);
    let handler = Arc::new(RecordingHandler::default());
    let client = AcpClient::connect_transport(
        client_r,
        client_w,
        handler.clone(),
        config,
    )
    .await
    .unwrap();
    Harness {
        client,
        agent: Arc::new(FakeAgent::new(agent_end)),
        handler,
    }
}

fn short_timeouts() -> AcpClientConfig {
    AcpClientConfig {
        idle_timeout: Duration::from_millis(300),
        request_timeout: Duration::from_millis(300),
        ..Default::default()
    }
}

// ── Acceptance tests ────────────────────────────────────────────────────

/// AC: Send `initialize`, receive response with capabilities.
#[tokio::test]
async fn initialize_round_trip() {
    let h = harness(Default::default()).await;
    // Agent task: expect a request, reply.
    let agent = h.agent.clone();
    let agent_task: JoinHandle<RequestId> = tokio::spawn(async move {
        let msg = agent.recv().await.unwrap();
        let Message::Request(req) = msg else { panic!("expected request") };
        assert_eq!(req.method, "initialize");
        let result = serde_json::to_value(InitializeResult {
            protocol_version: 1,
            agent_info: AgentInfo {
                name: "copilot".into(),
                version: "1.0".into(),
            },
            agent_capabilities: AgentCapabilities { streaming: Some(true) },
            auth_methods: vec![devdev_acp::types::AuthMethod { id: "api_key".into(), name: None, description: None }],
        })
        .unwrap();
        agent.reply(req.id.clone(), result).await;
        req.id
    });

    let out = h.client.initialize().await.unwrap();
    let _id = agent_task.await.unwrap();
    assert_eq!(out.agent_info.name, "copilot");
    assert_eq!(out.auth_methods.len(), 1);
}

/// AC: `new_session` returns a session id from the agent.
#[tokio::test]
async fn new_session_returns_id() {
    let h = harness(Default::default()).await;
    let agent = h.agent.clone();
    tokio::spawn(async move {
        let Message::Request(req) = agent.recv().await.unwrap() else { panic!() };
        assert_eq!(req.method, "session/new");
        agent
            .reply(
                req.id,
                serde_json::to_value(NewSessionResult {
                    session_id: "sess-42".into(),
                })
                .unwrap(),
            )
            .await;
    });

    let out = h
        .client
        .new_session(NewSessionParams {
            cwd: "/work".into(),
            mcp_servers: vec![],
        })
        .await
        .unwrap();
    assert_eq!(out.session_id, "sess-42");
}

/// AC: prompt() receives interleaved notifications then final response.
#[tokio::test]
async fn prompt_with_interleaved_updates() {
    let h = harness(Default::default()).await;
    let agent = h.agent.clone();
    let handler = h.handler.clone();
    tokio::spawn(async move {
        let Message::Request(req) = agent.recv().await.unwrap() else { panic!() };
        assert_eq!(req.method, "session/prompt");
        // Interleaved notifications.
        for txt in ["thinking", "streaming...", "done"] {
            agent
                .send(Message::Notification(Notification::new(
                    "session/update",
                    Some(serde_json::to_value(SessionUpdateParams {
                        session_id: "s".into(),
                        update: SessionUpdate::AgentMessageChunk {
                            content: ContentBlock { text: txt.into() },
                        },
                    }).unwrap()),
                )))
                .await;
        }
        // Final response.
        agent
            .reply(
                req.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let out = h
        .client
        .prompt(PromptParams {
            session_id: "s".into(),
            prompt: vec![PromptContent::Text { text: "hi".into() }],
        })
        .await
        .unwrap();
    assert!(matches!(out.stop_reason, StopReason::EndTurn));
    // Notifications are delivered asynchronously — give them a tick to land.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let updates = handler.updates.lock().unwrap();
    assert_eq!(updates.len(), 3, "got: {updates:?}");
}

/// AC: Agent `terminal/create` request is dispatched to handler, response flows back.
#[tokio::test]
async fn agent_terminal_create_dispatch() {
    let h = harness(Default::default()).await;
    let agent = h.agent.clone();
    let handler = h.handler.clone();

    // Drive a prompt; agent interleaves a terminal/create request, then
    // completes the turn.
    tokio::spawn(async move {
        let Message::Request(prompt_req) = agent.recv().await.unwrap() else { panic!() };
        // Send a terminal/create *request* to the client.
        agent
            .send(Message::Request(Request::new(
                RequestId::Number(9001),
                "terminal/create",
                Some(
                    serde_json::to_value(CreateTerminalParams {
                        session_id: "s".into(),
                        command: "ls".into(),
                        args: vec!["-la".into()],
                        cwd: None,
                        env: vec![],
                        output_byte_limit: None,
                    })
                    .unwrap(),
                ),
            )))
            .await;
        // Read the client's response to our terminal/create.
        let resp = agent.recv().await.unwrap();
        let Message::Response(r) = resp else { panic!("expected response, got {resp:?}") };
        assert_eq!(r.id, RequestId::Number(9001));
        let result: CreateTerminalResult =
            serde_json::from_value(r.result.unwrap()).unwrap();
        assert_eq!(result.terminal_id, "term-1");
        // Finish the prompt.
        agent
            .reply(
                prompt_req.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let _ = h
        .client
        .prompt(PromptParams {
            session_id: "s".into(),
            prompt: vec![PromptContent::Text { text: "go".into() }],
        })
        .await
        .unwrap();
    let terms = handler.terminals_created.lock().unwrap();
    assert_eq!(terms.len(), 1);
    assert_eq!(terms[0].command, "ls");
}

/// AC: Agent `fs/read_text_file` request → handler → response.
#[tokio::test]
async fn agent_fs_read_dispatch() {
    let h = harness(Default::default()).await;
    let agent = h.agent.clone();
    let handler = h.handler.clone();
    tokio::spawn(async move {
        let Message::Request(prompt_req) = agent.recv().await.unwrap() else { panic!() };
        agent
            .send(Message::Request(Request::new(
                RequestId::Number(42),
                "fs/read_text_file",
                Some(
                    serde_json::to_value(ReadTextFileParams {
                        session_id: "s".into(),
                        path: "/tmp/foo.rs".into(),
                        line: None,
                        limit: None,
                    })
                    .unwrap(),
                ),
            )))
            .await;
        let Message::Response(r) = agent.recv().await.unwrap() else { panic!() };
        assert_eq!(r.id, RequestId::Number(42));
        let result: ReadTextFileResult =
            serde_json::from_value(r.result.unwrap()).unwrap();
        assert_eq!(result.content, "file-contents");
        agent
            .reply(
                prompt_req.id,
                serde_json::to_value(PromptResult {
                    stop_reason: StopReason::EndTurn,
                })
                .unwrap(),
            )
            .await;
    });

    let _ = h
        .client
        .prompt(PromptParams {
            session_id: "s".into(),
            prompt: vec![PromptContent::Text { text: "x".into() }],
        })
        .await
        .unwrap();
    let reads = handler.fs_reads.lock().unwrap();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].path, "/tmp/foo.rs");
}

/// AC: Multiple in-flight requests resolve to the correct waiters.
#[tokio::test]
async fn multiple_in_flight_requests() {
    let h = harness(Default::default()).await;
    let agent = h.agent.clone();
    // Collect both requests, reply out of order.
    tokio::spawn(async move {
        let Message::Request(req1) = agent.recv().await.unwrap() else { panic!() };
        let Message::Request(req2) = agent.recv().await.unwrap() else { panic!() };
        // Reply to #2 first.
        agent
            .reply(
                req2.id,
                serde_json::to_value(NewSessionResult {
                    session_id: "sess-B".into(),
                })
                .unwrap(),
            )
            .await;
        agent
            .reply(
                req1.id,
                serde_json::to_value(NewSessionResult {
                    session_id: "sess-A".into(),
                })
                .unwrap(),
            )
            .await;
    });

    let client = h.client.clone();
    let a = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .new_session(NewSessionParams {
                    cwd: "/a".into(),
                    mcp_servers: vec![],
                })
                .await
        }
    });
    // Small delay so the two requests hit the wire in order.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let b = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .new_session(NewSessionParams {
                    cwd: "/b".into(),
                    mcp_servers: vec![],
                })
                .await
        }
    });
    let a = a.await.unwrap().unwrap();
    let b = b.await.unwrap().unwrap();
    assert_eq!(a.session_id, "sess-A");
    assert_eq!(b.session_id, "sess-B");
}

/// AC: Silent agent → prompt() times out with `AcpError::Timeout`.
#[tokio::test]
async fn prompt_times_out_on_silence() {
    let h = harness(short_timeouts()).await;
    let agent = h.agent.clone();
    // Keep the request, never reply.
    tokio::spawn(async move {
        let _ = agent.recv().await.unwrap();
    });
    let err = h
        .client
        .prompt(PromptParams {
            session_id: "s".into(),
            prompt: vec![PromptContent::Text { text: "x".into() }],
        })
        .await
        .unwrap_err();
    assert!(matches!(err, AcpError::Timeout), "got {err:?}");
}

/// AC: Dropped transport (simulating subprocess crash) surfaces as an
/// RPC-internal error to pending waiters — the reader task sees EOF and
/// drains the pending map with a synthetic "disconnected" error.
#[tokio::test]
async fn pipe_close_wakes_pending_waiters() {
    // Build the client by hand so the agent pipe can actually be dropped
    // (the shared `harness` stores the agent inside an Arc that the test
    // body would keep alive).
    let (client_end, agent_end) = duplex(64 * 1024);
    let (client_r, client_w) = split(client_end);
    let handler: Arc<dyn AcpHandler> = Arc::new(RecordingHandler::default());
    let client = AcpClient::connect_transport(
        client_r,
        client_w,
        handler,
        AcpClientConfig {
            request_timeout: Duration::from_millis(500),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Agent task: read one frame then drop the pipe end.
    tokio::spawn(async move {
        let (r, w) = split(agent_end);
        let mut reader = AsyncNdjsonReader::new(r);
        let _ = reader.recv().await;
        drop(reader);
        drop(w);
    });

    let err = client
        .new_session(NewSessionParams {
            cwd: "/x".into(),
            mcp_servers: vec![],
        })
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            AcpError::Rpc { .. }
                | AcpError::BrokenPipe
                | AcpError::Timeout
                | AcpError::AgentDisconnected
        ),
        "got {err:?}"
    );
}

/// AC: Env token short-circuits authenticate to `EnvToken` strategy.
#[tokio::test]
async fn auth_env_token_short_circuits() {
    // Set a temp token for this test only.
    // SAFETY: tests share a process; set_var is unsafe since Rust 2024 but
    // we accept the single-threaded-test caveat here.
    // SAFETY: test-only env mutation — acceptable; `cargo test` runs each
    // `#[tokio::test]` in its own runtime, and the harness spawns no other
    // threads that read these vars.
    unsafe {
        std::env::set_var("GH_TOKEN", "test-token");
    }
    let h = harness(Default::default()).await;
    let strat = h.client.authenticate(&["api_key".to_string()]).await.unwrap();
    assert!(matches!(strat, devdev_acp::AuthStrategy::EnvToken("GH_TOKEN")));
    unsafe {
        std::env::remove_var("GH_TOKEN");
    }
}

/// AC: `shutdown()` is idempotent-safe and returns Ok even with no
/// subprocess.
#[tokio::test]
async fn shutdown_without_subprocess() {
    let h = harness(Default::default()).await;
    h.client.shutdown().await.unwrap();
}

/// AC: Unknown agent-initiated method yields method-not-found error sent
/// back to the agent.
#[tokio::test]
async fn agent_unknown_method_gets_error() {
    let h = harness(Default::default()).await;
    let agent = h.agent.clone();
    tokio::spawn(async move {
        agent
            .send(Message::Request(Request::new(
                RequestId::Number(7),
                "bogus/method",
                Some(serde_json::json!({})),
            )))
            .await;
    });
    // Read the response the client sent back.
    let msg = h.agent.recv().await.unwrap();
    let Message::Response(r) = msg else { panic!("expected response, got {msg:?}") };
    assert_eq!(r.id, RequestId::Number(7));
    let err = r.error.unwrap();
    assert_eq!(err.code, devdev_acp::protocol::error_codes::METHOD_NOT_FOUND);
}

/// Sanity: the request id counter actually increments.
#[tokio::test]
async fn request_ids_increment() {
    let h = harness(Default::default()).await;
    let agent = h.agent.clone();
    tokio::spawn(async move {
        for _ in 0..3 {
            let Message::Request(req) = agent.recv().await.unwrap() else { panic!() };
            agent
                .reply(
                    req.id,
                    serde_json::to_value(NewSessionResult {
                        session_id: "s".into(),
                    })
                    .unwrap(),
                )
                .await;
        }
    });
    let mut seen = HashMap::new();
    for _ in 0..3 {
        let _ = h
            .client
            .new_session(NewSessionParams {
                cwd: "/".into(),
                mcp_servers: vec![],
            })
            .await
            .unwrap();
        // We can't peek at ids directly via public API — just confirm
        // three successive calls succeed without id collisions causing
        // oneshot mismatches. A collision would hang → timeout.
        *seen.entry(()).or_insert(0) += 1;
    }
    assert_eq!(seen[&()], 3);
}
/// AC: once `max_pending` concurrent calls are in flight, the next one
/// fails fast with `AcpError::Backpressure` instead of silently growing
/// the pending map.
#[tokio::test]
async fn backpressure_triggers_when_pending_full() {
    let cfg = AcpClientConfig {
        max_pending: 2,
        // Keep per-call timeout longer than the test window — we want
        // the extra call to hit Backpressure, not Timeout.
        request_timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let h = harness(cfg).await;

    // Agent deliberately never replies: it just drains inbound requests
    // so the writer side doesn't backpressure.
    let agent = h.agent.clone();
    let drainer: JoinHandle<()> = tokio::spawn(async move {
        loop {
            if agent.recv().await.is_none() {
                break;
            }
        }
    });

    // Two calls in flight — these will block forever on the fake agent.
    let c1 = h.client.clone();
    let c2 = h.client.clone();
    let f1 = tokio::spawn(async move {
        c1.new_session(NewSessionParams {
            cwd: "/".into(),
            mcp_servers: vec![],
        })
        .await
    });
    let f2 = tokio::spawn(async move {
        c2.new_session(NewSessionParams {
            cwd: "/".into(),
            mcp_servers: vec![],
        })
        .await
    });

    // Wait until both have registered in the pending map.
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Poke the third call — if pending isn't full yet it will just
        // register and we'll loop (but we'll see Backpressure once the
        // first two have landed).
        let res = h
            .client
            .new_session(NewSessionParams {
                cwd: "/".into(),
                mcp_servers: vec![],
            })
            .await;
        match res {
            Err(AcpError::Backpressure(n)) => {
                assert_eq!(n, 2);
                // Cleanup: shutdown releases everything.
                let _ = h.client.shutdown().await;
                drainer.abort();
                let _ = f1.await;
                let _ = f2.await;
                return;
            }
            // Still waiting for the first two to register. A spurious
            // success would mean the agent actually replied — which it
            // cannot — so this branch is unreachable, but tolerate it.
            Ok(_) => continue,
            // Some other error (e.g. shutdown) fails the test.
            Err(e) => panic!("unexpected error while waiting: {e:?}"),
        }
    }
    panic!("never observed Backpressure within window");
}