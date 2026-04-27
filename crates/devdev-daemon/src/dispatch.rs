//! IPC method dispatcher — routes incoming requests to subsystems.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{Mutex, watch};

use devdev_integrations::GitHubAdapter;
use devdev_tasks::approval::{
    ApprovalGate, ApprovalHandle, ApprovalPolicy, ApprovalResponse, approval_channel,
};
use devdev_tasks::events::EventBus;
use devdev_tasks::ledger::IdempotencyLedger;
use devdev_tasks::monitor_pr::MonitorPrTask;
use devdev_tasks::registry::TaskRegistry;
use devdev_tasks::repo_watch::RepoWatchTask;
use devdev_workspace::Fs;

use crate::ipc::{IpcRequest, IpcResponse};
use crate::router::{SessionHandle, SessionRouter};
use crate::runner::RouterRunner;
use crate::secrets::AgentSecrets;

/// Shared state for the dispatch layer.
pub struct DispatchContext {
    pub router: Arc<SessionRouter>,
    pub tasks: Arc<Mutex<TaskRegistry>>,
    pub github: Arc<dyn GitHubAdapter>,
    /// Sender side of the approval channel, used by `devdev_ask` to
    /// request user approval before the agent takes external action.
    pub approval_gate: Arc<Mutex<ApprovalGate>>,
    /// Receiver side, surfaced through the `approval_response` IPC so
    /// a TUI / CLI can pump approvals.
    pub approval_handle: Arc<Mutex<ApprovalHandle>>,
    pub event_bus: EventBus,
    pub ledger: Arc<dyn IdempotencyLedger>,
    pub approval_policy: ApprovalPolicy,
    pub approval_timeout: Duration,
    /// Host-derived secrets (e.g. `gh auth token`) handed out only on
    /// approved `devdev_ask` calls.
    pub agent_secrets: Arc<Mutex<AgentSecrets>>,
    pub shutdown_tx: watch::Sender<bool>,
    /// Workspace filesystem, shared with the MCP provider so `fs/read`
    /// IPC calls observe the same bytes the agent wrote via MCP tools.
    pub fs: Arc<Mutex<Fs>>,
    interactive: Mutex<Option<SessionHandle>>,
    /// Log entries per task (task_id → messages).
    task_logs: Mutex<HashMap<String, Vec<String>>>,
    /// Active `RepoWatchTask`s keyed by `(owner, repo)`.
    repo_watch_ids: Mutex<HashMap<(String, String), String>>,
    /// Active `MonitorPrTask`s keyed by `(owner, repo, number)`.
    monitor_pr_ids: Mutex<HashMap<(String, String, u64), String>>,
}

impl DispatchContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        router: Arc<SessionRouter>,
        tasks: Arc<Mutex<TaskRegistry>>,
        github: Arc<dyn GitHubAdapter>,
        approval_gate: Arc<Mutex<ApprovalGate>>,
        approval_handle: Arc<Mutex<ApprovalHandle>>,
        event_bus: EventBus,
        ledger: Arc<dyn IdempotencyLedger>,
        approval_policy: ApprovalPolicy,
        agent_secrets: Arc<Mutex<AgentSecrets>>,
        shutdown_tx: watch::Sender<bool>,
        fs: Arc<Mutex<Fs>>,
    ) -> Self {
        Self {
            router,
            tasks,
            github,
            approval_gate,
            approval_handle,
            event_bus,
            ledger,
            approval_policy,
            approval_timeout: Duration::from_secs(300),
            agent_secrets,
            shutdown_tx,
            fs,
            interactive: Mutex::new(None),
            task_logs: Mutex::new(HashMap::new()),
            repo_watch_ids: Mutex::new(HashMap::new()),
            monitor_pr_ids: Mutex::new(HashMap::new()),
        }
    }

    /// Set a custom approval timeout (useful for tests). Rebuilds the
    /// underlying channel so the new timeout is honored.
    pub fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.approval_timeout = timeout;
        let (gate, handle) = approval_channel(self.approval_policy, timeout);
        self.approval_gate = Arc::new(Mutex::new(gate));
        self.approval_handle = Arc::new(Mutex::new(handle));
        self
    }

    /// Dispatch an IPC request to the appropriate handler.
    pub async fn dispatch(&self, req: IpcRequest) -> IpcResponse {
        match req.method.as_str() {
            "send" => self.handle_send(req).await,
            "task/add" => self.handle_task_add(req).await,
            "task/log" => self.handle_task_log(req).await,
            "repo/watch" => self.handle_repo_watch(req).await,
            "repo/unwatch" => self.handle_repo_unwatch(req).await,
            "status" => self.handle_status(req).await,
            "shutdown" => self.handle_shutdown(req).await,
            "approval_response" => self.handle_approval(req).await,
            "fs/read" => self.handle_fs_read(req).await,
            _ => IpcResponse::err(req.id, -32601, format!("unknown method: {}", req.method)),
        }
    }

    /// "fs/read" — return the UTF-8 contents of an absolute VFS path.
    ///
    /// The test-and-introspection counterpart of the `devdev_fs_write`
    /// MCP tool. Lets users (and claim tests) observe daemon-owned Fs
    /// state through the same IPC surface they use for everything else.
    async fn handle_fs_read(&self, req: IpcRequest) -> IpcResponse {
        let path = match req.params["path"].as_str() {
            Some(p) => p.to_string(),
            None => return IpcResponse::err(req.id, -32602, "missing params.path"),
        };
        if !path.starts_with('/') {
            return IpcResponse::err(
                req.id,
                -32602,
                format!("path must be absolute (start with '/'): {path}"),
            );
        }
        let bytes = {
            let fs = self.fs.lock().await;
            match fs.read_path(path.as_bytes()) {
                Ok(b) => b,
                Err(e) => return IpcResponse::err(req.id, -1, format!("read_path {path}: {e:?}")),
            }
        };
        match String::from_utf8(bytes) {
            Ok(s) => IpcResponse::ok(req.id, json!({ "content": s })),
            Err(e) => IpcResponse::err(req.id, -1, format!("non-UTF-8 bytes at {path}: {e}")),
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
            Ok(resp) => IpcResponse::ok(
                req.id,
                json!({
                    "response": resp.text,
                    "stop_reason": resp.stop_reason,
                }),
            ),
            Err(e) => IpcResponse::err(req.id, -1, e.to_string()),
        }
    }

    /// "task/add" — create a MonitorPR task from a description.
    async fn handle_task_add(&self, req: IpcRequest) -> IpcResponse {
        let desc = match req.params["description"].as_str() {
            Some(d) => d.to_string(),
            None => return IpcResponse::err(req.id, -32602, "missing params.description"),
        };
        let auto_approve = req
            .params
            .get("auto_approve")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Extract PR ref from description (look for "owner/repo#N" or URL pattern).
        let pr_ref_str = extract_pr_ref(&desc);
        let pr_ref_str = match pr_ref_str {
            Some(s) => s,
            None => {
                return IpcResponse::err(
                    req.id,
                    -32602,
                    "could not find PR reference in description",
                );
            }
        };

        let policy = if auto_approve {
            ApprovalPolicy::AutoApprove
        } else {
            self.approval_policy
        };

        // If the per-task `auto_approve` overrides the daemon's
        // policy, swap the active approval channel so `devdev_ask`
        // bypasses prompts for this task.
        if policy != self.approval_policy {
            let (gate, handle) = approval_channel(policy, self.approval_timeout);
            *self.approval_gate.lock().await = gate;
            *self.approval_handle.lock().await = handle;
        }

        let mut registry = self.tasks.lock().await;
        let task_id = registry.next_id();
        let runner = Arc::new(RouterRunner::new(Arc::clone(&self.router), task_id.clone()))
            as Arc<dyn devdev_tasks::agent::AgentRunner>;

        match MonitorPrTask::new(
            task_id.clone(),
            &pr_ref_str,
            Arc::clone(&self.github),
            runner,
            &self.event_bus,
        ) {
            Ok(task) => {
                let pr = task.pr_ref().clone();
                registry.add(Box::new(task));
                drop(registry);
                self.monitor_pr_ids.lock().await.insert(
                    (pr.owner.clone(), pr.repo.clone(), pr.number),
                    task_id.clone(),
                );
                IpcResponse::ok(
                    req.id,
                    json!({
                        "task_id": task_id,
                    }),
                )
            }
            Err(e) => IpcResponse::err(req.id, -1, e.to_string()),
        }
    }

    /// "repo/watch" — start a `RepoWatchTask` for `(owner, repo)`.
    ///
    /// Idempotent: subsequent calls for the same repo return the
    /// existing task id without spawning a duplicate watcher.
    async fn handle_repo_watch(&self, req: IpcRequest) -> IpcResponse {
        let owner = match req.params["owner"].as_str() {
            Some(s) => s.to_string(),
            None => return IpcResponse::err(req.id, -32602, "missing params.owner"),
        };
        let repo = match req.params["repo"].as_str() {
            Some(s) => s.to_string(),
            None => return IpcResponse::err(req.id, -32602, "missing params.repo"),
        };
        let interval_secs = req
            .params
            .get("poll_interval_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        let key = (owner.clone(), repo.clone());
        {
            let watches = self.repo_watch_ids.lock().await;
            if let Some(id) = watches.get(&key) {
                return IpcResponse::ok(req.id, json!({ "task_id": id, "already_watching": true }));
            }
        }

        let mut registry = self.tasks.lock().await;
        let task_id = registry.next_id();
        let task = RepoWatchTask::new(
            task_id.clone(),
            owner.clone(),
            repo.clone(),
            Arc::clone(&self.github),
            Arc::clone(&self.ledger),
            self.event_bus.clone(),
        )
        .with_interval(Duration::from_secs(interval_secs));
        registry.add(Box::new(task));
        drop(registry);

        self.repo_watch_ids
            .lock()
            .await
            .insert(key, task_id.clone());

        IpcResponse::ok(
            req.id,
            json!({ "task_id": task_id, "already_watching": false }),
        )
    }

    /// "repo/unwatch" — cancel an active `RepoWatchTask`.
    async fn handle_repo_unwatch(&self, req: IpcRequest) -> IpcResponse {
        let owner = match req.params["owner"].as_str() {
            Some(s) => s.to_string(),
            None => return IpcResponse::err(req.id, -32602, "missing params.owner"),
        };
        let repo = match req.params["repo"].as_str() {
            Some(s) => s.to_string(),
            None => return IpcResponse::err(req.id, -32602, "missing params.repo"),
        };

        let task_id = {
            let mut watches = self.repo_watch_ids.lock().await;
            match watches.remove(&(owner.clone(), repo.clone())) {
                Some(id) => id,
                None => {
                    return IpcResponse::err(
                        req.id,
                        -32602,
                        format!("not watching {owner}/{repo}"),
                    );
                }
            }
        };

        let mut registry = self.tasks.lock().await;
        match registry.cancel(&task_id) {
            Ok(()) => IpcResponse::ok(req.id, json!({ "task_id": task_id })),
            Err(e) => IpcResponse::err(req.id, -1, e.to_string()),
        }
    }

    /// Ensure a `MonitorPrTask` exists for `(owner, repo, number)`.
    /// Used by the event coordinator on first observation of a PR.
    /// Returns `(task_id, newly_created)`. When `newly_created` is
    /// true the caller should replay the triggering event onto the
    /// bus so the freshly-subscribed task observes it.
    pub async fn ensure_monitor_pr_task(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<(String, bool), String> {
        let key = (owner.to_string(), repo.to_string(), number);
        {
            let map = self.monitor_pr_ids.lock().await;
            if let Some(id) = map.get(&key) {
                return Ok((id.clone(), false));
            }
        }

        let pr_ref_str = format!("{owner}/{repo}#{number}");
        let mut registry = self.tasks.lock().await;
        let task_id = registry.next_id();
        let runner = Arc::new(RouterRunner::new(Arc::clone(&self.router), task_id.clone()))
            as Arc<dyn devdev_tasks::agent::AgentRunner>;

        let task = MonitorPrTask::new(
            task_id.clone(),
            &pr_ref_str,
            Arc::clone(&self.github),
            runner,
            &self.event_bus,
        )
        .map_err(|e| e.to_string())?;
        registry.add(Box::new(task));
        drop(registry);

        self.monitor_pr_ids
            .lock()
            .await
            .insert(key, task_id.clone());
        Ok((task_id, true))
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

        IpcResponse::ok(
            req.id,
            json!({
                "tasks": tasks.len(),
                "sessions": sessions.len(),
            }),
        )
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
                        self.log_task_message(&task_id, &format!("error: {e}"))
                            .await;
                    }
                }
            }
        }
    }
}

/// Spawn a background coordinator that subscribes to the daemon
/// [`EventBus`] and ensures a [`MonitorPrTask`] exists for every PR
/// it observes. Runs until the watch flag flips.
pub fn spawn_event_coordinator(
    ctx: Arc<DispatchContext>,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let mut rx = ctx.event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                ev = rx.recv() => {
                    let ev = match ev {
                        Ok(e) => e,
                        Err(_) => break,
                    };
                    if let Some((owner, repo, number)) = ev.pr_target() {
                        let owner = owner.to_string();
                        let repo = repo.to_string();
                        match ctx
                            .ensure_monitor_pr_task(&owner, &repo, number)
                            .await
                        {
                            Ok((_, true)) => {
                                // Newly-created task subscribed *after* this
                                // event was published; replay so it sees it.
                                ctx.event_bus.publish(ev.clone());
                            }
                            Ok((_, false)) => {}
                            Err(e) => {
                                tracing::warn!(
                                    "event coordinator: ensure_monitor_pr_task failed for {owner}/{repo}#{number}: {e}"
                                );
                            }
                        }
                    }
                }
            }
        }
    })
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
            if before.contains('/')
                && !before.starts_with('/')
                && after.chars().all(|c| c.is_ascii_digit())
                && !after.is_empty()
            {
                return Some(word.to_string());
            }
        }
    }
    None
}
