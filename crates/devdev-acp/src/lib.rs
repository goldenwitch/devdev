//! ACP protocol client and hook handlers for DevDev.
//!
//! Implements JSON-RPC 2.0 over NDJSON for communication with
//! the Copilot CLI subprocess, including terminal and filesystem hooks.

pub mod auth;
pub mod client;
pub mod handler;
pub mod hooks;
pub mod ndjson;
pub mod protocol;
pub mod terminal;
pub mod trace;
pub mod transport;
pub mod types;

pub use auth::{AuthStrategy, find_env_token};
pub use client::{
    AcpClient, AcpClientConfig, AcpError, DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_INFLIGHT_HANDLERS,
    DEFAULT_MAX_PENDING, DEFAULT_REQUEST_TIMEOUT,
};
pub use handler::{AcpHandler, HandlerResult};
pub use hooks::{
    DEFAULT_COMMAND_TIMEOUT, DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_SHELL_CHANNEL_DEPTH,
    HandlerConfig, SandboxHandler,
};
pub use terminal::ShellWorker;
pub use ndjson::{NdjsonReader, NdjsonWriter};
pub use protocol::{Message, Notification, Request, RequestId, Response, RpcError};
pub use trace::{CollectingTraceLogger, NoopTraceLogger, TraceEvent, TraceLogger, TracingTraceLogger};
pub use transport::{AsyncNdjsonReader, AsyncNdjsonWriter};
pub use types::*;