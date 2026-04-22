//! MonitorPR task: monitors a GitHub PR, reviews it, watches for changes.

use std::sync::Arc;
use std::time::Duration;

use devdev_integrations::{GitHubAdapter, Review, ReviewComment, ReviewEvent};
use tokio::sync::Mutex;

use crate::approval::{ApprovalError, ApprovalGate};
use crate::pr_ref::PrRef;
use crate::review::parse_review;
use crate::task::{Task, TaskError, TaskMessage, TaskStatus};

/// Callback for getting agent reviews. Takes a prompt, returns review text.
/// This is how MonitorPrTask interacts with the agent without depending on
/// the daemon crate's SessionHandle directly.
pub type ReviewFn =
    Arc<dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>> + Send + Sync>;

/// A task that monitors a single GitHub PR.
pub struct MonitorPrTask {
    id: String,
    pr_ref: PrRef,
    status: TaskStatus,
    last_sha: Option<String>,
    observations: Vec<String>,
    poll_interval: Duration,
    github: Arc<dyn GitHubAdapter>,
    approval: Arc<Mutex<ApprovalGate>>,
    review_fn: ReviewFn,
}

impl MonitorPrTask {
    pub fn new(
        id: String,
        pr_ref_str: &str,
        github: Arc<dyn GitHubAdapter>,
        approval: Arc<Mutex<ApprovalGate>>,
        review_fn: ReviewFn,
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
            approval,
            review_fn,
        })
    }

    pub fn with_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    pub fn pr_ref(&self) -> &PrRef {
        &self.pr_ref
    }

    /// Build the prompt for the agent.
    fn build_prompt(&self, pr_title: &str, pr_body: &str, diff: &str) -> String {
        let mut prompt = format!(
            "You are reviewing PR #{} in {}/{}.\n\
             Title: {}\n\
             Description: {}\n\n\
             Diff:\n```\n{}\n```\n\n",
            self.pr_ref.number, self.pr_ref.owner, self.pr_ref.repo,
            pr_title, pr_body, diff,
        );

        if !self.observations.is_empty() {
            prompt.push_str("Prior observations:\n");
            for obs in &self.observations {
                prompt.push_str(&format!("- {obs}\n"));
            }
            prompt.push('\n');
        }

        prompt.push_str(
            "Review this PR. For inline comments, use the format [file:line] comment.\n\
             Provide a summary of your findings."
        );

        prompt
    }

    async fn do_review(&mut self) -> Result<Vec<TaskMessage>, TaskError> {
        let o = &self.pr_ref.owner;
        let r = &self.pr_ref.repo;
        let n = self.pr_ref.number;

        // Fetch PR info.
        let pr = self.github.get_pr(o, r, n).await.map_err(|e| {
            TaskError::PollFailed(format!("failed to fetch PR: {e}"))
        })?;

        // Check if merged/closed.
        match pr.state {
            devdev_integrations::PrState::Merged | devdev_integrations::PrState::Closed => {
                self.status = TaskStatus::Completed;
                return Ok(vec![TaskMessage::Text(format!(
                    "PR {} is now {:?}.",
                    self.pr_ref, pr.state
                ))]);
            }
            _ => {}
        }

        // Update SHA.
        self.last_sha = Some(pr.head_sha.clone());

        // Fetch diff.
        let diff = self.github.get_pr_diff(o, r, n).await.map_err(|e| {
            TaskError::PollFailed(format!("failed to fetch diff: {e}"))
        })?;

        // Build prompt and get review.
        let prompt = self.build_prompt(
            &pr.title,
            pr.body.as_deref().unwrap_or(""),
            &diff,
        );

        let review_text = (self.review_fn)(prompt).await.map_err(|e| {
            TaskError::PollFailed(format!("agent review failed: {e}"))
        })?;

        // Parse review.
        let parsed = parse_review(&review_text);

        // Store observation.
        let summary = if parsed.body.len() > 200 {
            format!("{}...", &parsed.body[..200])
        } else {
            parsed.body.clone()
        };
        self.observations.push(summary);

        // Build GitHub Review.
        let review = Review {
            event: ReviewEvent::Comment,
            body: parsed.body.clone(),
            comments: parsed
                .comments
                .iter()
                .map(|c| ReviewComment {
                    path: c.path.clone(),
                    line: c.line,
                    body: c.body.clone(),
                })
                .collect(),
        };

        // Request approval to post.
        let approval_result = {
            let mut gate = self.approval.lock().await;
            gate.request_approval(
                "post_review",
                serde_json::json!({
                    "repo": format!("{o}/{r}"),
                    "pr": n,
                    "comments": review.comments.len(),
                }),
            )
            .await
        };

        match approval_result {
            Ok(()) => {
                self.github
                    .post_review(o, r, n, review)
                    .await
                    .map_err(|e| TaskError::PollFailed(format!("failed to post review: {e}")))?;

                Ok(vec![TaskMessage::Text(format!(
                    "Review posted for {}:\n{review_text}",
                    self.pr_ref
                ))])
            }
            Err(ApprovalError::DryRun { .. }) => Ok(vec![TaskMessage::Text(format!(
                "[dry-run] Would post review for {}:\n{review_text}",
                self.pr_ref
            ))]),
            Err(ApprovalError::Rejected) => Ok(vec![TaskMessage::Text(format!(
                "Review rejected by user for {}:\n{review_text}",
                self.pr_ref
            ))]),
            Err(ApprovalError::Timeout) => Ok(vec![TaskMessage::Text(format!(
                "Approval timed out for {}. Review not posted.",
                self.pr_ref
            ))]),
            Err(e) => Err(TaskError::PollFailed(format!("approval error: {e}"))),
        }
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

        match &self.status {
            TaskStatus::Created | TaskStatus::Polling => {
                // First review.
                self.do_review().await
            }
            TaskStatus::Idle => {
                // Check for new commits.
                let o = &self.pr_ref.owner;
                let r = &self.pr_ref.repo;
                let n = self.pr_ref.number;

                let current_sha = self
                    .github
                    .get_pr_head_sha(o, r, n)
                    .await
                    .map_err(|e| TaskError::PollFailed(format!("failed to check SHA: {e}")))?;

                if self.last_sha.as_deref() == Some(&current_sha) {
                    // No change.
                    return Ok(vec![]);
                }

                // New commits — re-review.
                self.do_review().await
            }
            _ => Ok(vec![]),
        }
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
