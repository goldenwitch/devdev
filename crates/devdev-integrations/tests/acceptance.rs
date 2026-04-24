//! Acceptance tests for P2-05 — GitHub Integration Adapter.
//!
//! These tests use the MockGitHubAdapter. Live API tests are gated
//! behind DEVDEV_E2E (not run in CI).

use devdev_integrations::{
    CheckRun, Comment, GitHubAdapter, GitHubError, MockGitHubAdapter, PrState, PrStatus,
    PullRequest, Review, ReviewComment, ReviewEvent,
};

fn sample_pr() -> PullRequest {
    PullRequest {
        number: 42,
        title: "Fix widget crash".into(),
        author: "alice".into(),
        state: PrState::Open,
        head_sha: "abc123".into(),
        base_sha: "def456".into(),
        head_ref: "fix/widget".into(),
        base_ref: "main".into(),
        body: Some("Fixes #99".into()),
        created_at: "2025-01-01T00:00:00Z".into(),
        updated_at: "2025-01-02T00:00:00Z".into(),
    }
}

// ── Mock: get_pr returns configured data ───────────────────────

#[tokio::test]
async fn mock_returns_configured_pr() {
    let adapter = MockGitHubAdapter::new().with_pr("org", "repo", sample_pr());

    let pr = adapter.get_pr("org", "repo", 42).await.unwrap();
    assert_eq!(pr.number, 42);
    assert_eq!(pr.title, "Fix widget crash");
    assert_eq!(pr.author, "alice");
    assert_eq!(pr.state, PrState::Open);
    assert_eq!(pr.head_sha, "abc123");
}

// ── Mock: get_pr not found ─────────────────────────────────────

#[tokio::test]
async fn mock_get_pr_not_found() {
    let adapter = MockGitHubAdapter::new();
    let err = adapter.get_pr("org", "repo", 999).await.err().unwrap();
    assert!(matches!(err, GitHubError::NotFound(_)));
}

// ── Mock: get_pr_diff ──────────────────────────────────────────

#[tokio::test]
async fn mock_get_pr_diff_returns_diff() {
    let diff =
        "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let adapter = MockGitHubAdapter::new().with_diff("org", "repo", 42, diff);

    let result = adapter.get_pr_diff("org", "repo", 42).await.unwrap();
    assert_eq!(result, diff);
}

// ── Mock: list_pr_comments ─────────────────────────────────────

#[tokio::test]
async fn mock_list_comments() {
    let comments = vec![
        Comment {
            id: 1,
            author: "bob".into(),
            body: "Looks good".into(),
            path: Some("src/main.rs".into()),
            line: Some(10),
            created_at: "2025-01-01T00:00:00Z".into(),
        },
        Comment {
            id: 2,
            author: "carol".into(),
            body: "Needs tests".into(),
            path: None,
            line: None,
            created_at: "2025-01-01T01:00:00Z".into(),
        },
    ];
    let adapter = MockGitHubAdapter::new().with_comments("org", "repo", 42, comments);

    let result = adapter.list_pr_comments("org", "repo", 42).await.unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].author, "bob");
    assert_eq!(result[1].body, "Needs tests");
}

// ── Mock: empty comments for unknown PR ────────────────────────

#[tokio::test]
async fn mock_list_comments_empty_for_unknown_pr() {
    let adapter = MockGitHubAdapter::new();
    let result = adapter.list_pr_comments("org", "repo", 99).await.unwrap();
    assert!(result.is_empty());
}

// ── Mock: records posted reviews ───────────────────────────────

#[tokio::test]
async fn mock_records_posted_reviews() {
    let adapter = MockGitHubAdapter::new();

    let review = Review {
        event: ReviewEvent::Comment,
        body: "Overall looks good".into(),
        comments: vec![ReviewComment {
            path: "src/lib.rs".into(),
            line: 42,
            body: "Consider renaming".into(),
        }],
    };

    adapter
        .post_review("org", "repo", 42, review)
        .await
        .unwrap();

    let posted = adapter.posted_reviews();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].0, "org");
    assert_eq!(posted[0].1, "repo");
    assert_eq!(posted[0].2, 42);
    assert_eq!(posted[0].3.body, "Overall looks good");
    assert_eq!(posted[0].3.comments.len(), 1);
    assert_eq!(posted[0].3.comments[0].path, "src/lib.rs");
}

// ── Mock: records posted comments ──────────────────────────────

#[tokio::test]
async fn mock_records_posted_comments() {
    let adapter = MockGitHubAdapter::new();

    adapter
        .post_comment("org", "repo", 42, "Nice work!")
        .await
        .unwrap();
    adapter
        .post_comment("org", "repo", 42, "Merging now")
        .await
        .unwrap();

    let posted = adapter.posted_comments();
    assert_eq!(posted.len(), 2);
    assert_eq!(posted[0].3, "Nice work!");
    assert_eq!(posted[1].3, "Merging now");
}

// ── Mock: get_pr_head_sha ──────────────────────────────────────

#[tokio::test]
async fn mock_get_pr_head_sha() {
    let adapter = MockGitHubAdapter::new().with_pr("org", "repo", sample_pr());

    let sha = adapter.get_pr_head_sha("org", "repo", 42).await.unwrap();
    assert_eq!(sha, "abc123");
}

// ── Mock: get_pr_status ────────────────────────────────────────

#[tokio::test]
async fn mock_get_pr_status() {
    let status = PrStatus {
        mergeable: Some(true),
        checks: vec![
            CheckRun {
                name: "CI".into(),
                status: "completed".into(),
                conclusion: Some("success".into()),
            },
            CheckRun {
                name: "lint".into(),
                status: "completed".into(),
                conclusion: Some("failure".into()),
            },
        ],
    };

    let adapter = MockGitHubAdapter::new().with_status("org", "repo", 42, status);

    let result = adapter.get_pr_status("org", "repo", 42).await.unwrap();
    assert_eq!(result.mergeable, Some(true));
    assert_eq!(result.checks.len(), 2);
    assert_eq!(result.checks[0].name, "CI");
    assert_eq!(result.checks[1].conclusion, Some("failure".into()));
}

// ── Mock: diff not found ───────────────────────────────────────

#[tokio::test]
async fn mock_diff_not_found() {
    let adapter = MockGitHubAdapter::new();
    let err = adapter.get_pr_diff("org", "repo", 99).await.err().unwrap();
    assert!(matches!(err, GitHubError::NotFound(_)));
}

// ── Mock: status not found ─────────────────────────────────────

#[tokio::test]
async fn mock_status_not_found() {
    let adapter = MockGitHubAdapter::new();
    let err = adapter
        .get_pr_status("org", "repo", 99)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, GitHubError::NotFound(_)));
}

// ── Live: token_not_set_errors ─────────────────────────────────

#[test]
fn token_not_set_errors() {
    // Ensure GH_TOKEN is not set for this test
    // SAFETY: No other threads are reading GH_TOKEN concurrently in this test.
    unsafe { std::env::remove_var("GH_TOKEN") };
    let result = devdev_integrations::LiveGitHubAdapter::from_env();
    assert!(result.is_err());
    match result.err().unwrap() {
        GitHubError::TokenNotSet => {}
        e => panic!("expected TokenNotSet, got: {e}"),
    }
}

// ── Review event serialization ─────────────────────────────────

#[tokio::test]
async fn mock_post_review_preserves_event_type() {
    let adapter = MockGitHubAdapter::new();

    // Post an approval
    let review = Review {
        event: ReviewEvent::Approve,
        body: "LGTM".into(),
        comments: vec![],
    };
    adapter.post_review("org", "repo", 1, review).await.unwrap();

    // Post a request-changes
    let review = Review {
        event: ReviewEvent::RequestChanges,
        body: "Needs fixes".into(),
        comments: vec![],
    };
    adapter.post_review("org", "repo", 2, review).await.unwrap();

    let posted = adapter.posted_reviews();
    assert_eq!(posted[0].3.event, ReviewEvent::Approve);
    assert_eq!(posted[1].3.event, ReviewEvent::RequestChanges);
}

// ── PR state variants ──────────────────────────────────────────

#[tokio::test]
async fn mock_pr_state_variants() {
    let mut closed_pr = sample_pr();
    closed_pr.number = 10;
    closed_pr.state = PrState::Closed;

    let mut merged_pr = sample_pr();
    merged_pr.number = 20;
    merged_pr.state = PrState::Merged;

    let adapter = MockGitHubAdapter::new()
        .with_pr("org", "repo", sample_pr()) // Open, #42
        .with_pr("org", "repo", closed_pr)
        .with_pr("org", "repo", merged_pr);

    let open = adapter.get_pr("org", "repo", 42).await.unwrap();
    assert_eq!(open.state, PrState::Open);

    let closed = adapter.get_pr("org", "repo", 10).await.unwrap();
    assert_eq!(closed.state, PrState::Closed);

    let merged = adapter.get_pr("org", "repo", 20).await.unwrap();
    assert_eq!(merged.state, PrState::Merged);
}
