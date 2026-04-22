//! Headless NDJSON bridge: stdin → daemon, daemon → stdout.

use serde::{Deserialize, Serialize};

use crate::ipc_client::DaemonEvent;

/// Inbound message from stdin (user → daemon).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HeadlessInput {
    #[serde(rename = "message")]
    Message { text: String },

    #[serde(rename = "approval_response")]
    ApprovalResponse { approve: bool },
}

/// Outbound message to stdout (daemon → user).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HeadlessOutput {
    #[serde(rename = "agent_text")]
    AgentText { text: String, done: bool },

    #[serde(rename = "agent_done")]
    AgentDone { full_text: String },

    #[serde(rename = "approval_request")]
    ApprovalRequest {
        action: String,
        details: serde_json::Value,
    },

    #[serde(rename = "status")]
    Status { message: String },

    #[serde(rename = "error")]
    Error { message: String },
}

impl From<DaemonEvent> for HeadlessOutput {
    fn from(event: DaemonEvent) -> Self {
        match event {
            DaemonEvent::AgentText { text, done } => HeadlessOutput::AgentText { text, done },
            DaemonEvent::AgentDone { full_text } => HeadlessOutput::AgentDone { full_text },
            DaemonEvent::ApprovalRequest { action, details } => {
                HeadlessOutput::ApprovalRequest { action, details }
            }
            DaemonEvent::StatusUpdate { message } => HeadlessOutput::Status { message },
            DaemonEvent::Error { message } => HeadlessOutput::Error { message },
        }
    }
}

/// Parse a line of NDJSON from stdin.
pub fn parse_input(line: &str) -> Result<HeadlessInput, serde_json::Error> {
    serde_json::from_str(line.trim())
}

/// Serialize an output event as NDJSON.
pub fn format_output(output: &HeadlessOutput) -> Result<String, serde_json::Error> {
    serde_json::to_string(output)
}
