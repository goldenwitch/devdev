//! Local MCP (Model Context Protocol) server for DevDev.
//!
//! Exposes daemon-internal state (tasks, later the idempotency ledger and
//! preference inventory) as MCP tools so the Copilot agent can query them
//! natively. Copilot's `--allow-all-tools` prod path bypasses our ACP
//! hooks, so MCP is the injection surface — see `docs/internals/capabilities/28-mcp-tool-injection.md`.
//!
//! Transport: Streamable HTTP, stateless mode, loopback only, bearer-auth'd.
//! Shape is driven by the 2026-04-22 Node PoC (`target/tmp/poc-mcp/`) and
//! the Rust PoC (`target/tmp/poc-mcp-rs/`).

mod http;
mod provider;
mod tools;

pub use http::{McpEndpoint, McpServer, McpServerError};
pub use provider::DaemonToolProvider;
pub use tools::{
    AskKind, AskRequest, AskResponse, McpProviderError, McpToolProvider, StaticProvider, TaskInfo,
};
