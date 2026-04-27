//! E2E PR Shepherding tests — event-driven (Phase B2).
//!
//! Proves the daemon ↔ IPC ↔ dispatch ↔ MonitorPrTask ↔ EventBus
//! ↔ mock agent ↔ mock GitHub seam works.

use std::sync::Arc;
use std::time::Duration;

use devdev_daemon::dispatch::{DispatchContext, spawn_event_coordinator};
use devdev_daemon::ipc::IpcServer;
use devdev_daemon::ledger::NdjsonLedger;
use devdev_daemon::router::{
    AgentResponse, ResponseChunk, RouterError, SessionBackend, SessionRouter,
};
use devdev_daemon::{Daemon, DaemonConfig};
use devdev_integrations::{MockGitHubAdapter, PrState, PrStatus, PullRequest};
use devdev_tasks::approval::{self, ApprovalPolicy};
use devdev_tasks::events::{DaemonEvent, EventBus};
use devdev_tasks::ledger::IdempotencyLedger;
use devdev_tasks::registry::TaskRegistry;
use serde_json::json;
use tokio::sync::{Mutex, mpsc, watch};

// ── Fake Agent Backend ─────────────────────────────────────────

struct FakeAgentBackend {
    session_counter: std::sync::atomic::AtomicU64,
    prompts: std::sync::Mutex<Vec<String>>,
}

impl FakeAgentBackend {
    fn new() -> Self {
        Self {
            session_counter: std::sync::atomic::AtomicU64::new(1),
            prompts: std::sync::Mutex::new(Vec::new()),
        }
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
        self.prompts.lock().unwrap().push(text.to_string());
        Ok(AgentResponse {
            text: format!("Reviewed: {} chars", text.len()),
            stop_reason: "end_turn".to_string(),
        })
    }

    async fn send_prompt_streaming(
        &self,
        _session_id: &str,
        text: &str,
        tx: mpsc::Sender<ResponseChunk>,
    ) -> Result<(), RouterError> {
        self.prompts.lock().unwrap().push(text.to_string());
        let _ = tx.send(ResponseChunk::Text("Reviewed.".into())).await;
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

// ── E2E Harness ────────────────────────────────────────────────

struct E2EHarness {
    port: u16,
    ctx: Arc<DispatchContext>,
    bus: EventBus,
    backend: Arc<FakeAgentBackend>,
    shutdown_tx: watch::Sender<bool>,
    _server_handle: tokio::task::JoinHandle<()>,
    _coord_handle: tokio::task::JoinHandle<()>,
    _daemon: Daemon,
    _tmp: tempfile::TempDir,
}

impl E2EHarness {
    async fn new() -> Self {
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

        let backend = Arc::new(FakeAgentBackend::new());
        let backend_dyn: Arc<dyn SessionBackend> = backend.clone();
        let router = Arc::new(SessionRouter::new(backend_dyn));

        let registry = Arc::new(Mutex::new(TaskRegistry::new()));

        let policy = ApprovalPolicy::AutoApprove;
        let (gate, handle) = approval::approval_channel(policy, Duration::from_secs(30));
        let approval_gate = Arc::new(Mutex::new(gate));
        let approval_handle = Arc::new(Mutex::new(handle));
        let agent_secrets = Arc::new(Mutex::new(devdev_daemon::secrets::AgentSecrets::default()));

        let bus = EventBus::new();
        let ledger: Arc<dyn IdempotencyLedger> =
            Arc::new(NdjsonLedger::open(tmp.path().join("ledger.ndjson")).unwrap());

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let ctx = Arc::new(
            DispatchContext::new(
                Arc::clone(&router),
                Arc::clone(&registry),
                github,
                approval_gate,
                approval_handle,
                bus.clone(),
                ledger,
                policy,
                agent_secrets,
                shutdown_tx.clone(),
                Arc::new(Mutex::new(devdev_workspace::Fs::new())),
            )
            .with_approval_timeout(Duration::from_secs(2)),
        );

        let coord_handle = spawn_event_coordinator(Arc::clone(&ctx), shutdown_tx.subscribe());

        let server = IpcServer::bind().await.unwrap();
        let port = server.port();

        let ctx_clone = Arc::clone(&ctx);
        let server_handle = tokio::spawn(async move {
            devdev_daemon::server::run(ctx_clone, server, shutdown_rx).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        Self {
            port,
            ctx,
            bus,
            backend,
            shutdown_tx,
            _server_handle: server_handle,
            _coord_handle: coord_handle,
            _daemon: daemon,
            _tmp: tmp,
        }
    }

    async fn advance_polls(&self, count: usize) {
        for _ in 0..count {
            self.ctx.poll_all_tasks().await;
        }
    }

    async fn stop(self) {
        let _ = self.shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ── Scenarios ──────────────────────────────────────────────────

#[tokio::test]
async fn task_add_creates_idle_task_until_event_arrives() {
    let harness = E2EHarness::new().await;

    let add_resp = raw_ipc(
        harness.port,
        "task/add",
        json!({"description": "Monitor PR test-org/test-repo#1"}),
    )
    .await;
    assert!(
        add_resp.error.is_none(),
        "task/add failed: {:?}",
        add_resp.error
    );

    // No event yet → poll is quiet, agent untouched.
    harness.advance_polls(1).await;
    assert!(harness.backend.prompts.lock().unwrap().is_empty());

    harness.stop().await;
}

#[tokio::test]
async fn pr_opened_event_drives_agent_prompt() {
    let harness = E2EHarness::new().await;

    raw_ipc(
        harness.port,
        "task/add",
        json!({"description": "Monitor PR test-org/test-repo#1"}),
    )
    .await;

    harness.bus.publish(DaemonEvent::PrOpened {
        owner: "test-org".into(),
        repo: "test-repo".into(),
        number: 1,
        head_sha: "sha-initial-001".into(),
    });

    harness.advance_polls(1).await;

    let prompts = harness.backend.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1, "agent should be prompted exactly once");
    assert!(prompts[0].contains("opened"));
    assert!(prompts[0].contains("test-org/test-repo#1"));

    // Task log should reflect the agent reply.
    let log = raw_ipc(harness.port, "task/log", json!({"task_id": "t-1"})).await;
    let entries = log.result.unwrap()["entries"].as_array().unwrap().clone();
    assert!(!entries.is_empty(), "task log should have entries");

    harness.stop().await;
}

#[tokio::test]
async fn pr_closed_event_completes_task() {
    let harness = E2EHarness::new().await;

    raw_ipc(
        harness.port,
        "task/add",
        json!({"description": "Monitor PR test-org/test-repo#1"}),
    )
    .await;

    harness.bus.publish(DaemonEvent::PrClosed {
        owner: "test-org".into(),
        repo: "test-repo".into(),
        number: 1,
        merged: true,
    });

    harness.advance_polls(1).await;

    let status = raw_ipc(harness.port, "status", json!({})).await;
    let task_count = status.result.unwrap()["tasks"].as_u64().unwrap();
    assert!(task_count >= 1);

    // Task should be Completed; agent never invoked.
    assert!(harness.backend.prompts.lock().unwrap().is_empty());

    harness.stop().await;
}

#[tokio::test]
async fn pr_updated_event_reprompts_agent() {
    let harness = E2EHarness::new().await;

    raw_ipc(
        harness.port,
        "task/add",
        json!({"description": "Monitor PR test-org/test-repo#1"}),
    )
    .await;

    harness.bus.publish(DaemonEvent::PrOpened {
        owner: "test-org".into(),
        repo: "test-repo".into(),
        number: 1,
        head_sha: "sha-initial-001".into(),
    });
    harness.advance_polls(1).await;

    harness.bus.publish(DaemonEvent::PrUpdated {
        owner: "test-org".into(),
        repo: "test-repo".into(),
        number: 1,
        head_sha: "sha-second-002".into(),
    });
    harness.advance_polls(1).await;

    let prompts = harness.backend.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 2, "second push should re-prompt");
    assert!(prompts[1].contains("updated"));

    harness.stop().await;
}

#[tokio::test]
async fn unrelated_pr_event_is_ignored() {
    let harness = E2EHarness::new().await;

    raw_ipc(
        harness.port,
        "task/add",
        json!({"description": "Monitor PR test-org/test-repo#1"}),
    )
    .await;

    // Different number.
    harness.bus.publish(DaemonEvent::PrOpened {
        owner: "test-org".into(),
        repo: "test-repo".into(),
        number: 2,
        head_sha: "x".into(),
    });

    harness.advance_polls(1).await;
    assert!(harness.backend.prompts.lock().unwrap().is_empty());

    harness.stop().await;
}

#[tokio::test]
async fn repo_watch_ipc_spawns_repo_watch_task() {
    let harness = E2EHarness::new().await;

    let resp = raw_ipc(
        harness.port,
        "repo/watch",
        json!({ "owner": "test-org", "repo": "test-repo", "poll_interval_secs": 1 }),
    )
    .await;
    assert!(resp.error.is_none(), "repo/watch failed: {:?}", resp.error);
    let r = resp.result.unwrap();
    assert!(r["task_id"].as_str().unwrap().starts_with("t-"));
    assert_eq!(r["already_watching"], false);

    // Idempotent re-watch.
    let resp2 = raw_ipc(
        harness.port,
        "repo/watch",
        json!({ "owner": "test-org", "repo": "test-repo" }),
    )
    .await;
    let r2 = resp2.result.unwrap();
    assert_eq!(r2["already_watching"], true);

    // Polling RepoWatchTask publishes a PrOpened (via the mock
    // adapter's existing PR), which the coordinator turns into a
    // MonitorPrTask. Since polls + bus delivery + coordinator are
    // async, give the system time to settle.
    harness.advance_polls(1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    harness.advance_polls(1).await;

    let prompts = harness.backend.prompts.lock().unwrap().clone();
    assert!(
        !prompts.is_empty(),
        "coordinator should have spawned a MonitorPrTask that prompted the agent"
    );

    harness.stop().await;
}

#[tokio::test]
async fn repo_unwatch_ipc_cancels_task() {
    let harness = E2EHarness::new().await;

    raw_ipc(
        harness.port,
        "repo/watch",
        json!({ "owner": "test-org", "repo": "test-repo" }),
    )
    .await;

    let resp = raw_ipc(
        harness.port,
        "repo/unwatch",
        json!({ "owner": "test-org", "repo": "test-repo" }),
    )
    .await;
    assert!(resp.error.is_none());

    // Re-unwatch should now error.
    let resp2 = raw_ipc(
        harness.port,
        "repo/unwatch",
        json!({ "owner": "test-org", "repo": "test-repo" }),
    )
    .await;
    assert!(resp2.error.is_some());

    harness.stop().await;
}

#[tokio::test]
async fn checkpoint_recovery_round_trips_fs() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

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
    }
}

// ── Helpers ────────────────────────────────────────────────────

async fn raw_ipc(
    port: u16,
    method: &str,
    params: serde_json::Value,
) -> devdev_daemon::ipc::IpcResponse {
    let mut client = devdev_daemon::ipc::IpcClient::connect(port).await.unwrap();
    client.request(method, params).await.unwrap()
}
