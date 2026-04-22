//! Task trait and core types.

use std::time::Duration;

/// Error type for task operations.
#[derive(thiserror::Error, Debug)]
pub enum TaskError {
    #[error("task not found: {0}")]
    NotFound(String),

    #[error("task already cancelled: {0}")]
    AlreadyCancelled(String),

    #[error("poll failed: {0}")]
    PollFailed(String),

    #[error("serialization error: {0}")]
    Serialization(String),
}

/// A message produced by a task poll.
#[derive(Debug, Clone)]
pub enum TaskMessage {
    /// Textual output for the user.
    Text(String),
    /// Task status changed.
    StatusChange {
        task_id: String,
        old: TaskStatus,
        new: TaskStatus,
    },
}

/// Current status of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Created,
    Polling,
    Idle,
    Completed,
    Cancelled,
    Errored(String),
}

impl TaskStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Errored(_))
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Polling => write!(f, "polling"),
            Self::Idle => write!(f, "idle"),
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Errored(e) => write!(f, "errored: {e}"),
        }
    }
}

/// A long-lived unit of background work.
#[async_trait::async_trait]
pub trait Task: Send + Sync {
    /// Unique identifier for this task instance.
    fn id(&self) -> &str;

    /// Human-readable description.
    fn describe(&self) -> String;

    /// Current status.
    fn status(&self) -> &TaskStatus;

    /// Set status (used by scheduler).
    fn set_status(&mut self, status: TaskStatus);

    /// Called on schedule. Inspect state, produce messages.
    async fn poll(&mut self) -> Result<Vec<TaskMessage>, TaskError>;

    /// Serialize task state for checkpoint.
    fn serialize(&self) -> Result<serde_json::Value, TaskError>;

    /// Task type name (for deserialization dispatch).
    fn task_type(&self) -> &str;

    /// Requested polling interval.
    fn poll_interval(&self) -> Duration;
}
