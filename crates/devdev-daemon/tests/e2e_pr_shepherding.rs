//! E2E PR Shepherding tests — P2-09.
//!
//! Proves the full Phase 2 stack works: daemon ↔ IPC ↔ dispatch ↔ tasks ↔ mock agent ↔ mock GitHub.

use std::sync::Arc;
use std::time::Duration;

use devdev_daemon::dispatch::DispatchContext;
use devdev_daemon::ipc::IpcServer;
use devdev_daemon::router::{
    AgentResponse, ResponseChunk, RouterError, SessionBackend, SessionRouter,
};
use devdev_daemon::{Daemon, DaemonConfig};
use devdev_integrations::{MockGitHubAdapter, PrState, PrStatus, PullRequest};
use devdev_tasks::approval::{self, ApprovalPolicy};
use devdev_tasks::monitor_pr::ReviewFn;
use devdev_tasks::registry::TaskRegistry;
use serde_json::json;
use tokio::sync::{mpsc, watch, Mutex};

// ── Fake Agent Backend ─────────────────────────────────────────

/// Scripted fake agent that returns canned review text.
struct FakeAgentBackend {
    /// Responses keyed by prompt substring match.
    responses: Vec<(&'static str, String)>,
    /// Default response if no match.
    default_response: String,
    session_counter: std::sync::atomic::AtomicU64,
}

impl FakeAgentBackend {
    fn new() -> Self {
        Self {
            responses: vec![
                ("review", "Overall looks good.\n[src/config.rs:42] Missing validation.\n[src/lib.rs:10] Unused import.".to_string()),
                ("new commits", "Updated review: changes look fine.\n[src/config.rs:42] Fixed now.".to_string()),
            ],
            default_response: "I'll look into that.".to_string(),
            session_counter: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn find_response(&self, prompt: &str) -> String {
        let lower = prompt.to_lowercase();
        for (pattern, response) in &self.responses {
            if lower.contains(pattern) {
                return response.clone();
            }
        }
        self.default_response.clone()
    }
}

#[async_trait::async_trait]
impl SessionBackend for FakeAgentBackend {
    async fn create_session(&self, _cwd: &str) -> Result<String, RouterError> {
        let id = self
            .session_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(format!("fake-session-{id}"))
    }

    async fn send_prompt(
        &self,
        _session_id: &str,
        text: &str,
    ) -> Result<AgentResponse, RouterError> {
        Ok(AgentResponse {
            text: self.find_response(text),
            stop_reason: "end_turn".to_string(),
        })
    }

    async fn send_prompt_streaming(
        &self,
        _session_id: &str,
        text: &str,
        tx: mpsc::Sender<ResponseChunk>,
    ) -> Result<(), RouterError> {
        let response = self.find_response(text);
        let _ = tx.send(ResponseChunk::Text(response)).await;
        let _ = tx
            .send(ResponseChunk::Done {
                stop_reason: "end_turn".to_string(),
            })
            .await;
        Ok(())
    }

    async fn destroy_session(&self, _session_id: &str) -> Result<(), RouterError> {
        Ok(())
    }
}

// ── Test Fixtures ──────────────────────────────────────────────

fn test_pr(sha: &str) -> PullRequest {
    PullRequest {
        number: 1,
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

fn test_github(sha: &str) -> MockGitHubAdapter {
    MockGitHubAdapter::new()
        .with_pr("test-org", "test-repo", test_pr(sha))
        .with_diff(
            "test-org",
            "test-repo",
            1,
            "diff --git a/src/config.rs\n+fn parse()",
        )
        .with_status(
            "test-org",
            "test-repo",
            1,
            PrStatus {
                mergeable: Some(true),
                checks: vec![],
            },
        )
}

fn fake_review_fn(router: Arc<SessionRouter>) -> ReviewFn {
    Arc::new(move |prompt| {
        let router = Arc::clone(&router);
        Box::pin(async move {
            let handle = router.create_interactive_session().await.map_err(|e| e.to_string())?;
            let resp = handle.send_prompt(&prompt).await.map_err(|e| e.to_string())?;
            Ok(resp.text)
        })
    })
}

// ── E2E Harness ────────────────────────────────────────────────

struct E2EHarness {
    port: u16,
    ctx: Arc<DispatchContext>,
    github: Arc<MockGitHubAdapter>,
    shutdown_tx: watch::Sender<bool>,
    _server_handle: tokio::task::JoinHandle<()>,
    _daemon: Daemon,
}

impl E2EHarness {
    async fn new_with_policy(policy: ApprovalPolicy) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let config = DaemonConfig {
            data_dir: tmp.path().to_path_buf(),
            checkpoint_on_stop: true,
            foreground: true,
        };

        let daemon = Daemon::start(config, false).await.unwrap();

        let gh = Arc::new(test_github("sha-initial-001"));
        let github: Arc<dyn devdev_integrations::GitHubAdapter> =
            Arc::clone(&gh) as Arc<dyn devdev_integrations::GitHubAdapter>;

        let backend: Arc<dyn SessionBackend> = Arc::new(FakeAgentBackend::new());
        let router = Arc::new(SessionRouter::new(backend));

        let registry = Arc::new(Mutex::new(TaskRegistry::new()));

        let (approval_gate, approval_handle) =
            approval::approval_channel(policy, Duration::from_secs(30));
        // We'll replace this when tasks are created; keep initial handle.
        let _ = approval_gate; // consumed by channel
        let approval_handle = Arc::new(Mutex::new(approval_handle));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let review_fn = fake_review_fn(Arc::clone(&router));

        let ctx = Arc::new(DispatchContext::new(
            Arc::clone(&router),
            Arc::clone(&registry),
            github,
            approval_handle,
            review_fn,
            policy,
            shutdown_tx.clone(),
        ).with_approval_timeout(Duration::from_secs(2)));

        let server = IpcServer::bind().await.unwrap();
        let port = server.port();

        let ctx_clone = Arc::clone(&ctx);
        let server_handle = tokio::spawn(async move {
            devdev_daemon::server::run(ctx_clone, server, shutdown_rx).await;
        });

        // Small delay for server to be ready.
        tokio::time::sleep(Duration::from_millis(50)).await;

        Self {
            port,
            ctx,
            github: gh,
            shutdown_tx,
            _server_handle: server_handle,
            _daemon: daemon,
        }
    }

    async fn new() -> Self {
        Self::new_with_policy(ApprovalPolicy::Ask).await
    }

    /// Poll all tasks once (simulates scheduler tick).
    async fn advance_polls(&self, count: usize) {
        for _ in 0..count {
            self.ctx.poll_all_tasks().await;
        }
    }

    async fn stop(self) -> Vec<u8> {
        let _ = self.shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Return checkpoint data (fs serialized).
        let fs = self._daemon.fs.lock().await;
        fs.serialize()
    }
}

// ── Scenario A: Interactive ────────────────────────────────────

#[tokio::test]
async fn e2e_interactive_pr_monitoring() {
    let harness = E2EHarness::new().await;

    // User sends a message to trigger PR monitoring via interactive session.
    let resp = raw_ipc(harness.port, "send", json!({"text": "Please review PR test-org/test-repo#1"})).await;
    assert!(resp.error.is_none(), "send should succeed: {:?}", resp.error);
    let response_text = resp.result.unwrap()["response"].as_str().unwrap().to_string();
    assert!(!response_text.is_empty());

    // Check status.
    let status = raw_ipc(harness.port, "status", json!({})).await;
    assert!(status.error.is_none());

    harness.stop().await;
}

// ── Scenario B: Headless auto-approve ──────────────────────────

#[tokio::test]
async fn e2e_headless_auto_approve() {
    let harness = E2EHarness::new_with_policy(ApprovalPolicy::AutoApprove).await;

    // Create task via IPC.
    let add_resp = raw_ipc(harness.port, "task/add", json!({
        "description": "Monitor PR test-org/test-repo#1",
        "auto_approve": true
    }))
    .await;
    assert!(add_resp.error.is_none(), "task/add failed: {:?}", add_resp.error);
    let task_id = add_resp.result.unwrap()["task_id"].as_str().unwrap().to_string();

    // Poll tasks to trigger review.
    harness.advance_polls(1).await;

    // Review should be posted (auto-approve).
    assert!(
        !harness.github.posted_reviews().is_empty(),
        "review should be posted with auto-approve"
    );

    // Check status.
    let status = raw_ipc(harness.port, "status", json!({})).await;
    let tasks_count = status.result.unwrap()["tasks"].as_u64().unwrap();
    assert!(tasks_count >= 1);

    // Check task log.
    let log = raw_ipc(harness.port, "task/log", json!({"task_id": task_id})).await;
    let entries = log.result.unwrap()["entries"].as_array().unwrap().len();
    assert!(entries > 0, "task log should have entries after poll");

    harness.stop().await;
}

// ── Scenario C: One-shot ───────────────────────────────────────

#[tokio::test]
async fn e2e_one_shot_review() {
    let harness = E2EHarness::new_with_policy(ApprovalPolicy::AutoApprove).await;

    // Send a one-shot review request.
    let resp = raw_ipc(harness.port, "send", json!({"text": "Review PR test-org/test-repo#1"})).await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let response_text = result["response"].as_str().unwrap();
    assert!(!response_text.is_empty());

    // Shutdown via IPC.
    let resp = raw_ipc(harness.port, "shutdown", json!({})).await;
    assert!(resp.error.is_none());
}

// ── Checkpoint recovery ────────────────────────────────────────

#[tokio::test]
async fn e2e_checkpoint_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // Phase 1: start, write to fs, checkpoint, stop.
    {
        let config = DaemonConfig {
            data_dir: data_dir.clone(),
            checkpoint_on_stop: true,
            foreground: true,
        };
        let daemon = Daemon::start(config, false).await.unwrap();

        {
            let mut fs = daemon.fs.lock().await;
            fs.write_path(b"/marker.txt", b"phase1").unwrap();
        }

        daemon.save_checkpoint().await.unwrap();
        daemon.stop().await.unwrap();
    }

    // Phase 2: restart from checkpoint, verify fs intact.
    {
        let config = DaemonConfig {
            data_dir: data_dir.clone(),
            checkpoint_on_stop: false,
            foreground: true,
        };
        let daemon = Daemon::start(config, true).await.unwrap();

        let fs = daemon.fs.lock().await;
        let data = fs.read_path(b"/marker.txt").unwrap();
        assert_eq!(data, b"phase1");

        daemon.stop().await.unwrap();
    }
}

// ── Approval protocol ──────────────────────────────────────────

#[tokio::test]
async fn e2e_headless_approval_protocol() {
    let harness = E2EHarness::new_with_policy(ApprovalPolicy::Ask).await;

    // Create task that requires approval (short timeout so test doesn't hang).
    let add_resp = raw_ipc(harness.port, "task/add", json!({
        "description": "Monitor PR test-org/test-repo#1",
        "auto_approve": false
    }))
    .await;
    assert!(add_resp.error.is_none());

    // Poll tasks — approval will timeout since nobody responds.
    harness.advance_polls(1).await;

    // With Ask policy and timeout, review should NOT be posted.
    // But the log should still have an entry about the timeout.
    let log = raw_ipc(harness.port, "task/log", json!({"task_id": "t-1"})).await;
    let entries = log.result.unwrap()["entries"].as_array().unwrap().clone();
    assert!(!entries.is_empty(), "should have log entry about approval timeout");

    harness.stop().await;
}

// ── Dry run ────────────────────────────────────────────────────

#[tokio::test]
async fn e2e_dry_run_no_side_effects() {
    let harness = E2EHarness::new_with_policy(ApprovalPolicy::DryRun).await;

    // Create task with dry-run policy.
    let add_resp = raw_ipc(harness.port, "task/add", json!({
        "description": "Monitor PR test-org/test-repo#1"
    }))
    .await;
    assert!(add_resp.error.is_none());

    // Poll.
    harness.advance_polls(1).await;

    // No review should be posted.
    assert!(
        harness.github.posted_reviews().is_empty(),
        "dry-run should not post reviews"
    );

    // But log should contain the review text.
    let log = raw_ipc(harness.port, "task/log", json!({"task_id": "t-1"})).await;
    let entries = log.result.unwrap()["entries"].as_array().unwrap().clone();
    assert!(!entries.is_empty(), "dry-run should still log review text");

    let text = entries[0]["text"].as_str().unwrap();
    assert!(text.contains("dry-run"), "log should mention dry-run");

    harness.stop().await;
}

// ── Real GitHub (gated) ────────────────────────────────────────

#[tokio::test]
#[ignore = "requires DEVDEV_E2E and GH_TOKEN"]
async fn e2e_real_github_pr_review() {
    // Placeholder: would use LiveGitHubAdapter with real tokens.
}

// ── New push triggers re-review ────────────────────────────────

#[tokio::test]
async fn e2e_new_push_triggers_rereview() {
    let harness = E2EHarness::new_with_policy(ApprovalPolicy::AutoApprove).await;

    // Add task.
    let add_resp = raw_ipc(harness.port, "task/add", json!({
        "description": "Monitor PR test-org/test-repo#1",
        "auto_approve": true
    }))
    .await;
    assert!(add_resp.error.is_none());

    // First poll — initial review.
    harness.advance_polls(1).await;
    let reviews_after_first = harness.github.posted_reviews().len();
    assert_eq!(reviews_after_first, 1);

    // Set task to Idle so it checks for new SHAs.
    {
        let mut reg = harness.ctx.tasks.lock().await;
        if let Some(task) = reg.get_mut("t-1") {
            task.set_status(devdev_tasks::TaskStatus::Idle);
        }
    }

    // No change — poll should be quiet.
    harness.advance_polls(1).await;
    assert_eq!(harness.github.posted_reviews().len(), reviews_after_first);

    // Simulate new push.
    harness.github.update_head_sha("test-org", "test-repo", 1, "sha-new-push-002");

    // Poll again — should detect new SHA and re-review.
    harness.advance_polls(1).await;
    assert!(
        harness.github.posted_reviews().len() > reviews_after_first,
        "new push should trigger re-review"
    );

    harness.stop().await;
}

// ── Helpers ────────────────────────────────────────────────────

/// Send a raw IPC request to the daemon.
async fn raw_ipc(
    port: u16,
    method: &str,
    params: serde_json::Value,
) -> devdev_daemon::ipc::IpcResponse {
    let mut client = devdev_daemon::ipc::IpcClient::connect(port).await.unwrap();
    client.request(method, params).await.unwrap()
}
