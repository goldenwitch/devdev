//! `RepoWatchTask` — observes a GitHub repo and emits PR events.
//!
//! The polling counterpart to a webhook receiver. Each tick:
//!
//! 1. List open PRs via the [`GitHubAdapter`].
//! 2. For each PR, compute a state hash (head_sha + updated_at) and
//!    consult the [`IdempotencyLedger`]. If we've seen this exact
//!    state, skip — we already published the corresponding event.
//! 3. For new states, publish [`DaemonEvent::PrOpened`] (first time
//!    we've seen this PR number) or [`DaemonEvent::PrUpdated`] (we
//!    knew the PR but its hash moved). Then record in the ledger.
//! 4. PRs that disappeared since the last poll are reported via
//!    [`DaemonEvent::PrClosed`].
//!
//! State carried across polls is intentionally minimal: a map of
//! `pr_number → state_hash` for open/close detection. Cross-restart
//! dedup is the ledger's job, not ours.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devdev_integrations::{GitHubAdapter, pr_state_hash};

use crate::events::{DaemonEvent, EventBus};
use crate::ledger::{IdempotencyLedger, LedgerKey};
use crate::task::{Task, TaskError, TaskMessage, TaskStatus};

const ADAPTER: &str = "github";
const RESOURCE_TYPE: &str = "pr_state";

/// A task that polls a single repo's open PRs and emits events.
pub struct RepoWatchTask {
    id: String,
    owner: String,
    repo: String,
    /// `pr_number → state_hash` for the most recent poll.
    last_seen: HashMap<u64, String>,
    poll_interval: Duration,
    /// When `poll()` last actually ran. `None` until the first run.
    last_polled: Option<Instant>,
    status: TaskStatus,
    github: Arc<dyn GitHubAdapter>,
    ledger: Arc<dyn IdempotencyLedger>,
    bus: EventBus,
}

impl RepoWatchTask {
    pub fn new(
        id: String,
        owner: impl Into<String>,
        repo: impl Into<String>,
        github: Arc<dyn GitHubAdapter>,
        ledger: Arc<dyn IdempotencyLedger>,
        bus: EventBus,
    ) -> Self {
        Self {
            id,
            owner: owner.into(),
            repo: repo.into(),
            last_seen: HashMap::new(),
            poll_interval: Duration::from_secs(60),
            last_polled: None,
            status: TaskStatus::Created,
            github,
            ledger,
            bus,
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }

    fn resource_id(&self, number: u64) -> String {
        format!("{}/{}#{}", self.owner, self.repo, number)
    }

    async fn do_poll(&mut self) -> Result<Vec<TaskMessage>, TaskError> {
        let prs = self
            .github
            .list_open_prs(&self.owner, &self.repo)
            .await
            .map_err(|e| TaskError::PollFailed(format!("list_open_prs: {e}")))?;

        let mut messages = Vec::new();
        let mut current: HashMap<u64, String> = HashMap::new();

        for pr in &prs {
            let hash = pr_state_hash(pr);
            current.insert(pr.number, hash.clone());

            let key = LedgerKey::new(ADAPTER, RESOURCE_TYPE, self.resource_id(pr.number), &hash);

            // Ledger consult — if we've published this exact state
            // before (across the lifetime of the daemon), skip.
            let already = self
                .ledger
                .seen(&key)
                .map_err(|e| TaskError::PollFailed(format!("ledger.seen: {e}")))?;
            if already {
                continue;
            }

            let event = if self.last_seen.contains_key(&pr.number) {
                DaemonEvent::PrUpdated {
                    owner: self.owner.clone(),
                    repo: self.repo.clone(),
                    number: pr.number,
                    head_sha: pr.head_sha.clone(),
                }
            } else {
                DaemonEvent::PrOpened {
                    owner: self.owner.clone(),
                    repo: self.repo.clone(),
                    number: pr.number,
                    head_sha: pr.head_sha.clone(),
                }
            };
            self.bus.publish(event);
            self.ledger
                .record(
                    &key,
                    serde_json::json!({
                        "head_sha": pr.head_sha,
                        "updated_at": pr.updated_at,
                    }),
                )
                .map_err(|e| TaskError::PollFailed(format!("ledger.record: {e}")))?;

            messages.push(TaskMessage::Text(format!(
                "{} #{} state {}",
                self.resource_id(pr.number),
                pr.number,
                if self.last_seen.contains_key(&pr.number) {
                    "updated"
                } else {
                    "opened"
                }
            )));
        }

        // PrClosed: anything in last_seen but not in current.
        for (number, _) in self.last_seen.iter() {
            if !current.contains_key(number) {
                // We can't know merged-vs-closed cheaply without an
                // extra `get_pr` call. Best-effort: assume closed
                // (mergeable=false) and let MonitorPrTask resolve via
                // its own get_pr_status if it cares.
                let event = DaemonEvent::PrClosed {
                    owner: self.owner.clone(),
                    repo: self.repo.clone(),
                    number: *number,
                    merged: false,
                };
                self.bus.publish(event);
                messages.push(TaskMessage::Text(format!(
                    "{} closed",
                    self.resource_id(*number)
                )));
            }
        }

        self.last_seen = current;
        self.status = TaskStatus::Idle;
        Ok(messages)
    }
}

#[async_trait::async_trait]
impl Task for RepoWatchTask {
    fn id(&self) -> &str {
        &self.id
    }

    fn describe(&self) -> String {
        format!("Watching {}/{} for PR events", self.owner, self.repo)
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
        // Self-throttle: callers may invoke `poll()` faster than
        // our `poll_interval`. Skip until we're due.
        if let Some(prev) = self.last_polled
            && prev.elapsed() < self.poll_interval
        {
            return Ok(vec![]);
        }
        self.last_polled = Some(Instant::now());
        self.do_poll().await
    }

    fn serialize(&self) -> Result<serde_json::Value, TaskError> {
        Ok(serde_json::json!({
            "id": self.id,
            "owner": self.owner,
            "repo": self.repo,
            "last_seen": self.last_seen,
            "poll_interval_secs": self.poll_interval.as_secs(),
        }))
    }

    fn task_type(&self) -> &str {
        "repo_watch"
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devdev_integrations::{MockGitHubAdapter, PrState, PullRequest};

    /// In-memory test ledger.
    #[derive(Default)]
    struct MemLedger {
        seen: std::sync::Mutex<std::collections::HashSet<LedgerKey>>,
    }
    impl IdempotencyLedger for MemLedger {
        fn seen(&self, key: &LedgerKey) -> Result<bool, crate::ledger::LedgerError> {
            Ok(self.seen.lock().unwrap().contains(key))
        }
        fn record(
            &self,
            key: &LedgerKey,
            _meta: serde_json::Value,
        ) -> Result<(), crate::ledger::LedgerError> {
            self.seen.lock().unwrap().insert(key.clone());
            Ok(())
        }
        fn prune(&self, _older_than: Duration) -> Result<usize, crate::ledger::LedgerError> {
            Ok(0)
        }
    }

    fn pr(number: u64, sha: &str, updated: &str) -> PullRequest {
        PullRequest {
            number,
            title: format!("PR {number}"),
            author: "test".into(),
            state: PrState::Open,
            head_sha: sha.into(),
            base_sha: "base".into(),
            head_ref: "head".into(),
            base_ref: "main".into(),
            body: None,
            created_at: "2026-01-01".into(),
            updated_at: updated.into(),
        }
    }

    fn task() -> (
        RepoWatchTask,
        Arc<MockGitHubAdapter>,
        Arc<MemLedger>,
        EventBus,
    ) {
        let gh = Arc::new(MockGitHubAdapter::new());
        let ledger = Arc::new(MemLedger::default());
        let bus = EventBus::new();
        let t = RepoWatchTask::new(
            "t-1".into(),
            "o",
            "r",
            gh.clone() as Arc<dyn GitHubAdapter>,
            ledger.clone() as Arc<dyn IdempotencyLedger>,
            bus.clone(),
        );
        (t, gh, ledger, bus)
    }

    #[tokio::test]
    async fn empty_repo_emits_nothing() {
        let (mut t, _gh, _l, bus) = task();
        let mut rx = bus.subscribe();
        let msgs = t.poll().await.unwrap();
        assert!(msgs.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn first_pr_emits_opened() {
        let gh = Arc::new(MockGitHubAdapter::new().with_pr("o", "r", pr(1, "sha1", "t1")));
        let ledger = Arc::new(MemLedger::default());
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut t = RepoWatchTask::new(
            "t-1".into(),
            "o",
            "r",
            gh as Arc<dyn GitHubAdapter>,
            ledger as Arc<dyn IdempotencyLedger>,
            bus,
        );
        t.poll().await.unwrap();
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, DaemonEvent::PrOpened { number: 1, .. }));
    }

    #[tokio::test]
    async fn second_poll_no_change_emits_nothing() {
        let gh = Arc::new(MockGitHubAdapter::new().with_pr("o", "r", pr(1, "sha1", "t1")));
        let ledger = Arc::new(MemLedger::default());
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut t = RepoWatchTask::new(
            "t-1".into(),
            "o",
            "r",
            gh as Arc<dyn GitHubAdapter>,
            ledger as Arc<dyn IdempotencyLedger>,
            bus,
        );
        t.poll().await.unwrap();
        let _ = rx.recv().await.unwrap();
        // Second poll: same SHA + updated_at → no event.
        t.poll().await.unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn updated_pr_emits_pr_updated() {
        let gh = Arc::new(MockGitHubAdapter::new().with_pr("o", "r", pr(1, "sha1", "t1")));
        let ledger = Arc::new(MemLedger::default());
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut t = RepoWatchTask::new(
            "t-1".into(),
            "o",
            "r",
            gh.clone() as Arc<dyn GitHubAdapter>,
            ledger as Arc<dyn IdempotencyLedger>,
            bus,
        );
        t.poll().await.unwrap();
        let _ = rx.recv().await.unwrap();
        // Simulate force-push: head_sha changes via mock override.
        gh.update_head_sha("o", "r", 1, "sha2");
        t.poll().await.unwrap();
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, DaemonEvent::PrUpdated { number: 1, .. }));
    }

    #[tokio::test]
    async fn closed_pr_emits_pr_closed() {
        // Two adapters: one with PR open, one without.
        let gh_open = Arc::new(MockGitHubAdapter::new().with_pr("o", "r", pr(1, "sha1", "t1")));
        let ledger = Arc::new(MemLedger::default());
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mut t = RepoWatchTask::new(
            "t-1".into(),
            "o",
            "r",
            gh_open as Arc<dyn GitHubAdapter>,
            ledger.clone() as Arc<dyn IdempotencyLedger>,
            bus.clone(),
        );
        t.poll().await.unwrap();
        let _ = rx.recv().await.unwrap();

        // Replace adapter with empty one (PR disappeared).
        let gh_empty = Arc::new(MockGitHubAdapter::new());
        t.github = gh_empty;
        t.poll().await.unwrap();
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, DaemonEvent::PrClosed { number: 1, .. }));
    }

    #[tokio::test]
    async fn ledger_dedups_across_restart() {
        let gh = Arc::new(MockGitHubAdapter::new().with_pr("o", "r", pr(1, "sha1", "t1")));
        let ledger = Arc::new(MemLedger::default());
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // First task instance: emits PrOpened.
        let mut t1 = RepoWatchTask::new(
            "t-1".into(),
            "o",
            "r",
            gh.clone() as Arc<dyn GitHubAdapter>,
            ledger.clone() as Arc<dyn IdempotencyLedger>,
            bus.clone(),
        );
        t1.poll().await.unwrap();
        let _ = rx.recv().await.unwrap();
        drop(t1);

        // Second task instance ("restart"): same ledger, fresh in-memory state.
        let mut t2 = RepoWatchTask::new(
            "t-1".into(),
            "o",
            "r",
            gh as Arc<dyn GitHubAdapter>,
            ledger as Arc<dyn IdempotencyLedger>,
            bus,
        );
        t2.poll().await.unwrap();
        // Ledger says "seen" — no event.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn serialize_round_trips() {
        let (mut t, _gh, _l, _bus) = task();
        t.last_seen.insert(1, "hash1".into());
        let v = t.serialize().unwrap();
        assert_eq!(v["owner"], "o");
        assert_eq!(v["repo"], "r");
        assert_eq!(v["last_seen"]["1"], "hash1");
    }
}
