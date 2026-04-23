//! Real ACP-backed [`SessionBackend`] implementation.
//!
//! Spawns one `copilot --acp --allow-all-tools` subprocess on first use
//! and multiplexes every task's logical session onto it via
//! [`AcpClient::new_session`]. Per-session `session/update` notifications
//! are routed back to the originating prompt call through a small
//! `HashMap<session_id, mpsc::Sender<ResponseChunk>>` owned by the
//! handler.
//!
//! See `capabilities/21-session-router.md` for the PoC result that
//! validated one-subprocess-multiplex; this file implements that
//! decision.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use devdev_acp::handler::{AcpHandler, HandlerResult};
use devdev_acp::protocol::{RpcError, error_codes};
use devdev_acp::types::{
    CreateTerminalParams, CreateTerminalResult, KillTerminalParams, NewSessionParams,
    PermissionRequestParams, PermissionResponse, PromptContent, PromptParams, ReadTextFileParams,
    ReadTextFileResult, ReleaseTerminalParams, SessionUpdate, SessionUpdateParams,
    TerminalOutputParams, TerminalOutputResult, WaitForExitParams, WaitForExitResult,
    WriteTextFileParams,
};
use devdev_acp::{AcpClient, AcpClientConfig, AcpError};
use devdev_daemon::router::{AgentResponse, ResponseChunk, RouterError, SessionBackend};
use tokio::sync::{Mutex, OnceCell, mpsc};

/// Session backend that talks ACP/NDJSON to a single `copilot` subprocess.
pub struct AcpSessionBackend {
    program: String,
    args: Vec<String>,
    inner: OnceCell<Inner>,
}

struct Inner {
    client: Arc<AcpClient>,
    handler: Arc<RouterHandler>,
}

impl AcpSessionBackend {
    pub fn new(program: String, args: Vec<String>) -> Self {
        Self {
            program,
            args,
            inner: OnceCell::new(),
        }
    }

    /// Lazily spawn the subprocess, run `initialize`, and (if the agent
    /// advertises methods) run `authenticate`. Idempotent: subsequent
    /// calls return the already-initialized [`Inner`].
    async fn ensure_started(&self) -> Result<&Inner, RouterError> {
        self.inner
            .get_or_try_init(|| async {
                let handler = Arc::new(RouterHandler::default());
                let argv: Vec<&str> = self.args.iter().map(String::as_str).collect();
                let client = AcpClient::connect_process(
                    &self.program,
                    &argv,
                    handler.clone() as Arc<dyn AcpHandler>,
                    AcpClientConfig::default(),
                )
                .await
                .map_err(acp_to_router)?;
                let init = client.initialize().await.map_err(acp_to_router)?;
                // If the agent advertises auth methods, try them. The
                // Copilot CLI advertises `copilot-login` even when already
                // authenticated; it returns `{}` in that case. A `NoAuth`
                // result here means "nothing to do" — treat as success.
                let methods: Vec<String> =
                    init.auth_methods.iter().map(|m| m.kind.clone()).collect();
                if !methods.is_empty() {
                    match client.authenticate(&methods).await {
                        Ok(_) => {}
                        Err(AcpError::NoAuth) => {}
                        Err(e) => return Err(acp_to_router(e)),
                    }
                }
                Ok::<Inner, RouterError>(Inner { client, handler })
            })
            .await
    }
}

fn acp_to_router(e: AcpError) -> RouterError {
    match e {
        AcpError::SubprocessCrashed(_)
        | AcpError::AgentDisconnected
        | AcpError::BrokenPipe => RouterError::SubprocessCrashed,
        other => RouterError::Backend(other.to_string()),
    }
}

#[async_trait]
impl SessionBackend for AcpSessionBackend {
    async fn create_session(&self, cwd: &str) -> Result<String, RouterError> {
        let inner = self.ensure_started().await?;
        let result = inner
            .client
            .new_session(NewSessionParams {
                cwd: cwd.to_owned(),
                mcp_servers: vec![],
            })
            .await
            .map_err(acp_to_router)?;
        Ok(result.session_id)
    }

    async fn send_prompt(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<AgentResponse, RouterError> {
        let inner = self.ensure_started().await?;
        let (tx, mut rx) = mpsc::channel::<ResponseChunk>(64);
        inner.handler.register(session_id.to_owned(), tx).await;

        let client = inner.client.clone();
        let params = PromptParams {
            session_id: session_id.to_owned(),
            prompt: vec![PromptContent::Text {
                text: text.to_owned(),
            }],
        };
        let mut prompt_fut = Box::pin(async move { client.prompt(params).await });

        let mut text_buf = String::new();
        let prompt_result = loop {
            tokio::select! {
                res = &mut prompt_fut => break res,
                Some(chunk) = rx.recv() => {
                    if let ResponseChunk::Text(t) = chunk {
                        text_buf.push_str(&t);
                    }
                }
            }
        };

        // Drain any chunks that landed between the prompt response and
        // our select loop exit.
        while let Ok(chunk) = rx.try_recv() {
            if let ResponseChunk::Text(t) = chunk {
                text_buf.push_str(&t);
            }
        }
        inner.handler.unregister(session_id).await;

        let prompt_result = prompt_result.map_err(acp_to_router)?;
        Ok(AgentResponse {
            text: text_buf,
            stop_reason: prompt_result.stop_reason.as_str().to_owned(),
        })
    }

    async fn send_prompt_streaming(
        &self,
        session_id: &str,
        text: &str,
        tx: mpsc::Sender<ResponseChunk>,
    ) -> Result<(), RouterError> {
        let inner = self.ensure_started().await?;
        inner
            .handler
            .register(session_id.to_owned(), tx.clone())
            .await;

        let prompt_result = inner
            .client
            .prompt(PromptParams {
                session_id: session_id.to_owned(),
                prompt: vec![PromptContent::Text {
                    text: text.to_owned(),
                }],
            })
            .await;
        inner.handler.unregister(session_id).await;

        let prompt_result = prompt_result.map_err(acp_to_router)?;
        let _ = tx
            .send(ResponseChunk::Done {
                stop_reason: prompt_result.stop_reason.as_str().to_owned(),
            })
            .await;
        Ok(())
    }

    async fn destroy_session(&self, session_id: &str) -> Result<(), RouterError> {
        if let Some(inner) = self.inner.get() {
            inner.handler.unregister(session_id).await;
            // Best-effort cancel in case a turn is in flight. Swallow
            // errors — `cancel` failures on an idle session are expected.
            let _ = inner.client.cancel(session_id).await;
        }
        Ok(())
    }
}

// ── Handler: fan session/update notifications out to per-session senders

/// ACP handler owned by the backend. Routes `session/update`
/// notifications to the [`mpsc::Sender`] registered for each
/// `session_id`. Agent-initiated tool/fs/permission requests are
/// rejected — the Copilot CLI runs its own tools when launched with
/// `--allow-all-tools`, so those hooks are never exercised on this path.
#[derive(Default)]
struct RouterHandler {
    senders: Mutex<HashMap<String, mpsc::Sender<ResponseChunk>>>,
}

impl RouterHandler {
    async fn register(&self, session_id: String, tx: mpsc::Sender<ResponseChunk>) {
        self.senders.lock().await.insert(session_id, tx);
    }

    async fn unregister(&self, session_id: &str) {
        self.senders.lock().await.remove(session_id);
    }
}

#[async_trait]
impl AcpHandler for RouterHandler {
    async fn on_permission_request(
        &self,
        _params: PermissionRequestParams,
    ) -> HandlerResult<PermissionResponse> {
        Err(not_supported("session/request_permission"))
    }

    async fn on_terminal_create(
        &self,
        _params: CreateTerminalParams,
    ) -> HandlerResult<CreateTerminalResult> {
        Err(not_supported("terminal/create"))
    }

    async fn on_terminal_output(
        &self,
        _params: TerminalOutputParams,
    ) -> HandlerResult<TerminalOutputResult> {
        Err(not_supported("terminal/output"))
    }

    async fn on_terminal_wait(
        &self,
        _params: WaitForExitParams,
    ) -> HandlerResult<WaitForExitResult> {
        Err(not_supported("terminal/wait_for_exit"))
    }

    async fn on_terminal_kill(&self, _params: KillTerminalParams) -> HandlerResult<()> {
        Err(not_supported("terminal/kill"))
    }

    async fn on_terminal_release(&self, _params: ReleaseTerminalParams) -> HandlerResult<()> {
        Err(not_supported("terminal/release"))
    }

    async fn on_fs_read(
        &self,
        _params: ReadTextFileParams,
    ) -> HandlerResult<ReadTextFileResult> {
        Err(not_supported("fs/read_text_file"))
    }

    async fn on_fs_write(&self, _params: WriteTextFileParams) -> HandlerResult<()> {
        Err(not_supported("fs/write_text_file"))
    }

    async fn on_session_update(&self, params: SessionUpdateParams) {
        let SessionUpdateParams { session_id, update } = params;
        let text = match update {
            SessionUpdate::AgentMessageChunk { content } => content.text,
            // Thought chunks and tool-call notifications don't belong in
            // the user-visible response stream. Drop them here; a future
            // change can widen `ResponseChunk` to carry them through.
            _ => return,
        };
        if text.is_empty() {
            return;
        }
        let sender = {
            let map = self.senders.lock().await;
            map.get(&session_id).cloned()
        };
        if let Some(tx) = sender {
            // `try_send` keeps the handler non-blocking; the reader task
            // must never wait on a slow consumer.
            let _ = tx.try_send(ResponseChunk::Text(text));
        }
    }
}

fn not_supported(method: &str) -> RpcError {
    RpcError {
        code: error_codes::METHOD_NOT_FOUND,
        message: format!("{method} not supported by devdev session router backend"),
        data: None,
    }
}
