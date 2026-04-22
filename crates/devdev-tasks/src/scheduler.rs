//! Task scheduler: drives polling loops for registered tasks.

use std::sync::Arc;

use tokio::sync::{watch, Mutex};
use tracing;

use crate::registry::TaskRegistry;
use crate::task::{TaskMessage, TaskStatus};

/// Drives polling loops for all tasks in the registry.
pub struct TaskScheduler {
    registry: Arc<Mutex<TaskRegistry>>,
}

impl TaskScheduler {
    pub fn new(registry: Arc<Mutex<TaskRegistry>>) -> Self {
        Self { registry }
    }

    /// Run the scheduling loop. Polls each task at its requested interval.
    /// Returns when shutdown is signaled.
    pub async fn run(
        &self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Vec<TaskMessage> {
        let mut all_messages = Vec::new();
        let mut handles: Vec<tokio::task::JoinHandle<Vec<TaskMessage>>> = Vec::new();

        // Snapshot task IDs and intervals.
        let task_info: Vec<(String, std::time::Duration)> = {
            let reg = self.registry.lock().await;
            reg.list()
                .iter()
                .filter(|t| !t.status().is_terminal())
                .map(|t| (t.id().to_string(), t.poll_interval()))
                .collect()
        };

        for (task_id, interval) in task_info {
            let registry = Arc::clone(&self.registry);
            let mut shutdown_rx = shutdown.clone();

            let handle = tokio::spawn(async move {
                let mut messages = Vec::new();
                let mut ticker = tokio::time::interval(interval);
                // Skip the first immediate tick so we don't poll at t=0
                // Actually we DO want to poll immediately on first tick.

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            let mut reg = registry.lock().await;
                            if let Some(task) = reg.get_mut(&task_id) {
                                if task.status().is_terminal() {
                                    break;
                                }
                                task.set_status(TaskStatus::Polling);
                                match task.poll().await {
                                    Ok(msgs) => {
                                        if task.status() == &TaskStatus::Polling {
                                            task.set_status(TaskStatus::Idle);
                                        }
                                        messages.extend(msgs);
                                    }
                                    Err(e) => {
                                        tracing::error!(task = %task_id, "poll failed: {e}");
                                        task.set_status(TaskStatus::Errored(e.to_string()));
                                        break;
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            break;
                        }
                    }
                }

                messages
            });

            handles.push(handle);
        }

        // Wait for all tasks to finish or shutdown.
        tokio::select! {
            _ = async {
                for handle in &mut handles {
                    if let Ok(msgs) = handle.await {
                        all_messages.extend(msgs);
                    }
                }
            } => {}
            _ = shutdown.changed() => {
                // Abort all task handles on shutdown.
                for handle in &handles {
                    handle.abort();
                }
            }
        }

        all_messages
    }
}
