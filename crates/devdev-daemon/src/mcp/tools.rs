//! Tool provider abstraction + the concrete `rmcp` handler that bridges
//! provider calls into MCP tool responses.
//!
//! `McpToolProvider` is the data-source trait. The rmcp handler in this
//! module owns the MCP-side tool registration (via `#[tool]` macros) and
//! delegates into the provider for state. This split lets us test the
//! server skeleton without spinning up a real `TaskRegistry`, and lets
//! future capabilities (cap 27 ledger, prefs) add provider methods
//! without touching the rmcp handler shape.

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

// ── Provider trait ────────────────────────────────────────────────

/// Errors a [`McpToolProvider`] may surface. Deliberately opaque — the
/// MCP surface converts these into generic tool errors rather than
/// leaking internal details to the agent.
#[derive(thiserror::Error, Debug)]
pub enum McpProviderError {
    #[error("provider error: {0}")]
    Other(String),
}

/// Minimal per-task shape returned by [`McpToolProvider::tasks_list`].
/// Shallow on purpose — MCP tool output is JSON text Copilot paraphrases,
/// so more fields = noisier agent output. Expand when a real consumer asks.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskInfo {
    pub id: String,
    /// Task type (e.g. `"monitor-pr"`, `"vibe-check"`). Matches the
    /// `Task::task_type()` string used for checkpoint deserialization.
    pub kind: String,
    /// Human-readable description (`Task::describe()` output).
    pub name: String,
    pub status: String,
}

/// Kinds of asks the agent can make through [`McpToolProvider::ask`].
/// Drives both the user-facing summary and (for `RequestToken`)
/// whether we surface the host `gh` token in the response.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskKind {
    /// Agent intends to post a PR review/comment. Approval response
    /// surfaces a short-lived `gh` token the agent uses with `gh pr
    /// comment` / `gh pr review`.
    PostReview,
    /// Agent wants to leave a non-review comment.
    PostComment,
    /// Agent wants the `gh` token for an action not enumerated above.
    RequestToken,
    /// Agent wants the user to answer an open question. No token.
    Question,
}

/// One ask request from the agent.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AskRequest {
    pub kind: AskKind,
    /// Human-readable single-line summary surfaced in the approval prompt.
    pub summary: String,
    /// Free-form structured payload (e.g. `{ comment, file?, line? }`
    /// for `post_review`). Echoed back in the response.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Repo host the ask targets (e.g. `"github.com"`,
    /// `"ghe.acme.io"`, `"dev.azure.com"`). Optional for back-compat
    /// with single-host clients; when absent the provider defaults
    /// to `github.com`. The provider uses this to pick which
    /// credential entry to surface in the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Outcome the agent receives.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AskResponse {
    /// Approved. `token` is `Some` for kinds that requested a token
    /// AND the host had one to give.
    Approved {
        #[serde(skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// Wall-clock seconds since epoch hinting when the token
        /// should no longer be relied upon.
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
        /// Echo of the original payload so the agent can correlate.
        payload: serde_json::Value,
    },
    /// User rejected the request.
    Rejected { reason: String },
    /// Approval request timed out.
    Timeout,
}

/// Data source for DevDev MCP tools.
///
/// Concrete daemon wiring lives in a separate capability
/// (`daemon-tool-provider`); the trait lets the skeleton here be tested
/// with a `StaticProvider` and later extended with ledger/prefs methods
/// without churning the MCP surface.
#[async_trait]
pub trait McpToolProvider: Send + Sync {
    async fn tasks_list(&self) -> Result<Vec<TaskInfo>, McpProviderError>;

    /// Write `content` (UTF-8 bytes) to `path` in the DevDev workspace,
    /// creating the file (and any missing parent directories) if needed
    /// and truncating it if present.
    ///
    /// Providers backed by a real `Fs` mutate daemon state; the default
    /// implementation returns an error so `StaticProvider` and other
    /// read-only providers fail loudly rather than silently dropping
    /// writes.
    async fn fs_write(&self, _path: String, _content: String) -> Result<(), McpProviderError> {
        Err(McpProviderError::Other(
            "fs_write not supported by this provider".into(),
        ))
    }

    /// Submit an ask to the user. Default impl rejects so read-only
    /// providers (e.g. `StaticProvider`) fail loudly rather than
    /// auto-approving silently.
    async fn ask(&self, _req: AskRequest) -> Result<AskResponse, McpProviderError> {
        Err(McpProviderError::Other(
            "ask not supported by this provider".into(),
        ))
    }
}

/// Fixed-data provider used by tests and documentation examples.
#[derive(Clone, Debug, Default)]
pub struct StaticProvider {
    pub tasks: Vec<TaskInfo>,
}

#[async_trait]
impl McpToolProvider for StaticProvider {
    async fn tasks_list(&self) -> Result<Vec<TaskInfo>, McpProviderError> {
        Ok(self.tasks.clone())
    }
    // fs_write uses the trait default: NotSupported.
}

// ── rmcp handler ──────────────────────────────────────────────────

/// rmcp `ServerHandler` that exposes DevDev tools. One instance per
/// incoming HTTP request (rmcp's `service_factory` constructs afresh);
/// the `Arc<dyn McpToolProvider>` is cheap to clone.
#[derive(Clone)]
pub(crate) struct DevDevMcpHandler {
    provider: Arc<dyn McpToolProvider>,
    tool_router: ToolRouter<DevDevMcpHandler>,
}

#[tool_router]
impl DevDevMcpHandler {
    pub(crate) fn new(provider: Arc<dyn McpToolProvider>) -> Self {
        Self {
            provider,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List active DevDev tasks currently known to the daemon \
         (monitor-PR, vibe-check, scout-router, and any other task types). \
         Returns id, name, and status. Call this whenever the user asks what \
         tasks are running, what DevDev is doing, or for task status."
    )]
    async fn devdev_tasks_list(&self) -> Result<CallToolResult, McpError> {
        let tasks = self
            .provider
            .tasks_list()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // Serde-json serialization of a Vec<TaskInfo> can't fail; unwrap is sound.
        let text = serde_json::to_string_pretty(&tasks).unwrap();
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Write UTF-8 text to a file in the DevDev workspace \
         filesystem. `path` is an absolute VFS path (e.g. `/notes/hello.txt`); \
         missing parent directories are created automatically. An existing \
         file is truncated. Use this to create or overwrite files when the \
         user asks you to write into the DevDev workspace.")]
    async fn devdev_fs_write(
        &self,
        rmcp::handler::server::wrapper::Parameters(args): rmcp::handler::server::wrapper::Parameters<
            FsWriteArgs,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.provider
            .fs_write(args.path.clone(), args.content)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "wrote {}",
            args.path
        ))]))
    }

    #[tool(
        description = "Ask the user (via DevDev's approval gate) for permission \
         to take an external action. Use this BEFORE running `gh pr comment`, \
         posting a review, fetching a token, or any other side-effecting \
         operation. `kind` is one of: `post_review` (will return a short-lived \
         GitHub token), `post_comment` (also returns token), `request_token` \
         (returns token only), `question` (returns approval, no token). \
         `summary` is a single-line human-readable description shown in the \
         approval prompt. `payload` is your free-form structured arguments \
         (e.g. `{ \"comment\": \"...\", \"file\": \"...\", \"line\": 42 }`). \
         The response is `{status: \"approved\"|\"rejected\"|\"timeout\", ...}`. \
         On `approved`, use `token` (if present) with `GH_TOKEN=<token> gh ...`."
    )]
    async fn devdev_ask(
        &self,
        rmcp::handler::server::wrapper::Parameters(args): rmcp::handler::server::wrapper::Parameters<
            AskRequest,
        >,
    ) -> Result<CallToolResult, McpError> {
        let resp = self
            .provider
            .ask(args)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // Serialize is infallible for this enum.
        let text = serde_json::to_string_pretty(&resp).unwrap();
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

/// Arguments for the `devdev_fs_write` MCP tool.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FsWriteArgs {
    /// Absolute VFS path (must start with `/`).
    pub path: String,
    /// UTF-8 content to write. File is truncated and overwritten.
    pub content: String,
}

#[tool_handler]
impl ServerHandler for DevDevMcpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "DevDev MCP — surfaces daemon-internal state (tasks, \
                 idempotency ledger, preferences) as callable tools."
                    .to_string(),
            ),
        }
    }
}
