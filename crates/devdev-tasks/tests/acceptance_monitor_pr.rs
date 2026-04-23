//! Acceptance tests for P2-07 — MonitorPR Task.

use std::sync::Arc;
use std::time::Duration;

use devdev_integrations::{MockGitHubAdapter, PrState, PullRequest, PrStatus};
use devdev_tasks::approval::{self, ApprovalPolicy, ApprovalResponse};
use devdev_tasks::monitor_pr::{MonitorPrTask, ReviewFn};
use devdev_tasks::pr_ref::PrRef;
use devdev_tasks::review::parse_review;
use devdev_tasks::task::{Task, TaskStatus};
use tokio::sync::Mutex;

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

fn fake_review_fn() -> ReviewFn {
    Arc::new(|_prompt| {
        Box::pin(async {
            Ok("Overall looks good.\n[src/config.rs:42] Missing validation for empty strings.\n[src/lib.rs:10] Unused import.".to_string())
        })
    })
}

fn mock_github(sha: &str) -> MockGitHubAdapter {
    MockGitHubAdapter::new()
        .with_pr("org", "repo", mock_pr(247, sha))
        .with_diff("org", "repo", 247, "diff --git a/src/config.rs\n+fn parse()")
        .with_status("org", "repo", 247, PrStatus { mergeable: Some(true), checks: vec![] })
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
    assert_eq!(review.comments[1].path, "src/lib.rs");
}

#[test]
fn parse_fallback_body_only() {
    let text = "The code looks fine overall. No major issues found.";
    let review = parse_review(text);
    assert!(review.comments.is_empty());
    assert!(!review.body.is_empty());
}

#[test]
fn parse_mixed() {
    let text = "Summary: looks ok.\n[src/config.rs:42] bad validation\nOther notes.";
    let review = parse_review(text);
    assert_eq!(review.comments.len(), 1);
    assert!(review.body.contains("Summary"));
    assert!(review.body.contains("Other notes"));
}

// ── MonitorPrTask lifecycle ────────────────────────────────────

#[tokio::test]
async fn first_poll_loads_and_reviews() {
    let github: Arc<dyn devdev_integrations::GitHubAdapter> = Arc::new(mock_github("abc123"));
    let (gate, _handle) = approval::approval_channel(ApprovalPolicy::AutoApprove, Duration::from_secs(5));
    let gate = Arc::new(Mutex::new(gate));

    let mut task = MonitorPrTask::new(
        "t-1".into(),
        "org/repo#247",
        github,
        gate,
        fake_review_fn(),
    )
    .unwrap();

    let msgs = task.poll().await.unwrap();
    assert!(!msgs.is_empty());
}

#[tokio::test]
async fn first_poll_posts_review_when_approved() {
    let gh = Arc::new(mock_github("abc123"));
    let github: Arc<dyn devdev_integrations::GitHubAdapter> = Arc::clone(&gh) as Arc<dyn devdev_integrations::GitHubAdapter>;
    let (gate, _handle) = approval::approval_channel(ApprovalPolicy::AutoApprove, Duration::from_secs(5));
    let gate = Arc::new(Mutex::new(gate));

    let mut task = MonitorPrTask::new(
        "t-1".into(),
        "org/repo#247",
        github,
        gate,
        fake_review_fn(),
    )
    .unwrap();

    let _msgs = task.poll().await.unwrap();
    assert!(!gh.posted_reviews().is_empty());
}

#[tokio::test]
async fn first_poll_skips_post_when_rejected() {
    let gh = Arc::new(mock_github("abc123"));
    let github: Arc<dyn devdev_integrations::GitHubAdapter> = Arc::clone(&gh) as Arc<dyn devdev_integrations::GitHubAdapter>;
    let (gate, mut handle) = approval::approval_channel(ApprovalPolicy::Ask, Duration::from_secs(5));
    let gate = Arc::new(Mutex::new(gate));

    // Respond with reject in background.
    tokio::spawn(async move {
        let req = handle.request_rx.recv().await.unwrap();
        handle
            .response_tx
            .send(ApprovalResponse { id: req.id, approve: false })
            .await
            .unwrap();
    });

    let mut task = MonitorPrTask::new(
        "t-1".into(),
        "org/repo#247",
        github,
        gate,
        fake_review_fn(),
    )
    .unwrap();

    let msgs = task.poll().await.unwrap();
    assert!(!msgs.is_empty()); // Should have "rejected" message.
    assert!(gh.posted_reviews().is_empty()); // No review posted.
}

#[tokio::test]
async fn subsequent_poll_no_change_quiet() {
    let github: Arc<dyn devdev_integrations::GitHubAdapter> = Arc::new(mock_github("abc123"));
    let (gate, _handle) = approval::approval_channel(ApprovalPolicy::AutoApprove, Duration::from_secs(5));
    let gate = Arc::new(Mutex::new(gate));

    let mut task = MonitorPrTask::new(
        "t-1".into(),
        "org/repo#247",
        github,
        gate,
        fake_review_fn(),
    )
    .unwrap();

    // First poll — does review.
    let _msgs = task.poll().await.unwrap();

    // Set to Idle.
    task.set_status(TaskStatus::Idle);

    // Second poll — same SHA, should be quiet.
    let msgs = task.poll().await.unwrap();
    assert!(msgs.is_empty());
}

#[tokio::test]
async fn pr_merged_transitions_to_completed() {
    let mut pr = mock_pr(247, "abc123");
    pr.state = PrState::Merged;

    let github: Arc<dyn devdev_integrations::GitHubAdapter> = Arc::new(
        MockGitHubAdapter::new()
            .with_pr("org", "repo", pr)
            .with_diff("org", "repo", 247, "")
            .with_status("org", "repo", 247, PrStatus { mergeable: None, checks: vec![] }),
    );
    let (gate, _handle) = approval::approval_channel(ApprovalPolicy::AutoApprove, Duration::from_secs(5));
    let gate = Arc::new(Mutex::new(gate));

    let mut task = MonitorPrTask::new(
        "t-1".into(),
        "org/repo#247",
        github,
        gate,
        fake_review_fn(),
    )
    .unwrap();

    let msgs = task.poll().await.unwrap();
    assert!(!msgs.is_empty());
    assert_eq!(task.status(), &TaskStatus::Completed);
}

#[tokio::test]
async fn serialize_deserialize_roundtrip() {
    let github: Arc<dyn devdev_integrations::GitHubAdapter> = Arc::new(mock_github("abc123"));
    let (gate, _handle) = approval::approval_channel(ApprovalPolicy::AutoApprove, Duration::from_secs(5));
    let gate = Arc::new(Mutex::new(gate));

    let task = MonitorPrTask::new(
        "t-1".into(),
        "org/repo#247",
        github,
        gate,
        fake_review_fn(),
    )
    .unwrap();

    let data = task.serialize().unwrap();
    assert_eq!(data["id"], "t-1");
    assert_eq!(data["owner"], "org");
    assert_eq!(data["repo"], "repo");
    assert_eq!(data["number"], 247);
}

#[tokio::test]
async fn dry_run_never_posts() {
    let gh = Arc::new(mock_github("abc123"));
    let github: Arc<dyn devdev_integrations::GitHubAdapter> = Arc::clone(&gh) as Arc<dyn devdev_integrations::GitHubAdapter>;
    let (gate, _handle) = approval::approval_channel(ApprovalPolicy::DryRun, Duration::from_secs(5));
    let gate = Arc::new(Mutex::new(gate));

    let mut task = MonitorPrTask::new(
        "t-1".into(),
        "org/repo#247",
        github,
        gate,
        fake_review_fn(),
    )
    .unwrap();

    let msgs = task.poll().await.unwrap();
    assert!(!msgs.is_empty());
    // Verify dry-run message.
    if let devdev_tasks::TaskMessage::Text(text) = &msgs[0] {
        assert!(text.contains("dry-run"));
    }
    assert!(gh.posted_reviews().is_empty());
}
