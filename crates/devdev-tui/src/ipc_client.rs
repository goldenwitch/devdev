//! IPC client: DaemonConnection and DaemonEvent types.
//!
//! Shared by both TUI and headless modes.

use std::path::Path;

use devdev_daemon::ipc::{self, IpcClient, IpcResponse};

/// Error connecting to or communicating with the daemon.
#[derive(thiserror::Error, Debug)]
pub enum ConnectError {
    #[error("daemon not running (no port file)")]
    NotRunning,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(String),
}

/// Events received from the daemon.
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    /// Agent text chunk (stream incrementally).
    AgentText { text: String, done: bool },
    /// Agent finished a complete response.
    AgentDone { full_text: String },
    /// Task wants to take an external action and needs approval.
    ApprovalRequest {
        action: String,
        details: serde_json::Value,
    },
    /// Status update (task created, task finished, repo loaded, etc.).
    StatusUpdate { message: String },
    /// Error from daemon.
    Error { message: String },
}

impl DaemonEvent {
    /// Parse a daemon event from a JSON value.
    pub fn from_json(val: &serde_json::Value) -> Result<Self, ConnectError> {
        let event_type = val["type"]
            .as_str()
            .unwrap_or("unknown");

        match event_type {
            "agent_text" => Ok(DaemonEvent::AgentText {
                text: val["text"].as_str().unwrap_or("").to_string(),
                done: val["done"].as_bool().unwrap_or(false),
            }),
            "agent_done" => Ok(DaemonEvent::AgentDone {
                full_text: val["full_text"].as_str().unwrap_or("").to_string(),
            }),
            "approval_request" => Ok(DaemonEvent::ApprovalRequest {
                action: val["action"].as_str().unwrap_or("").to_string(),
                details: val["details"].clone(),
            }),
            "status" => Ok(DaemonEvent::StatusUpdate {
                message: val["message"].as_str().unwrap_or("").to_string(),
            }),
            "error" => Ok(DaemonEvent::Error {
                message: val["message"].as_str().unwrap_or("").to_string(),
            }),
            _ => Err(ConnectError::Protocol(format!(
                "unknown event type: {event_type}"
            ))),
        }
    }

    /// Serialize a daemon event to JSON.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            DaemonEvent::AgentText { text, done } => serde_json::json!({
                "type": "agent_text",
                "text": text,
                "done": done,
            }),
            DaemonEvent::AgentDone { full_text } => serde_json::json!({
                "type": "agent_done",
                "full_text": full_text,
            }),
            DaemonEvent::ApprovalRequest { action, details } => serde_json::json!({
                "type": "approval_request",
                "action": action,
                "details": details,
            }),
            DaemonEvent::StatusUpdate { message } => serde_json::json!({
                "type": "status",
                "message": message,
            }),
            DaemonEvent::Error { message } => serde_json::json!({
                "type": "error",
                "message": message,
            }),
        }
    }
}

/// Client connection to the daemon (used by TUI and headless).
pub struct DaemonConnection {
    client: IpcClient,
}

impl DaemonConnection {
    /// Connect to the running daemon by reading the port file.
    pub async fn connect(data_dir: &Path) -> Result<Self, ConnectError> {
        let port = ipc::read_port(data_dir)?
            .ok_or(ConnectError::NotRunning)?;

        let client = IpcClient::connect(port).await?;
        Ok(Self { client })
    }

    /// Connect directly to a known port (for testing).
    pub async fn connect_to_port(port: u16) -> Result<Self, ConnectError> {
        let client = IpcClient::connect(port).await?;
        Ok(Self { client })
    }

    /// Send a user message.
    pub async fn send_message(&mut self, text: &str) -> Result<IpcResponse, ConnectError> {
        self.client
            .request("send", serde_json::json!({"text": text}))
            .await
            .map_err(ConnectError::Io)
    }

    /// Send an approval response.
    pub async fn send_approval(&mut self, approve: bool) -> Result<IpcResponse, ConnectError> {
        self.client
            .request(
                "approval_response",
                serde_json::json!({"approve": approve}),
            )
            .await
            .map_err(ConnectError::Io)
    }

    /// Request daemon status.
    pub async fn status(&mut self) -> Result<IpcResponse, ConnectError> {
        self.client
            .request("status", serde_json::json!({}))
            .await
            .map_err(ConnectError::Io)
    }

    /// Request daemon shutdown.
    pub async fn shutdown(&mut self) -> Result<IpcResponse, ConnectError> {
        self.client
            .request("shutdown", serde_json::json!({}))
            .await
            .map_err(ConnectError::Io)
    }
}
