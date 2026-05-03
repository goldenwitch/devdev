//! `MonitorPrTask` — event-driven per-PR shepherd.
//!
//! Subscribes to the daemon [`EventBus`] on construction and filters
//! to its own `(owner, repo, number)` triple. On `PrOpened` /
//! `PrUpdated` it re-prompts the agent via [`AgentRunner`] with the
//! current diff plus any caller-supplied preference-file paths. On
//! `PrClosed` the task completes.
//!
//! There is no longer any `post_review`/`ApprovalGate` plumbing here.
//! Posting is the agent's job — it runs `gh` itself, gated by the
//! `devdev_ask` MCP tool which carries approval and a short-lived
//! token.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use devdev_integrations::RepoHostAdapter;
use tokio::sync::broadcast::{Receiver, error::TryRecvError};

use crate::agent::AgentRunner;
use crate::events::{DaemonEvent, EventBus};
use crate::pr_ref::PrRef;
use crate::task::{Task, TaskError, TaskMessage, TaskStatus};

/// A task that watches a single GitHub PR for changes via the bus.
pub struct MonitorPrTask {
    id: String,
    pr_ref: PrRef,
    status: TaskStatus,
    last_sha: Option<String>,
    observations: Vec<String>,
    poll_interval: Duration,
    github: Arc<dyn RepoHostAdapter>,
    runner: Arc<dyn AgentRunner>,
    rx: Receiver<DaemonEvent>,
    /// Paths to `.devdev/*.md` preference files (Vibe Check, Phase D).
    /// Injected verbatim into the prompt; agent reads them on demand.
    preference_paths: Vec<PathBuf>,
}

impl MonitorPrTask {
    pub fn new(
        id: String,
        pr_ref_str: &str,
        github: Arc<dyn RepoHostAdapter>,
        runner: Arc<dyn AgentRunner>,
        bus: &EventBus,
    ) -> Result<Self, TaskError> {
        let pr_ref = PrRef::parse(pr_ref_str)?;
        Ok(Self {
            id,
            pr_ref,
            status: TaskStatus::Created,
            last_sha: None,
            observations: Vec::new(),
            poll_interval: Duration::from_secs(60),
            github,
            runner,
            rx: bus.subscribe(),
            preference_paths: Vec::new(),
        })
    }

    pub fn with_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    pub fn with_preferences(mut self, paths: Vec<PathBuf>) -> Self {
        self.preference_paths = paths;
        self
    }

    pub fn pr_ref(&self) -> &PrRef {
        &self.pr_ref
    }

    pub fn observations(&self) -> &[String] {
        &self.observations
    }

    /// Whether an event targets this task's PR.
    fn matches(&self, ev: &DaemonEvent) -> bool {
        match ev.pr_target() {
            Some((host, o, r, n)) => {
                host == &self.pr_ref.host_id
                    && o == self.pr_ref.owner
                    && r == self.pr_ref.repo
                    && n == self.pr_ref.number
            }
            None => false,
        }
    }

    fn build_prompt(&self, kind: &str, pr_title: &str, pr_body: &str, diff: &str) -> String {
        let mut prompt = format!(
            "PR {} was {} (head_sha={}).\n\
             Title: {}\n\
             Description: {}\n\n\
             Diff:\n```\n{}\n```\n\n",
            self.pr_ref,
            kind,
            self.last_sha.as_deref().unwrap_or("?"),
            pr_title,
            pr_body,
            diff,
        );

        if !self.preference_paths.is_empty() {
            prompt.push_str("Preference files (read on demand):\n");
            for p in &self.preference_paths {
                prompt.push_str(&format!("- {}\n", p.display()));
            }
            prompt.push('\n');
        }

        if !self.observations.is_empty() {
            prompt.push_str("Prior observations:\n");
            for obs in &self.observations {
                prompt.push_str(&format!("- {obs}\n"));
            }
            prompt.push('\n');
        }

        prompt.push_str(
            "Review this PR. To post a comment or review, call the \
             `devdev_ask` tool with `kind=post_review`; on approval \
             you'll receive a short-lived token to run `gh` yourself.",
        );

        prompt
    }

    async fn handle_event(&mut self, ev: DaemonEvent) -> Result<Vec<TaskMessage>, TaskError> {
        match ev {
            DaemonEvent::PrClosed { merged, .. } => {
                self.status = TaskStatus::Completed;
                Ok(vec![TaskMessage::Text(format!(
                    "PR {} closed (merged={merged}).",
                    self.pr_ref
                ))])
            }
            DaemonEvent::PrOpened { head_sha, .. } => {
                self.last_sha = Some(head_sha);
                self.do_prompt("opened").await
            }
            DaemonEvent::PrUpdated { head_sha, .. } => {
                self.last_sha = Some(head_sha);
                self.do_prompt("updated").await
            }
        }
    }

    async fn do_prompt(&mut self, kind: &str) -> Result<Vec<TaskMessage>, TaskError> {
        let o = &self.pr_ref.owner;
        let r = &self.pr_ref.repo;
        let n = self.pr_ref.number;

        let pr = self
            .github
            .get_pr(o, r, n)
            .await
            .map_err(|e| TaskError::PollFailed(format!("get_pr: {e}")))?;

        if matches!(
            pr.state,
            devdev_integrations::PrState::Closed | devdev_integrations::PrState::Merged
        ) {
            self.status = TaskStatus::Completed;
            return Ok(vec![TaskMessage::Text(format!(
                "PR {} is now {:?}.",
                self.pr_ref, pr.state
            ))]);
        }

        let diff = self
            .github
            .get_pr_diff(o, r, n)
            .await
            .map_err(|e| TaskError::PollFailed(format!("get_pr_diff: {e}")))?;

        let prompt = self.build_prompt(kind, &pr.title, pr.body.as_deref().unwrap_or(""), &diff);
        let reply = self
            .runner
            .run_prompt(prompt)
            .await
            .map_err(|e| TaskError::PollFailed(format!("agent: {e}")))?;

        let summary = if reply.len() > 200 {
            format!("{}…", &reply[..200])
        } else {
            reply.clone()
        };
        self.observations.push(summary);
        self.status = TaskStatus::Idle;

        Ok(vec![TaskMessage::Text(format!(
            "PR {} ({kind}) → agent reply:\n{reply}",
            self.pr_ref
        ))])
    }
}

#[async_trait::async_trait]
impl Task for MonitorPrTask {
    fn id(&self) -> &str {
        &self.id
    }

    fn describe(&self) -> String {
        format!("Monitoring PR {}", self.pr_ref)
    }

    fn status(&self) -> &TaskStatus {
        &self.status
    }

    fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
    }

    async fn poll(&mut self) -> Result<Vec<TaskMessage>, TaskError> {
        if self.status.is_terminal() {
            return Ok(vec![]);
        }

        let mut messages = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(ev) => {
                    if self.matches(&ev) {
                        let mut m = self.handle_event(ev).await?;
                        messages.append(&mut m);
                        if self.status.is_terminal() {
                            break;
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Closed) => break,
                Err(TryRecvError::Lagged(n)) => {
                    messages.push(TaskMessage::Text(format!(
                        "[warning] event bus lagged {n} messages — possible missed events for {}",
                        self.pr_ref
                    )));
                }
            }
        }
        Ok(messages)
    }

    fn serialize(&self) -> Result<serde_json::Value, TaskError> {
        Ok(serde_json::json!({
            "id": self.id,
            "owner": self.pr_ref.owner,
            "repo": self.pr_ref.repo,
            "number": self.pr_ref.number,
            "last_sha": self.last_sha,
            "observations": self.observations,
        }))
    }

    fn task_type(&self) -> &str {
        "monitor_pr"
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }
}
