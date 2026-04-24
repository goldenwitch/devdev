//! HTTP transport: axum router + bearer-auth + lifecycle.
//!
//! Layout:
//! ```text
//!   POST /mcp  → StreamableHttpService (bearer-auth layer)
//!   *          → 404 fallback  (no auth; absorbs Copilot's well-known probes)
//! ```
//!
//! Loopback-only by construction (`127.0.0.1:0`). The bearer is generated
//! fresh per [`McpServer::start`] call — leaks only survive until the
//! daemon restarts.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rmcp::transport::{
    StreamableHttpService, streamable_http_server::session::local::LocalSessionManager,
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::mcp::tools::{DevDevMcpHandler, McpToolProvider};

/// Where the MCP server is listening + how to authenticate to it.
/// This is the struct that gets threaded into `AcpSessionBackend` and
/// rendered into `NewSessionParams.mcp_servers` per session.
#[derive(Clone, Debug)]
pub struct McpEndpoint {
    /// Full URL, including path, e.g. `http://127.0.0.1:58234/mcp`.
    pub url: String,
    /// Secret bearer token. Rendered as `Authorization: Bearer <bearer>`.
    pub bearer: String,
}

/// Errors surfaced while starting the server.
#[derive(thiserror::Error, Debug)]
pub enum McpServerError {
    #[error("bind failed: {0}")]
    Bind(#[from] std::io::Error),
}

/// A running MCP server owned by the daemon. Dropping does *not* stop the
/// server — call [`McpServer::shutdown`] explicitly from the daemon's
/// shutdown path so the bound port is released promptly.
pub struct McpServer {
    endpoint: McpEndpoint,
    shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

impl McpServer {
    /// Bind a fresh loopback port and start serving. Returns as soon as
    /// `accept()` is ready, so callers can advertise the URL immediately.
    pub async fn start(provider: Arc<dyn McpToolProvider>) -> Result<Self, McpServerError> {
        let bearer = generate_bearer();

        // rmcp's factory is called once per inbound session; each handler
        // gets its own router table but shares the provider arc.
        let provider_for_factory = Arc::clone(&provider);
        let service = StreamableHttpService::new(
            move || Ok(DevDevMcpHandler::new(Arc::clone(&provider_for_factory))),
            Arc::new(LocalSessionManager::default()),
            rmcp::transport::streamable_http_server::StreamableHttpServerConfig {
                sse_keep_alive: None,
                stateful_mode: false,
            },
        );

        // Bearer auth wraps only the /mcp routes — the 404 fallback must
        // be reachable anonymously so Copilot's OAuth well-known probes
        // get fast closure rather than silent timeouts (observed in the
        // Node PoC: ~6 stray GETs after a 401).
        let bearer_for_mw = bearer.clone();
        let mcp_routes = Router::new()
            .nest_service("/mcp", service)
            .layer(middleware::from_fn(move |req, next| {
                let expected = bearer_for_mw.clone();
                async move { bearer_auth(expected, req, next).await }
            }));

        let app = Router::new().merge(mcp_routes).fallback(not_found);

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        let url = format!("http://{addr}/mcp");

        let shutdown = CancellationToken::new();
        let shutdown_srv = shutdown.clone();
        let handle = tokio::spawn(async move {
            let serve_result = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown_srv.cancelled().await })
                .await;
            if let Err(e) = serve_result {
                tracing::warn!(error = %e, "mcp server exited with error");
            }
        });

        tracing::info!(%url, "mcp server started");

        Ok(Self {
            endpoint: McpEndpoint { url, bearer },
            shutdown,
            handle,
        })
    }

    /// URL + bearer for handing to the ACP backend.
    pub fn endpoint(&self) -> &McpEndpoint {
        &self.endpoint
    }

    /// Signal the server to stop and await the serve task.
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        let _ = self.handle.await;
    }
}

// ── Middleware ────────────────────────────────────────────────────

async fn bearer_auth(expected: String, req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let expected_header = format!("Bearer {expected}");
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    if provided == Some(expected_header.as_str()) {
        let resp = next.run(req).await;
        tracing::debug!(
            target: "devdev_daemon::mcp::http",
            %method, %uri, status = resp.status().as_u16(), auth = "ok",
            "mcp request"
        );
        resp
    } else {
        let reason = if provided.is_some() {
            "bad_bearer"
        } else {
            "missing_bearer"
        };
        tracing::debug!(
            target: "devdev_daemon::mcp::http",
            %method, %uri, status = 401u16, auth = reason,
            "mcp request rejected"
        );
        let mut resp = (StatusCode::UNAUTHORIZED, r#"{"error":"unauthorized"}"#).into_response();
        // Hint to clients (and keeps Copilot's fallback probes consistent
        // with what the Node PoC observed).
        resp.headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        resp
    }
}

async fn not_found(req: Request<Body>) -> impl IntoResponse {
    tracing::debug!(
        target: "devdev_daemon::mcp::http",
        method = %req.method(), uri = %req.uri(), status = 404u16,
        "mcp fallback 404"
    );
    (StatusCode::NOT_FOUND, "")
}

// ── Token generation ──────────────────────────────────────────────

fn generate_bearer() -> String {
    // 32 bytes → 64 hex chars. Random sources are fine here: we use
    // getrandom (pulled in transitively via rmcp/tokio); any
    // cryptographically-secure source would do.
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("getrandom failed (platform lacks entropy source)");
    bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::{StaticProvider, TaskInfo};
    use serde_json::{Value, json};

    fn test_provider() -> Arc<dyn McpToolProvider> {
        Arc::new(StaticProvider {
            tasks: vec![
                TaskInfo {
                    id: "t-1".into(),
                    kind: "monitor-pr".into(),
                    name: "monitor owner/repo#42".into(),
                    status: "polling".into(),
                },
                TaskInfo {
                    id: "t-2".into(),
                    kind: "vibe-check".into(),
                    name: "vibe-check".into(),
                    status: "idle".into(),
                },
            ],
        })
    }

    async fn rpc(
        client: &reqwest::Client,
        url: &str,
        bearer: &str,
        body: Value,
    ) -> reqwest::Response {
        client
            .post(url)
            .header("authorization", format!("Bearer {bearer}"))
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .expect("rpc send")
    }

    /// Parse an rmcp response — might be JSON or an SSE text/event-stream frame.
    async fn parse_rmcp(resp: reqwest::Response) -> Value {
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp.text().await.expect("body text");
        if ctype.starts_with("text/event-stream") {
            let data_line = body
                .lines()
                .find(|l| l.starts_with("data:"))
                .expect("sse data line");
            serde_json::from_str(data_line.trim_start_matches("data:").trim()).expect("sse json")
        } else {
            serde_json::from_str(&body).expect("json body")
        }
    }

    #[tokio::test]
    async fn starts_and_exposes_endpoint() {
        let server = McpServer::start(test_provider()).await.expect("start");
        assert!(server.endpoint().url.starts_with("http://127.0.0.1:"));
        assert_eq!(server.endpoint().bearer.len(), 64); // 32 bytes hex
        server.shutdown().await;
    }

    #[tokio::test]
    async fn bearer_rejects_missing() {
        let server = McpServer::start(test_provider()).await.expect("start");
        let client = reqwest::Client::new();

        let resp = client
            .post(&server.endpoint().url)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .send()
            .await
            .expect("send");

        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn bearer_rejects_wrong_token() {
        let server = McpServer::start(test_provider()).await.expect("start");
        let client = reqwest::Client::new();

        let resp = rpc(
            &client,
            &server.endpoint().url,
            "nope-not-the-token",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        )
        .await;

        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn well_known_probe_gets_fast_404_without_auth() {
        let server = McpServer::start(test_provider()).await.expect("start");
        let client = reqwest::Client::new();

        // Base URL without /mcp suffix.
        let base = server.endpoint().url.trim_end_matches("/mcp").to_string();

        for path in [
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/oauth-authorization-server",
        ] {
            let resp = client
                .get(format!("{base}{path}"))
                .send()
                .await
                .expect("send");
            assert_eq!(
                resp.status(),
                reqwest::StatusCode::NOT_FOUND,
                "path {path} should 404"
            );
        }

        server.shutdown().await;
    }

    #[tokio::test]
    async fn tools_list_and_call_round_trip() {
        let server = McpServer::start(test_provider()).await.expect("start");
        let client = reqwest::Client::new();
        let url = &server.endpoint().url;
        let bearer = &server.endpoint().bearer;

        // 1. initialize
        let init = rpc(
            &client,
            url,
            bearer,
            json!({
                "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                    "protocolVersion":"2025-06-18",
                    "capabilities":{},
                    "clientInfo":{"name":"devdev-tests","version":"0.0.0"}
                }
            }),
        )
        .await;
        assert!(init.status().is_success(), "initialize status");
        let _ = parse_rmcp(init).await;

        // 2. initialized notification (MUST be sent before other calls)
        let initd = rpc(
            &client,
            url,
            bearer,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
        assert!(
            initd.status().is_success() || initd.status() == reqwest::StatusCode::ACCEPTED,
            "initialized status"
        );

        // 3. tools/list
        let list_resp = rpc(
            &client,
            url,
            bearer,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await;
        let list = parse_rmcp(list_resp).await;
        let tools = list["result"]["tools"].as_array().expect("tools array");
        assert!(
            tools.iter().any(|t| t["name"] == "devdev_tasks_list"),
            "devdev_tasks_list should be present: {list:?}"
        );

        // 4. tools/call devdev_tasks_list
        let call_resp = rpc(
            &client,
            url,
            bearer,
            json!({
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"devdev_tasks_list","arguments":{}}
            }),
        )
        .await;
        let call = parse_rmcp(call_resp).await;
        let text = call["result"]["content"][0]["text"]
            .as_str()
            .expect("text content");
        let parsed: Vec<TaskInfo> = serde_json::from_str(text).expect("parse tasks");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "t-1");
        assert_eq!(parsed[1].id, "t-2");

        server.shutdown().await;
    }
}
