//! Concrete [`McpToolProvider`] backed by the daemon's live state.
//!
//! This is the production bridge between DevDev's long-lived daemon
//! structures and the MCP tools exposed over loopback HTTP. Separate
//! from `tools.rs` so tests in that module can continue to exercise
//! the server skeleton with just a `StaticProvider`.

use std::sync::Arc;

use async_trait::async_trait;
use devdev_tasks::registry::TaskRegistry;
use devdev_workspace::Fs;
use tokio::sync::Mutex;

use crate::mcp::{McpProviderError, McpToolProvider, TaskInfo};

/// Wraps the daemon's shared `Arc<Mutex<TaskRegistry>>` and
/// `Arc<Mutex<Fs>>` so the MCP server can both surface task state and
/// mutate the workspace filesystem on the agent's behalf.
///
/// Additional providers (ledger, prefs) will be folded into this
/// struct as capabilities 27 and workspace prefs land — keeping a
/// single concrete type simplifies the boot wiring in `run_up`.
#[derive(Clone)]
pub struct DaemonToolProvider {
    tasks: Arc<Mutex<TaskRegistry>>,
    fs: Arc<Mutex<Fs>>,
}

impl DaemonToolProvider {
    pub fn new(tasks: Arc<Mutex<TaskRegistry>>, fs: Arc<Mutex<Fs>>) -> Self {
        Self { tasks, fs }
    }
}

#[async_trait]
impl McpToolProvider for DaemonToolProvider {
    async fn tasks_list(&self) -> Result<Vec<TaskInfo>, McpProviderError> {
        let registry = self.tasks.lock().await;
        let out = registry
            .list()
            .into_iter()
            .map(|t| TaskInfo {
                id: t.id().to_string(),
                kind: t.task_type().to_string(),
                name: t.describe(),
                status: t.status().to_string(),
            })
            .collect();
        Ok(out)
    }

    async fn fs_write(
        &self,
        path: String,
        content: String,
    ) -> Result<(), McpProviderError> {
        if !path.starts_with('/') {
            return Err(McpProviderError::Other(format!(
                "path must be absolute (start with '/'): {path}"
            )));
        }
        let mut fs = self.fs.lock().await;
        // Create parent dirs so the agent doesn't have to mkdir first.
        if let Some(parent_end) = path.rfind('/') {
            let parent = &path[..parent_end];
            if !parent.is_empty() {
                fs.mkdir_p(parent.as_bytes(), 0o755)
                    .map_err(|e| McpProviderError::Other(format!("mkdir_p {parent}: {e:?}")))?;
            }
        }
        fs.write_path(path.as_bytes(), content.as_bytes())
            .map_err(|e| McpProviderError::Other(format!("write_path {path}: {e:?}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devdev_tasks::task::{Task, TaskError, TaskMessage, TaskStatus};
    use std::time::Duration;

    /// Minimal `Task` for testing — no real poll behaviour, just
    /// exposes the four accessors the provider reads.
    struct FakeTask {
        id: String,
        kind: &'static str,
        desc: String,
        status: TaskStatus,
    }

    #[async_trait]
    impl Task for FakeTask {
        fn id(&self) -> &str {
            &self.id
        }
        fn describe(&self) -> String {
            self.desc.clone()
        }
        fn status(&self) -> &TaskStatus {
            &self.status
        }
        fn set_status(&mut self, status: TaskStatus) {
            self.status = status;
        }
        async fn poll(&mut self) -> Result<Vec<TaskMessage>, TaskError> {
            Ok(vec![])
        }
        fn serialize(&self) -> Result<serde_json::Value, TaskError> {
            Ok(serde_json::json!({}))
        }
        fn task_type(&self) -> &str {
            self.kind
        }
        fn poll_interval(&self) -> Duration {
            Duration::from_secs(60)
        }
    }

    #[tokio::test]
    async fn tasks_list_reflects_registry_snapshot() {
        let mut reg = TaskRegistry::new();
        reg.add(Box::new(FakeTask {
            id: "t-1".into(),
            kind: "monitor-pr",
            desc: "monitor owner/repo#42".into(),
            status: TaskStatus::Polling,
        }));
        reg.add(Box::new(FakeTask {
            id: "t-2".into(),
            kind: "vibe-check",
            desc: "vibe check".into(),
            status: TaskStatus::Idle,
        }));

        let provider = DaemonToolProvider::new(
            Arc::new(Mutex::new(reg)),
            Arc::new(Mutex::new(devdev_workspace::Fs::new())),
        );
        let mut tasks = provider.tasks_list().await.expect("list");
        tasks.sort_by(|a, b| a.id.cmp(&b.id));

        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "t-1");
        assert_eq!(tasks[0].kind, "monitor-pr");
        assert_eq!(tasks[0].status, "polling");
        assert_eq!(tasks[1].id, "t-2");
        assert_eq!(tasks[1].kind, "vibe-check");
        assert_eq!(tasks[1].status, "idle");
    }

    #[tokio::test]
    async fn tasks_list_empty_registry_returns_empty_vec() {
        let provider = DaemonToolProvider::new(
            Arc::new(Mutex::new(TaskRegistry::new())),
            Arc::new(Mutex::new(devdev_workspace::Fs::new())),
        );
        let tasks = provider.tasks_list().await.expect("list");
        assert!(tasks.is_empty());
    }
}
