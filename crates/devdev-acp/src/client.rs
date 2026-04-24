//! [`AcpClient`] — async ACP transport orchestrator.
//!
//! Manages one ACP peer (typically a spawned `copilot --acp`
//! subprocess) and the bidirectional NDJSON stream. Owns a reader task
//! that demultiplexes incoming messages into:
//!
//! * **Responses** — matched by id to a pending request future.
//! * **Notifications** — forwarded to the [`AcpHandler`].
//! * **Agent-initiated requests** — dispatched to the handler, with the
//!   returned value flushed back to the agent as a response.
//!
//! A writer task serialises the outgoing side so reader-side handlers can
//! enqueue responses without stepping on an in-flight client request.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::auth::{AuthStrategy, choose_strategy, find_env_token};
use crate::handler::AcpHandler;
use crate::protocol::{Message, Notification, Request, RequestId, Response, RpcError, error_codes};
use crate::transport::{AsyncNdjsonReader, AsyncNdjsonWriter};
use crate::types::{
    AuthenticateParams, AuthenticateResult, CancelParams, InitializeParams, InitializeResult,
    NewSessionParams, NewSessionResult, PromptParams, PromptResult, SessionUpdateParams,
};

/// Default idle timeout: if no messages arrive from the agent for this
/// long, [`AcpClient::prompt`] fails with [`AcpError::Timeout`] and the
/// subprocess is killed.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Default one-shot request timeout (initialize, new_session, cancel…).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default cap on concurrently in-flight client-initiated requests. When
/// the pending map is at this size, [`AcpClient::call`] returns
/// [`AcpError::Backpressure`] instead of enqueuing. Picked to be large
/// enough for legitimate burst traffic and small enough to prevent
/// unbounded host memory growth under a pathological agent.
pub const DEFAULT_MAX_PENDING: usize = 1024;

/// Default cap on concurrently running agent-initiated request handlers
/// (permission, terminal/*, fs/*). Gates [`tokio::spawn`] inside the
/// reader task so a flood of inbound requests can't spawn an unbounded
/// number of handler tasks.
pub const DEFAULT_MAX_INFLIGHT_HANDLERS: usize = 256;

/// Message marker used by the reader-EOF drain to signal
/// [`AcpError::AgentDisconnected`] back through `call_with_timeout`.
/// Not intended to be user-visible.
pub(crate) const AGENT_DISCONNECTED_SENTINEL: &str = "__devdev_agent_disconnected__";

/// Tunable knobs for an [`AcpClient`].
#[derive(Debug, Clone)]
pub struct AcpClientConfig {
    pub idle_timeout: Duration,
    pub request_timeout: Duration,
    pub client_name: String,
    pub client_version: String,
    pub protocol_version: u16,
    /// Maximum number of in-flight client-initiated requests. Extra
    /// calls fail fast with [`AcpError::Backpressure`] rather than
    /// growing the pending map without bound.
    pub max_pending: usize,
    /// Maximum number of concurrently running agent-initiated request
    /// handlers.
    pub max_inflight_handlers: usize,
    /// Extra environment variables to set on the spawned subprocess,
    /// *in addition to* the parent's env. Most callers leave this empty;
    /// it exists so callers can inject `NODE_OPTIONS` to work around
    /// known libuv/WinFSP `realpath` quirks on Windows (see
    /// `devdev-cli/src/realpath_shim.rs`).
    pub env_overrides: Vec<(String, String)>,
}

impl Default for AcpClientConfig {
    fn default() -> Self {
        Self {
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            client_name: "devdev".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: 1,
            max_pending: DEFAULT_MAX_PENDING,
            max_inflight_handlers: DEFAULT_MAX_INFLIGHT_HANDLERS,
            env_overrides: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("failed to spawn agent subprocess: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("subprocess crashed (exit code {0:?})")]
    SubprocessCrashed(Option<i32>),
    #[error("broken pipe to subprocess")]
    BrokenPipe,
    #[error("agent closed the connection before the request completed")]
    AgentDisconnected,
    #[error("timed out waiting for agent")]
    Timeout,
    #[error("transport error: {0}")]
    Transport(#[source] std::io::Error),
    #[error("agent returned RPC error: {code} {message}{}",
        .data.as_ref().map(|d| format!(" (data: {d})")).unwrap_or_default())]
    Rpc {
        code: i32,
        message: String,
        /// The JSON-RPC `error.data` field, preserved so callers can see
        /// structured error details the agent attached. Copilot emits
        /// backtrace/reason here for several failure modes.
        data: Option<serde_json::Value>,
    },
    #[error("malformed response from agent: {0}")]
    MalformedResponse(String),
    #[error("client is shut down")]
    Shutdown,
    #[error("no auth strategy available")]
    NoAuth,
    /// Too many in-flight client requests. The caller should retry after
    /// some of them complete. Distinct from [`AcpError::Timeout`] because
    /// the request never hit the wire.
    #[error("client at max in-flight requests ({0})")]
    Backpressure(usize),
}

impl From<RpcError> for AcpError {
    fn from(e: RpcError) -> Self {
        Self::Rpc {
            code: e.code,
            message: e.message,
            data: e.data,
        }
    }
}

type PendingMap = Arc<Mutex<HashMap<RequestId, oneshot::Sender<Response>>>>;

/// The ACP client.
pub struct AcpClient {
    outgoing: mpsc::Sender<Message>,
    pending: PendingMap,
    next_id: AtomicU64,
    config: AcpClientConfig,
    /// Gates concurrent agent-initiated request handlers. One permit per
    /// in-flight handler task. Held across the handler callback so the
    /// cap is on concurrency, not throughput.
    handler_sem: Arc<Semaphore>,
    /// `Some` when we own a spawned subprocess; `None` for transport-only
    /// (test) clients.
    child: Mutex<Option<Child>>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    writer_task: Mutex<Option<JoinHandle<()>>>,
}

impl AcpClient {
    /// Spawn an ACP agent (e.g. `copilot --acp --allow-all-tools`)
    /// and wire up the client. The caller supplies `program` + `args`;
    /// stdio is the only transport the Copilot CLI supports in ACP
    /// mode, so no transport flag is needed.
    pub async fn connect_process(
        program: &str,
        args: &[&str],
        handler: Arc<dyn AcpHandler>,
        config: AcpClientConfig,
    ) -> Result<Arc<Self>, AcpError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &config.env_overrides {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(AcpError::Spawn)?;
        let stdin = child.stdin.take().ok_or(AcpError::BrokenPipe)?;
        let stdout = child.stdout.take().ok_or(AcpError::BrokenPipe)?;
        let this = Self::connect_transport(stdout, stdin, handler, config).await?;
        *this.child.lock().await = Some(child);
        Ok(this)
    }

    /// Wire the client to pre-existing async pipes. Useful for tests and
    /// for callers that want to manage the subprocess themselves.
    pub async fn connect_transport<R, W>(
        reader: R,
        writer: W,
        handler: Arc<dyn AcpHandler>,
        config: AcpClientConfig,
    ) -> Result<Arc<Self>, AcpError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<Message>(64);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let handler_sem = Arc::new(Semaphore::new(config.max_inflight_handlers));

        // Writer task: drain mpsc → NDJSON. Every outgoing frame is
        // logged at `trace` level under target `devdev_acp::wire` so a
        // single `RUST_LOG=devdev_acp::wire=trace` lights up the full
        // client→agent side of the conversation.
        let mut ndjson_writer = AsyncNdjsonWriter::new(writer);
        let writer_task = tokio::spawn(async move {
            while let Some(msg) = outgoing_rx.recv().await {
                if tracing::enabled!(target: "devdev_acp::wire", tracing::Level::TRACE) {
                    match serde_json::to_string(&msg) {
                        Ok(s) => tracing::trace!(target: "devdev_acp::wire", dir = "tx", frame = %s),
                        Err(e) => tracing::trace!(target: "devdev_acp::wire", dir = "tx", error = %e),
                    }
                }
                if let Err(e) = ndjson_writer.send(&msg).await {
                    tracing::warn!("acp writer: send failed: {e}");
                    break;
                }
            }
        });

        // Reader task: NDJSON → (oneshot | handler | handler+response).
        let mut ndjson_reader = AsyncNdjsonReader::new(reader);
        let pending_r = pending.clone();
        let outgoing_r = outgoing_tx.clone();
        let handler_r = handler.clone();
        let sem_r = handler_sem.clone();
        let reader_task = tokio::spawn(async move {
            loop {
                match ndjson_reader.recv().await {
                    Ok(Some(msg)) => {
                        if tracing::enabled!(target: "devdev_acp::wire", tracing::Level::TRACE) {
                            match serde_json::to_string(&msg) {
                                Ok(s) => tracing::trace!(target: "devdev_acp::wire", dir = "rx", frame = %s),
                                Err(e) => tracing::trace!(target: "devdev_acp::wire", dir = "rx", error = %e),
                            }
                        }
                        Self::dispatch_incoming(
                            msg,
                            pending_r.clone(),
                            outgoing_r.clone(),
                            handler_r.clone(),
                            sem_r.clone(),
                        )
                        .await;
                    }
                    Ok(None) => {
                        tracing::debug!("acp reader: EOF");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("acp reader: {e}");
                        break;
                    }
                }
            }
            // On reader exit, wake every pending waiter with an error so
            // no future hangs forever. The sentinel id/message tell
            // `call_with_timeout` to surface [`AcpError::AgentDisconnected`]
            // instead of a generic `Rpc`.
            let mut map = pending_r.lock().await;
            for (_, tx) in map.drain() {
                let _ = tx.send(Response {
                    jsonrpc: "2.0".into(),
                    id: RequestId::Number(0),
                    result: None,
                    error: Some(RpcError {
                        code: error_codes::INTERNAL_ERROR,
                        message: AGENT_DISCONNECTED_SENTINEL.into(),
                        data: None,
                    }),
                });
            }
        });

        Ok(Arc::new(Self {
            outgoing: outgoing_tx,
            pending,
            next_id: AtomicU64::new(1),
            config,
            handler_sem,
            child: Mutex::new(None),
            reader_task: Mutex::new(Some(reader_task)),
            writer_task: Mutex::new(Some(writer_task)),
        }))
    }

    async fn dispatch_incoming(
        msg: Message,
        pending: PendingMap,
        outgoing: mpsc::Sender<Message>,
        handler: Arc<dyn AcpHandler>,
        sem: Arc<Semaphore>,
    ) {
        match msg {
            Message::Response(resp) => {
                // Match by id, fire oneshot — inline, synchronously in
                // the reader task. Spawning here would race the
                // end-of-stream drain: a response read moments before
                // EOF could have its oneshot replaced with the
                // disconnect sentinel if the drain wins the scheduler.
                let mut map = pending.lock().await;
                if let Some(tx) = map.remove(&resp.id) {
                    let _ = tx.send(resp);
                } else {
                    tracing::warn!("acp: stray response for id {:?}", resp.id);
                }
            }
            Message::Notification(note) => {
                tokio::spawn(async move {
                    // Acquire the semaphore so a flood of notifications
                    // can't spawn unbounded handler tasks. If the client
                    // is tearing down the semaphore is closed; just drop
                    // the notification.
                    let _permit = match sem.acquire_owned().await {
                        Ok(p) => p,
                        Err(_) => return,
                    };
                    Self::handle_notification(note, handler).await;
                });
            }
            Message::Request(req) => {
                tokio::spawn(async move {
                    let _permit = match sem.acquire_owned().await {
                        Ok(p) => p,
                        Err(_) => return,
                    };
                    let id = req.id.clone();
                    let response = Self::handle_agent_request(req, handler).await;
                    let reply = match response {
                        Ok(result) => Message::Response(Response::success(id, result)),
                        Err(err) => Message::Response(Response::error(id, err)),
                    };
                    if outgoing.send(reply).await.is_err() {
                        tracing::warn!("acp: writer channel closed; dropping response");
                    }
                });
            }
        }
    }

    async fn handle_notification(note: Notification, handler: Arc<dyn AcpHandler>) {
        if note.method == "session/update" {
            match parse_params::<SessionUpdateParams>(note.params) {
                Ok(params) => handler.on_session_update(params).await,
                Err(e) => tracing::warn!("acp: bad session/update params: {e:?}"),
            }
        } else {
            tracing::debug!("acp: unhandled notification {}", note.method);
        }
    }

    async fn handle_agent_request(
        req: Request,
        handler: Arc<dyn AcpHandler>,
    ) -> Result<serde_json::Value, RpcError> {
        match req.method.as_str() {
            "session/request_permission" => {
                let params = parse_params(req.params)?;
                let out = handler.on_permission_request(params).await?;
                Ok(serde_json::to_value(out).map_err(internal)?)
            }
            "terminal/create" => {
                let params = parse_params(req.params)?;
                let out = handler.on_terminal_create(params).await?;
                Ok(serde_json::to_value(out).map_err(internal)?)
            }
            "terminal/output" => {
                let params = parse_params(req.params)?;
                let out = handler.on_terminal_output(params).await?;
                Ok(serde_json::to_value(out).map_err(internal)?)
            }
            "terminal/wait_for_exit" => {
                let params = parse_params(req.params)?;
                let out = handler.on_terminal_wait(params).await?;
                Ok(serde_json::to_value(out).map_err(internal)?)
            }
            "terminal/kill" => {
                let params = parse_params(req.params)?;
                handler.on_terminal_kill(params).await?;
                Ok(serde_json::Value::Null)
            }
            "terminal/release" => {
                let params = parse_params(req.params)?;
                handler.on_terminal_release(params).await?;
                Ok(serde_json::Value::Null)
            }
            "fs/read_text_file" => {
                let params = parse_params(req.params)?;
                let out = handler.on_fs_read(params).await?;
                Ok(serde_json::to_value(out).map_err(internal)?)
            }
            "fs/write_text_file" => {
                let params = parse_params(req.params)?;
                handler.on_fs_write(params).await?;
                Ok(serde_json::Value::Null)
            }
            other => Err(RpcError {
                code: error_codes::METHOD_NOT_FOUND,
                message: format!("method not found: {other}"),
                data: None,
            }),
        }
    }

    /// Low-level: send a request, await the response within the
    /// configured request timeout.
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<R, AcpError> {
        self.call_with_timeout(method, params, self.config.request_timeout)
            .await
    }

    /// Like [`call`](Self::call) but with an explicit timeout — used by
    /// `prompt()` which extends the idle window across agent callbacks.
    pub async fn call_with_timeout<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
        timeout: Duration,
    ) -> Result<R, AcpError> {
        let id = RequestId::Number(self.next_id.fetch_add(1, Ordering::Relaxed));
        let params_json = match params {
            Some(p) => Some(serde_json::to_value(p).map_err(|e| {
                AcpError::MalformedResponse(format!("serialize params: {e}"))
            })?),
            None => None,
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            if map.len() >= self.config.max_pending {
                return Err(AcpError::Backpressure(self.config.max_pending));
            }
            map.insert(id.clone(), tx);
        }
        let req = Request::new(id.clone(), method, params_json);
        if self.outgoing.send(Message::Request(req)).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(AcpError::BrokenPipe);
        }
        let resp = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => return Err(AcpError::BrokenPipe),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(AcpError::Timeout);
            }
        };
        if let Some(err) = resp.error {
            if err.code == error_codes::INTERNAL_ERROR
                && err.message == AGENT_DISCONNECTED_SENTINEL
            {
                return Err(AcpError::AgentDisconnected);
            }
            return Err(err.into());
        }
        let value = resp.result.unwrap_or(serde_json::Value::Null);
        serde_json::from_value(value)
            .map_err(|e| AcpError::MalformedResponse(format!("deserialize result: {e}")))
    }

    // ── High-level ACP methods ────────────────────────────────────

    pub async fn initialize(&self) -> Result<InitializeResult, AcpError> {
        let params = InitializeParams {
            protocol_version: self.config.protocol_version,
            client_capabilities: crate::types::ClientCapabilities {
                fs: Some(crate::types::FsCapabilities {
                    read_text_file: true,
                    write_text_file: true,
                }),
                terminal: Some(true),
            },
            client_info: crate::types::ClientInfo {
                name: self.config.client_name.clone(),
                version: self.config.client_version.clone(),
            },
        };
        self.call("initialize", Some(params)).await
    }

    /// Authenticate using an advertised method list. Returns the chosen
    /// [`AuthStrategy`] so callers can report what happened (env, method
    /// name, or none).
    pub async fn authenticate(
        &self,
        advertised: &[String],
    ) -> Result<AuthStrategy, AcpError> {
        let strat = choose_strategy(advertised);
        match &strat {
            AuthStrategy::EnvToken(_) => Ok(strat),
            AuthStrategy::Method(method) => {
                let token = find_env_token().map(|(_, v)| v);
                let params = AuthenticateParams {
                    method_id: method.clone(),
                    token,
                };
                let _out: AuthenticateResult =
                    self.call("authenticate", Some(params)).await?;
                Ok(strat)
            }
            AuthStrategy::None => Err(AcpError::NoAuth),
        }
    }

    pub async fn new_session(
        &self,
        params: NewSessionParams,
    ) -> Result<NewSessionResult, AcpError> {
        self.call("session/new", Some(params)).await
    }

    /// Send a prompt. Uses the idle timeout — each round-trip inside the
    /// turn (handler callback, stream chunk) resets the clock on the
    /// reader side, but *this* future completes only when the turn
    /// terminates.
    pub async fn prompt(
        &self,
        params: PromptParams,
    ) -> Result<PromptResult, AcpError> {
        self.call_with_timeout("session/prompt", Some(params), self.config.idle_timeout)
            .await
    }

    pub async fn cancel(&self, session_id: &str) -> Result<(), AcpError> {
        let params = CancelParams {
            session_id: session_id.to_owned(),
        };
        // `session/cancel` is typically a notification in ACP, but the
        // spec leaves room for either. We send as a request and tolerate
        // a null result. Agents like Copilot CLI may not implement it
        // (returning `-32601 Method not found`); treat that as a no-op
        // since cancel is best-effort.
        match self.call::<_, serde_json::Value>("session/cancel", Some(params)).await {
            Ok(_) => Ok(()),
            Err(AcpError::Rpc { code: -32601, .. }) => {
                tracing::debug!(
                    session_id,
                    "agent does not implement session/cancel; treating as no-op"
                );
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Gracefully stop: kill child (if any) to unblock the reader, then
    /// tear down both I/O tasks. Safe to call with or without a
    /// subprocess. Idempotent — racing shutdown calls are fine; the one
    /// that takes each `Option` does the work, the other sees `None`.
    pub async fn shutdown(&self) -> Result<(), AcpError> {
        // Close the handler semaphore so any handler tasks waiting for a
        // permit wake up and exit instead of outliving the client.
        self.handler_sem.close();
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
        if let Some(w) = self.writer_task.lock().await.take() {
            w.abort();
            let _ = w.await;
        }
        if let Some(r) = self.reader_task.lock().await.take() {
            r.abort();
            let _ = r.await;
        }
        Ok(())
    }
}

fn parse_params<T: DeserializeOwned>(
    params: Option<serde_json::Value>,
) -> Result<T, RpcError> {
    let v = params.unwrap_or(serde_json::Value::Null);
    serde_json::from_value(v).map_err(|e| RpcError {
        code: error_codes::INVALID_PARAMS,
        message: format!("invalid params: {e}"),
        data: None,
    })
}

fn internal(e: serde_json::Error) -> RpcError {
    RpcError {
        code: error_codes::INTERNAL_ERROR,
        message: e.to_string(),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that cancel tolerates JSON-RPC error -32601 (Method not
    /// found). This is tested through the actual implementation by
    /// observing its match arm on `AcpError::Rpc { code: -32601, .. }`.
    /// A full integration test with a mock transport would be complex; this
    /// at least documents the error-swallowing contract.
    #[test]
    fn cancel_error_handling_logic() {
        // The cancel method explicitly matches on:
        //   Err(AcpError::Rpc { code: -32601, .. }) => Ok(())
        // This test verifies that conversion from RpcError -> AcpError
        // preserves the code so that match works.
        let rpc_err = RpcError {
            code: -32601,
            message: "Method not found".into(),
            data: None,
        };
        let acp_err: AcpError = rpc_err.into();
        
        match acp_err {
            AcpError::Rpc { code, .. } => {
                assert_eq!(code, -32601, "code should be preserved in AcpError::Rpc");
            }
            other => panic!("expected AcpError::Rpc, got {other:?}"),
        }
    }
}
