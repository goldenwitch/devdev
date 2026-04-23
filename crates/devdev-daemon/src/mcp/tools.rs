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

/// Data source for DevDev MCP tools.
///
/// Concrete daemon wiring lives in a separate capability
/// (`daemon-tool-provider`); the trait lets the skeleton here be tested
/// with a `StaticProvider` and later extended with ledger/prefs methods
/// without churning the MCP surface.
#[async_trait]
pub trait McpToolProvider: Send + Sync {
    async fn tasks_list(&self) -> Result<Vec<TaskInfo>, McpProviderError>;
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
