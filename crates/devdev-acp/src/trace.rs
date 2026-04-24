//! Observability hook for ACP handlers.
//!
//! The handler emits one event per inbound ACP call plus one per
//! `session/update` notification. A [`TraceLogger`] implementation is
//! free to push those events to `tracing`, a test collector, or `/dev/null`.

use std::sync::Mutex;

use crate::types::{SessionUpdate, SessionUpdateParams};

/// A single observable event from the sandbox handler. Kept small and
/// owned so loggers can forward, buffer, or stringify at will.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceEvent {
    /// `terminal/create` was dispatched to the shell.
    TerminalCreated {
        terminal_id: String,
        command: String,
        exit_code: i32,
        duration_ms: u64,
    },
    /// `terminal/create` rejected for policy reasons (sandbox escape).
    TerminalRejected { command: String, reason: String },
    /// `fs/read_text_file` was served from the VFS.
    FsRead { path: String, bytes: usize },
    /// `fs/write_text_file` was applied to the VFS.
    FsWrite { path: String, bytes: usize },
    /// `session/request_permission` was auto-approved.
    PermissionGranted {
        tool_call_id: String,
        option_id: String,
    },
    /// A `session/update` notification arrived.
    SessionUpdate {
        session_id: String,
        kind: &'static str,
    },
}

/// Receiver for [`TraceEvent`]s. Safe to call concurrently; implementations
/// must not panic on malformed input.
pub trait TraceLogger: Send + Sync {
    fn record(&self, event: TraceEvent);

    /// Helper: classify and forward a `session/update` notification.
    fn record_session_update(&self, params: &SessionUpdateParams) {
        let kind = match &params.update {
            SessionUpdate::AgentMessageChunk { .. } => "agent_message_chunk",
            SessionUpdate::AgentThoughtChunk { .. } => "agent_thought_chunk",
            SessionUpdate::ToolCall(_) => "tool_call",
            SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
            SessionUpdate::Plan { .. } => "plan",
        };
        self.record(TraceEvent::SessionUpdate {
            session_id: params.session_id.clone(),
            kind,
        });
    }
}

/// Discards every event. Default choice for embeddings that don't care.
#[derive(Debug, Default)]
pub struct NoopTraceLogger;

impl TraceLogger for NoopTraceLogger {
    fn record(&self, _event: TraceEvent) {}
}

/// Emits events at `info` level through the `tracing` crate.
#[derive(Debug, Default)]
pub struct TracingTraceLogger;

impl TraceLogger for TracingTraceLogger {
    fn record(&self, event: TraceEvent) {
        tracing::info!(target: "devdev_acp::hooks", ?event);
    }
}

/// Buffers every event into a `Vec`. Designed for acceptance tests.
#[derive(Debug, Default)]
pub struct CollectingTraceLogger {
    events: Mutex<Vec<TraceEvent>>,
}

impl CollectingTraceLogger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<TraceEvent> {
        self.events.lock().expect("trace logger poisoned").clone()
    }
}

impl TraceLogger for CollectingTraceLogger {
    fn record(&self, event: TraceEvent) {
        self.events
            .lock()
            .expect("trace logger poisoned")
            .push(event);
    }
}
