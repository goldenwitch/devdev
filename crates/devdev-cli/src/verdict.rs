//! Trace-logger sinks that feed `EvalResult`.
//!
//! The evaluator installs a [`FanoutTraceLogger`] over the shell
//! handler so the one trace stream drives two independent collectors:
//!
//! - [`VerdictCollector`] — concatenates every `agent_message_chunk`
//!   text into a single string (the verdict rule, cap 13).
//! - [`ToolCallCollector`] — records one [`ToolCallLog`] per
//!   `terminal/create` completion (command + exit + duration, from
//!   the `TerminalCreated` trace event emitted by the cap 12 hook).
//!
//! Both sinks are `Send + Sync` — they live behind `Arc<dyn
//! TraceLogger>` inside `SandboxHandler`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use devdev_acp::trace::{TraceEvent, TraceLogger};
use devdev_acp::types::{SessionUpdate, SessionUpdateParams};

use crate::config::ToolCallLog;

/// Concatenates `agent_message_chunk.text` into a single verdict
/// string, in arrival order. All other `session/update` variants are
/// ignored.
#[derive(Debug, Default)]
pub struct VerdictCollector {
    buf: Mutex<String>,
}

impl VerdictCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the verdict buffer.
    pub fn take(&self) -> String {
        std::mem::take(&mut *self.buf.lock().expect("verdict mutex poisoned"))
    }
}

impl TraceLogger for VerdictCollector {
    // Default `record` is a no-op for this sink — we only care about
    // session updates.
    fn record(&self, _event: TraceEvent) {}

    fn record_session_update(&self, params: &SessionUpdateParams) {
        if let SessionUpdate::AgentMessageChunk { content } = &params.update {
            let mut buf = self.buf.lock().expect("verdict mutex poisoned");
            buf.push_str(&content.text);
        }
    }
}

/// Records a [`ToolCallLog`] entry per `TerminalCreated` trace event.
#[derive(Debug, Default)]
pub struct ToolCallCollector {
    calls: Mutex<Vec<ToolCallLog>>,
}

impl ToolCallCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drain the recorded calls, preserving order.
    pub fn take(&self) -> Vec<ToolCallLog> {
        std::mem::take(&mut *self.calls.lock().expect("tool_calls mutex poisoned"))
    }
}

impl TraceLogger for ToolCallCollector {
    fn record(&self, event: TraceEvent) {
        if let TraceEvent::TerminalCreated {
            command,
            exit_code,
            duration_ms,
            ..
        } = event
        {
            let mut v = self.calls.lock().expect("tool_calls mutex poisoned");
            v.push(ToolCallLog {
                command,
                exit_code,
                duration: Duration::from_millis(duration_ms),
            });
        }
    }
}

/// Fan a single trace stream out to multiple child sinks. Each child
/// receives every event; calls are serialised by the caller (the hook
/// layer), so no cross-sink ordering guarantees beyond that.
pub struct FanoutTraceLogger {
    children: Vec<Arc<dyn TraceLogger>>,
}

impl FanoutTraceLogger {
    pub fn new(children: Vec<Arc<dyn TraceLogger>>) -> Self {
        Self { children }
    }
}

impl std::fmt::Debug for FanoutTraceLogger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FanoutTraceLogger")
            .field("children", &self.children.len())
            .finish()
    }
}

impl TraceLogger for FanoutTraceLogger {
    fn record(&self, event: TraceEvent) {
        // Clone into each child — TraceEvent is cheap-ish (a few small
        // strings per event), and the hook layer calls this on the
        // handler task, not the shell worker.
        for child in &self.children {
            child.record(event.clone());
        }
    }

    fn record_session_update(&self, params: &SessionUpdateParams) {
        for child in &self.children {
            child.record_session_update(params);
        }
    }
}
