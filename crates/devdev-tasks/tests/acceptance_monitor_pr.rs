//! Acceptance tests for `MonitorPrTask` — event-driven shepherd.

use std::sync::Arc;

use async_trait::async_trait;
use devdev_integrations::{MockAdapter, PrState, PrStatus, PullRequest};
use devdev_tasks::agent::AgentRunner;
use devdev_tasks::events::{DaemonEvent, EventBus};
use devdev_tasks::monitor_pr::MonitorPrTask;
use devdev_tasks::pr_ref::PrRef;
use devdev_tasks::review::parse_review;
use devdev_tasks::task::{Task, TaskStatus};

fn mock_pr(number: u64, sha: &str) -> PullRequest {
    PullRequest {
        number,
        title: "Fix config validation".into(),
        author: "alice".into(),
        state: PrState::Open,
        head_sha: sha.into(),
        base_sha: "base000".into(),
        head_ref: "fix/config".into(),
        base_ref: "main".into(),
        body: Some("Fixes validation in parse_config.".into()),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-02T00:00:00Z".into(),
    }
}

fn mock_github(sha: &str) -> MockAdapter {
    MockAdapter::new()
        .with_pr("org", "repo", mock_pr(247, sha))
        .with_diff(
            "org",
            "repo",
            247,
            "diff --git a/src/config.rs\n+fn parse()",
        )
        .with_status(
            "org",
            "repo",
            247,
            PrStatus {
                mergeable: Some(true),
                checks: vec![],
            },
        )
}

/// Canned agent that records prompts and replies "looks good".
#[derive(Default)]
struct FakeRunner {
    seen: tokio::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl AgentRunner for FakeRunner {
    async fn run_prompt(&self, prompt: String) -> Result<String, String> {
        self.seen.lock().await.push(prompt);
        Ok("Overall looks good.".to_string())
    }
}

// ── PR ref parsing ─────────────────────────────────────────────

#[test]
fn parse_pr_ref_from_shorthand() {
    let pr = PrRef::parse("org/repo#247").unwrap();
    assert_eq!(pr.owner, "org");
    assert_eq!(pr.repo, "repo");
    assert_eq!(pr.number, 247);
}

#[test]
fn parse_pr_ref_from_url() {
    let pr = PrRef::parse("https://github.com/org/repo/pull/247").unwrap();
    assert_eq!(pr.owner, "org");
    assert_eq!(pr.repo, "repo");
    assert_eq!(pr.number, 247);
}

#[test]
fn parse_pr_ref_invalid_errors() {
    assert!(PrRef::parse("not_a_ref").is_err());
    assert!(PrRef::parse("").is_err());
    assert!(PrRef::parse("org/repo").is_err());
    assert!(PrRef::parse("#123").is_err());
}

// ── Review parsing ─────────────────────────────────────────────

#[test]
fn parse_structured_review() {
    let text = "[src/config.rs:42] bad validation\n[src/lib.rs:10] unused import";
    let review = parse_review(text);
    assert_eq!(review.comments.len(), 2);
    assert_eq!(review.comments[0].path, "src/config.rs");
    assert_eq!(review.comments[0].line, 42);
}

// ── MonitorPrTask lifecycle (event-driven) ─────────────────────

#[tokio::test]
async fn idle_with_no_events_is_quiet() {
    let gh: Arc<dyn devdev_integrations::RepoHostAdapter> = Arc::new(mock_github("abc123"));
    let bus = EventBus::new();
    let runner: Arc<dyn AgentRunner> = Arc::new(FakeRunner::default());
    let mut task = MonitorPrTask::new("t-1".into(), "org/repo#247", gh, runner, &bus).unwrap();

    let msgs = task.poll().await.unwrap();
    assert!(msgs.is_empty());
}

#[tokio::test]
async fn pr_opened_event_triggers_agent_prompt() {
    let gh: Arc<dyn devdev_integrations::RepoHostAdapter> = Arc::new(mock_github("abc123"));
    let bus = EventBus::new();
    let runner = Arc::new(FakeRunner::default());
    let runner_dyn: Arc<dyn AgentRunner> = runner.clone();
    let mut task = MonitorPrTask::new("t-1".into(), "org/repo#247", gh, runner_dyn, &bus).unwrap();

    bus.publish(DaemonEvent::PrOpened {
        owner: "org".into(),
        repo: "repo".into(),
        number: 247,
        head_sha: "abc123".into(),
    });

    let msgs = task.poll().await.unwrap();
    assert!(!msgs.is_empty());
    let seen = runner.seen.lock().await;
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("opened"));
    assert!(seen[0].contains("abc123"));
}

#[tokio::test]
async fn pr_updated_event_triggers_agent_prompt() {
    let gh: Arc<dyn devdev_integrations::RepoHostAdapter> = Arc::new(mock_github("abc123"));
    let bus = EventBus::new();
    let runner = Arc::new(FakeRunner::default());
    let runner_dyn: Arc<dyn AgentRunner> = runner.clone();
    let mut task = MonitorPrTask::new("t-1".into(), "org/repo#247", gh, runner_dyn, &bus).unwrap();

    bus.publish(DaemonEvent::PrUpdated {
        owner: "org".into(),
        repo: "repo".into(),
        number: 247,
        head_sha: "def456".into(),
    });

    task.poll().await.unwrap();
    let seen = runner.seen.lock().await;
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("updated"));
}

#[tokio::test]
async fn pr_closed_event_completes_task() {
    let gh: Arc<dyn devdev_integrations::RepoHostAdapter> = Arc::new(mock_github("abc123"));
    let bus = EventBus::new();
    let runner: Arc<dyn AgentRunner> = Arc::new(FakeRunner::default());
    let mut task = MonitorPrTask::new("t-1".into(), "org/repo#247", gh, runner, &bus).unwrap();

    bus.publish(DaemonEvent::PrClosed {
        owner: "org".into(),
        repo: "repo".into(),
        number: 247,
        merged: true,
    });

    let msgs = task.poll().await.unwrap();
    assert!(!msgs.is_empty());
    assert_eq!(task.status(), &TaskStatus::Completed);
}

#[tokio::test]
async fn non_matching_event_is_ignored() {
    let gh: Arc<dyn devdev_integrations::RepoHostAdapter> = Arc::new(mock_github("abc123"));
    let bus = EventBus::new();
    let runner = Arc::new(FakeRunner::default());
    let runner_dyn: Arc<dyn AgentRunner> = runner.clone();
    let mut task = MonitorPrTask::new("t-1".into(), "org/repo#247", gh, runner_dyn, &bus).unwrap();

    // Different PR number.
    bus.publish(DaemonEvent::PrOpened {
        owner: "org".into(),
        repo: "repo".into(),
        number: 999,
        head_sha: "x".into(),
    });

    let msgs = task.poll().await.unwrap();
    assert!(msgs.is_empty());
    assert!(runner.seen.lock().await.is_empty());
}

#[tokio::test]
async fn observations_accumulate_across_events() {
    let gh: Arc<dyn devdev_integrations::RepoHostAdapter> = Arc::new(mock_github("abc123"));
    let bus = EventBus::new();
    let runner: Arc<dyn AgentRunner> = Arc::new(FakeRunner::default());
    let mut task = MonitorPrTask::new("t-1".into(), "org/repo#247", gh, runner, &bus).unwrap();

    bus.publish(DaemonEvent::PrOpened {
        owner: "org".into(),
        repo: "repo".into(),
        number: 247,
        head_sha: "abc123".into(),
    });
    task.poll().await.unwrap();

    bus.publish(DaemonEvent::PrUpdated {
        owner: "org".into(),
        repo: "repo".into(),
        number: 247,
        head_sha: "def456".into(),
    });
    task.poll().await.unwrap();

    assert_eq!(task.observations().len(), 2);
}

#[tokio::test]
async fn merged_pr_short_circuits_to_completed() {
    let mut pr = mock_pr(247, "abc123");
    pr.state = PrState::Merged;
    let gh: Arc<dyn devdev_integrations::RepoHostAdapter> = Arc::new(
        MockAdapter::new()
            .with_pr("org", "repo", pr)
            .with_diff("org", "repo", 247, ""),
    );
    let bus = EventBus::new();
    let runner: Arc<dyn AgentRunner> = Arc::new(FakeRunner::default());
    let mut task = MonitorPrTask::new("t-1".into(), "org/repo#247", gh, runner, &bus).unwrap();

    bus.publish(DaemonEvent::PrUpdated {
        owner: "org".into(),
        repo: "repo".into(),
        number: 247,
        head_sha: "abc123".into(),
    });
    task.poll().await.unwrap();
    assert_eq!(task.status(), &TaskStatus::Completed);
}

#[tokio::test]
async fn serialize_includes_pr_state() {
    let gh: Arc<dyn devdev_integrations::RepoHostAdapter> = Arc::new(mock_github("abc123"));
    let bus = EventBus::new();
    let runner: Arc<dyn AgentRunner> = Arc::new(FakeRunner::default());
    let task = MonitorPrTask::new("t-1".into(), "org/repo#247", gh, runner, &bus).unwrap();

    let data = task.serialize().unwrap();
    assert_eq!(data["id"], "t-1");
    assert_eq!(data["owner"], "org");
    assert_eq!(data["repo"], "repo");
    assert_eq!(data["number"], 247);
}
