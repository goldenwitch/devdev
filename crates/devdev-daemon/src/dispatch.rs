//! IPC method dispatcher — routes incoming requests to subsystems.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{watch, Mutex};

use devdev_integrations::GitHubAdapter;
use devdev_tasks::approval::{ApprovalHandle, ApprovalPolicy, ApprovalResponse};
use devdev_tasks::monitor_pr::{MonitorPrTask, ReviewFn};
use devdev_tasks::registry::TaskRegistry;


use crate::ipc::{IpcRequest, IpcResponse};
use crate::router::{SessionHandle, SessionRouter};

/// Shared state for the dispatch layer.
pub struct DispatchContext {
    pub router: Arc<SessionRouter>,
    pub tasks: Arc<Mutex<TaskRegistry>>,
    pub github: Arc<dyn GitHubAdapter>,
    pub approval_handle: Arc<Mutex<ApprovalHandle>>,
    pub review_fn: ReviewFn,
    pub approval_policy: ApprovalPolicy,
    pub approval_timeout: Duration,
    pub shutdown_tx: watch::Sender<bool>,
    interactive: Mutex<Option<SessionHandle>>,
    /// Log entries per task (task_id → messages).
    task_logs: Mutex<std::collections::HashMap<String, Vec<String>>>,
}

impl DispatchContext {
    pub fn new(
        router: Arc<SessionRouter>,
        tasks: Arc<Mutex<TaskRegistry>>,
        github: Arc<dyn GitHubAdapter>,
        approval_handle: Arc<Mutex<ApprovalHandle>>,
        review_fn: ReviewFn,
        approval_policy: ApprovalPolicy,
        shutdown_tx: watch::Sender<bool>,
    ) -> Self {
        Self {
            router,
            tasks,
            github,
            approval_handle,
            review_fn,
            approval_policy,
            approval_timeout: Duration::from_secs(300),
            shutdown_tx,
            interactive: Mutex::new(None),
            task_logs: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Set a custom approval timeout (useful for tests).
    pub fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.approval_timeout = timeout;
        self
    }

    /// Dispatch an IPC request to the appropriate handler.
    pub async fn dispatch(&self, req: IpcRequest) -> IpcResponse {
        match req.method.as_str() {
            "send" => self.handle_send(req).await,
            "task/add" => self.handle_task_add(req).await,
            "task/log" => self.handle_task_log(req).await,
            "status" => self.handle_status(req).await,
            "shutdown" => self.handle_shutdown(req).await,
            "approval_response" => self.handle_approval(req).await,
            _ => IpcResponse::err(req.id, -32601, format!("unknown method: {}", req.method)),
        }
    }

    /// "send" — forward a message to the interactive session.
    async fn handle_send(&self, req: IpcRequest) -> IpcResponse {
        let text = match req.params["text"].as_str() {
            Some(t) => t,
            None => return IpcResponse::err(req.id, -32602, "missing params.text"),
        };

        // Lazily create interactive session.
        let mut interactive = self.interactive.lock().await;
        if interactive.is_none() {
            match self.router.create_interactive_session().await {
                Ok(handle) => *interactive = Some(handle),
                Err(e) => return IpcResponse::err(req.id, -1, e.to_string()),
            }
        }

        let handle = interactive.as_ref().unwrap();
        match handle.send_prompt(text).await {
            Ok(resp) => IpcResponse::ok(req.id, json!({
                "response": resp.text,
                "stop_reason": resp.stop_reason,
            })),
            Err(e) => IpcResponse::err(req.id, -1, e.to_string()),
        }
    }

    /// "task/add" — create a MonitorPR task from a description.
    async fn handle_task_add(&self, req: IpcRequest) -> IpcResponse {
        let desc = match req.params["description"].as_str() {
            Some(d) => d.to_string(),
            None => return IpcResponse::err(req.id, -32602, "missing params.description"),
        };
        let auto_approve = req.params.get("auto_approve")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Extract PR ref from description (look for "owner/repo#N" or URL pattern).
        let pr_ref_str = extract_pr_ref(&desc);
        let pr_ref_str = match pr_ref_str {
            Some(s) => s,
            None => return IpcResponse::err(req.id, -32602, "could not find PR reference in description"),
        };

        let policy = if auto_approve {
            ApprovalPolicy::AutoApprove
        } else {
            self.approval_policy
        };

        let (gate, handle) = devdev_tasks::approval::approval_channel(policy, self.approval_timeout);

        // Store the new approval handle (replace previous one if any).
        {
            let mut ah = self.approval_handle.lock().await;
            *ah = handle;
        }

        let gate = Arc::new(Mutex::new(gate));
        let mut registry = self.tasks.lock().await;
        let task_id = registry.next_id();

        match MonitorPrTask::new(
            task_id.clone(),
            &pr_ref_str,
            Arc::clone(&self.github),
            gate,
            Arc::clone(&self.review_fn),
        ) {
            Ok(task) => {
                registry.add(Box::new(task));
                IpcResponse::ok(req.id, json!({
                    "task_id": task_id,
                }))
            }
            Err(e) => IpcResponse::err(req.id, -1, e.to_string()),
        }
    }

    /// "task/log" — return logged messages for a task.
    async fn handle_task_log(&self, req: IpcRequest) -> IpcResponse {
        let task_id = match req.params["task_id"].as_str() {
            Some(id) => id,
            None => return IpcResponse::err(req.id, -32602, "missing params.task_id"),
        };

        let logs = self.task_logs.lock().await;
        let entries: Vec<Value> = logs
            .get(task_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|s| json!({"text": s}))
            .collect();

        IpcResponse::ok(req.id, json!({ "entries": entries }))
    }

    /// "status" — return daemon/task status.
    async fn handle_status(&self, req: IpcRequest) -> IpcResponse {
        let tasks = self.tasks.lock().await;
        let sessions = self.router.active_sessions().await;

        IpcResponse::ok(req.id, json!({
            "tasks": tasks.len(),
            "sessions": sessions.len(),
        }))
    }

    /// "shutdown" — signal the daemon to stop.
    async fn handle_shutdown(&self, req: IpcRequest) -> IpcResponse {
        let _ = self.shutdown_tx.send(true);
        IpcResponse::ok(req.id, json!({"ok": true}))
    }

    /// "approval_response" — forward a user approval.
    async fn handle_approval(&self, req: IpcRequest) -> IpcResponse {
        let approve = match req.params.get("approve").and_then(|v| v.as_bool()) {
            Some(a) => a,
            None => return IpcResponse::err(req.id, -32602, "missing params.approve"),
        };

        let handle = self.approval_handle.lock().await;
        // We use "a-1" as a conventional ID; the gate matches on first pending.
        let response = ApprovalResponse {
            id: "a-1".to_string(),
            approve,
        };

        match handle.response_tx.send(response).await {
            Ok(()) => IpcResponse::ok(req.id, json!({"ok": true})),
            Err(_) => IpcResponse::err(req.id, -1, "approval channel closed"),
        }
    }

    /// Record a task log entry.
    pub async fn log_task_message(&self, task_id: &str, text: &str) {
        let mut logs = self.task_logs.lock().await;
        logs.entry(task_id.to_string())
            .or_default()
            .push(text.to_string());
    }

    /// Poll all tasks once and log their output.
    pub async fn poll_all_tasks(&self) {
        let registry = self.tasks.lock().await;
        let task_ids: Vec<String> = registry.list().iter().map(|t| t.id().to_string()).collect();
        drop(registry);

        for task_id in task_ids {
            let mut registry = self.tasks.lock().await;
            if let Some(task) = registry.get_mut(&task_id) {
                match task.poll().await {
                    Ok(msgs) => {
                        drop(registry);
                        for msg in msgs {
                            match &msg {
                                devdev_tasks::TaskMessage::Text(text) => {
                                    self.log_task_message(&task_id, text).await;
                                }
                                devdev_tasks::TaskMessage::StatusChange { .. } => {}
                            }
                        }
                    }
                    Err(e) => {
                        drop(registry);
                        self.log_task_message(&task_id, &format!("error: {e}")).await;
                    }
                }
            }
        }
    }
}

/// Extract a PR reference string from free-text description.
fn extract_pr_ref(desc: &str) -> Option<String> {
    // Try "owner/repo#N" pattern.
    let re_shorthand = regex_lite_shorthand(desc);
    if let Some(s) = re_shorthand {
        return Some(s);
    }

    // Try GitHub URL pattern.
    if let Some(start) = desc.find("https://github.com/") {
        let rest = &desc[start..];
        let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }

    None
}

/// Simple substring search for owner/repo#N pattern.
fn regex_lite_shorthand(desc: &str) -> Option<String> {
    for word in desc.split_whitespace() {
        if let Some(hash_pos) = word.find('#') {
            let before = &word[..hash_pos];
            let after = &word[hash_pos + 1..];
            if before.contains('/') && !before.starts_with('/') && after.chars().all(|c| c.is_ascii_digit()) && !after.is_empty() {
                return Some(word.to_string());
            }
        }
    }
    None
}
