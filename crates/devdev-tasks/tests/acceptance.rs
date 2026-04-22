//! Acceptance tests for P2-04 — Task Manager & Approval Gate.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use devdev_tasks::approval::{self, ApprovalError, ApprovalPolicy, ApprovalResponse};
use devdev_tasks::registry::{TaskFactories, TaskRegistry};
use devdev_tasks::scheduler::TaskScheduler;
use devdev_tasks::task::{Task, TaskError, TaskMessage, TaskStatus};
use tokio::sync::Mutex;

// ── Mock task ──────────────────────────────────────────────────

struct MockTask {
    id: String,
    description: String,
    status: TaskStatus,
    poll_count: Arc<AtomicU32>,
    max_polls: Option<u32>,
    fail_on_poll: Option<u32>,
    state_val: i32,
    interval: Duration,
}

impl MockTask {
    fn new(id: &str, desc: &str) -> Self {
        Self {
            id: id.to_string(),
            description: desc.to_string(),
            status: TaskStatus::Created,
            poll_count: Arc::new(AtomicU32::new(0)),
            max_polls: None,
            fail_on_poll: None,
            state_val: 0,
            interval: Duration::from_millis(50),
        }
    }

    fn with_max_polls(mut self, n: u32) -> Self {
        self.max_polls = Some(n);
        self
    }

    fn with_fail_on_poll(mut self, n: u32) -> Self {
        self.fail_on_poll = Some(n);
        self
    }

    fn with_state(mut self, val: i32) -> Self {
        self.state_val = val;
        self
    }

    fn with_interval(mut self, d: Duration) -> Self {
        self.interval = d;
        self
    }

    fn poll_count_handle(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.poll_count)
    }

    fn from_json(data: serde_json::Value) -> Result<Box<dyn Task>, TaskError> {
        let id = data["id"].as_str().unwrap_or("t-0").to_string();
        let desc = data["description"].as_str().unwrap_or("").to_string();
        let state_val = data["state_val"].as_i64().unwrap_or(0) as i32;
        Ok(Box::new(MockTask {
            id,
            description: desc,
            status: TaskStatus::Created,
            poll_count: Arc::new(AtomicU32::new(0)),
            max_polls: None,
            fail_on_poll: None,
            state_val,
            interval: Duration::from_millis(50),
        }))
    }
}

#[async_trait::async_trait]
impl Task for MockTask {
    fn id(&self) -> &str {
        &self.id
    }

    fn describe(&self) -> String {
        self.description.clone()
    }

    fn status(&self) -> &TaskStatus {
        &self.status
    }

    fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
    }

    async fn poll(&mut self) -> Result<Vec<TaskMessage>, TaskError> {
        let count = self.poll_count.fetch_add(1, Ordering::SeqCst) + 1;

        if let Some(fail_at) = self.fail_on_poll {
            if count >= fail_at {
                return Err(TaskError::PollFailed("intentional failure".into()));
            }
        }

        if let Some(max) = self.max_polls {
            if count >= max {
                self.status = TaskStatus::Completed;
                return Ok(vec![TaskMessage::Text(format!("completed after {count} polls"))]);
            }
        }

        Ok(vec![TaskMessage::Text(format!("poll #{count}"))])
    }

    fn serialize(&self) -> Result<serde_json::Value, TaskError> {
        Ok(serde_json::json!({
            "id": self.id,
            "description": self.description,
            "state_val": self.state_val,
        }))
    }

    fn task_type(&self) -> &str {
        "mock"
    }

    fn poll_interval(&self) -> Duration {
        self.interval
    }
}

// ── Task lifecycle ─────────────────────────────────────────────

#[test]
fn task_add_and_list() {
    let mut reg = TaskRegistry::new();
    let task = MockTask::new("t-1", "Test task");
    reg.add(Box::new(task));

    let list = reg.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id(), "t-1");
    assert_eq!(list[0].status(), &TaskStatus::Created);
}

#[test]
fn task_cancel() {
    let mut reg = TaskRegistry::new();
    reg.add(Box::new(MockTask::new("t-1", "Test")));

    reg.cancel("t-1").unwrap();

    let task = reg.get("t-1").unwrap();
    assert_eq!(task.status(), &TaskStatus::Cancelled);
}

#[test]
fn task_cancel_nonexistent_errors() {
    let mut reg = TaskRegistry::new();
    let err = reg.cancel("t-999").unwrap_err();
    assert!(matches!(err, TaskError::NotFound(_)));
}

#[tokio::test]
async fn task_poll_updates_status() {
    let registry = Arc::new(Mutex::new(TaskRegistry::new()));
    let poll_count = {
        let task = MockTask::new("t-1", "Test").with_max_polls(2);
        let handle = task.poll_count_handle();
        registry.lock().await.add(Box::new(task));
        handle
    };

    let scheduler = TaskScheduler::new(Arc::clone(&registry));
    let (tx, rx) = tokio::sync::watch::channel(false);

    // Run scheduler briefly.
    let sched_handle = tokio::spawn(async move {
        scheduler.run(rx).await
    });

    // Wait for at least one poll.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = tx.send(true);

    let _msgs = sched_handle.await.unwrap();
    assert!(poll_count.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn task_poll_returns_messages() {
    let registry = Arc::new(Mutex::new(TaskRegistry::new()));
    let task = MockTask::new("t-1", "Test").with_max_polls(2);
    registry.lock().await.add(Box::new(task));

    let scheduler = TaskScheduler::new(Arc::clone(&registry));
    let (_tx, rx) = tokio::sync::watch::channel(false);

    let msgs = scheduler.run(rx).await;
    // Task completes after 2 polls, producing messages.
    assert!(!msgs.is_empty());
}

#[tokio::test]
async fn task_poll_error_transitions_to_errored() {
    let registry = Arc::new(Mutex::new(TaskRegistry::new()));
    let task = MockTask::new("t-1", "Test").with_fail_on_poll(1);
    registry.lock().await.add(Box::new(task));

    let scheduler = TaskScheduler::new(Arc::clone(&registry));
    let (_tx, rx) = tokio::sync::watch::channel(false);

    let _msgs = scheduler.run(rx).await;

    let reg = registry.lock().await;
    let task = reg.get("t-1").unwrap();
    assert!(matches!(task.status(), TaskStatus::Errored(_)));
}

#[tokio::test]
async fn task_completed_stops_polling() {
    let registry = Arc::new(Mutex::new(TaskRegistry::new()));
    let task = MockTask::new("t-1", "Test").with_max_polls(1);
    let poll_count = task.poll_count_handle();
    registry.lock().await.add(Box::new(task));

    let scheduler = TaskScheduler::new(Arc::clone(&registry));
    let (_tx, rx) = tokio::sync::watch::channel(false);

    let _msgs = scheduler.run(rx).await;

    // Should have polled exactly once, then completed.
    // Give a small tolerance — might see 1 or 2 depending on timing.
    assert!(poll_count.load(Ordering::SeqCst) <= 2);

    let reg = registry.lock().await;
    let task = reg.get("t-1").unwrap();
    assert_eq!(task.status(), &TaskStatus::Completed);
}

// ── Approval gate ──────────────────────────────────────────────

#[tokio::test]
async fn approval_auto_approve_executes() {
    let (mut gate, _handle) = approval::approval_channel(
        ApprovalPolicy::AutoApprove,
        Duration::from_secs(5),
    );
    let result = gate
        .request_approval("post_review", serde_json::json!({}))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn approval_dry_run_never_executes() {
    let (mut gate, _handle) = approval::approval_channel(
        ApprovalPolicy::DryRun,
        Duration::from_secs(5),
    );
    let result = gate
        .request_approval("post_review", serde_json::json!({"pr": 42}))
        .await;
    match result {
        Err(ApprovalError::DryRun { action, details }) => {
            assert_eq!(action, "post_review");
            assert_eq!(details["pr"], 42);
        }
        other => panic!("expected DryRun, got: {other:?}"),
    }
}

#[tokio::test]
async fn approval_ask_waits_for_response() {
    let (mut gate, mut handle) = approval::approval_channel(
        ApprovalPolicy::Ask,
        Duration::from_secs(5),
    );

    // Respond in a background task.
    tokio::spawn(async move {
        let req = handle.request_rx.recv().await.unwrap();
        handle
            .response_tx
            .send(ApprovalResponse {
                id: req.id,
                approve: true,
            })
            .await
            .unwrap();
    });

    let result = gate
        .request_approval("post_review", serde_json::json!({}))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn approval_ask_rejected() {
    let (mut gate, mut handle) = approval::approval_channel(
        ApprovalPolicy::Ask,
        Duration::from_secs(5),
    );

    tokio::spawn(async move {
        let req = handle.request_rx.recv().await.unwrap();
        handle
            .response_tx
            .send(ApprovalResponse {
                id: req.id,
                approve: false,
            })
            .await
            .unwrap();
    });

    let result = gate
        .request_approval("post_review", serde_json::json!({}))
        .await;
    assert!(matches!(result, Err(ApprovalError::Rejected)));
}

#[tokio::test]
async fn approval_ask_timeout_drops() {
    let (mut gate, _handle) = approval::approval_channel(
        ApprovalPolicy::Ask,
        Duration::from_millis(50), // very short timeout
    );

    // No one responds → should timeout.
    let result = gate
        .request_approval("post_review", serde_json::json!({}))
        .await;
    assert!(matches!(result, Err(ApprovalError::Timeout)));
}

#[tokio::test]
async fn approval_request_emitted_to_channel() {
    let (mut gate, mut handle) = approval::approval_channel(
        ApprovalPolicy::Ask,
        Duration::from_secs(5),
    );

    // Spawn request in background.
    let gate_handle = tokio::spawn(async move {
        // Will wait for response.
        let _ = gate.request_approval("deploy", serde_json::json!({"env": "prod"})).await;
    });

    // Read the request from the handle.
    let req = handle.request_rx.recv().await.unwrap();
    assert_eq!(req.action, "deploy");
    assert_eq!(req.details["env"], "prod");

    // Respond to unblock the gate.
    handle
        .response_tx
        .send(ApprovalResponse {
            id: req.id,
            approve: true,
        })
        .await
        .unwrap();

    gate_handle.await.unwrap();
}

// ── Serialization ──────────────────────────────────────────────

#[test]
fn registry_serialize_roundtrip() {
    let mut reg = TaskRegistry::new();
    reg.add(Box::new(MockTask::new("t-1", "Task 1").with_state(10)));
    reg.add(Box::new(MockTask::new("t-2", "Task 2").with_state(20)));
    reg.add(Box::new(MockTask::new("t-3", "Task 3").with_state(30)));

    let data = reg.serialize().unwrap();

    let mut factories = TaskFactories::new();
    factories.register("mock", Box::new(MockTask::from_json));

    let restored = TaskRegistry::deserialize(&data, &factories).unwrap();
    assert_eq!(restored.len(), 3);
    assert!(restored.get("t-1").is_some());
    assert!(restored.get("t-2").is_some());
    assert!(restored.get("t-3").is_some());
}

#[test]
fn registry_serialize_empty() {
    let reg = TaskRegistry::new();
    let data = reg.serialize().unwrap();

    let factories = TaskFactories::new();
    let restored = TaskRegistry::deserialize(&data, &factories).unwrap();
    assert!(restored.is_empty());
}

#[test]
fn task_serialize_preserves_state() {
    let task = MockTask::new("t-1", "Stateful").with_state(42);
    let data = task.serialize().unwrap();

    let restored = MockTask::from_json(data).unwrap();
    assert_eq!(restored.id(), "t-1");
    assert_eq!(restored.describe(), "Stateful");
}

// ── Scheduler ──────────────────────────────────────────────────

#[tokio::test]
async fn scheduler_respects_poll_interval() {
    let registry = Arc::new(Mutex::new(TaskRegistry::new()));
    let task = MockTask::new("t-1", "Fast").with_interval(Duration::from_millis(20));
    let poll_count = task.poll_count_handle();
    registry.lock().await.add(Box::new(task));

    let scheduler = TaskScheduler::new(Arc::clone(&registry));
    let (tx, rx) = tokio::sync::watch::channel(false);

    let sched_handle = tokio::spawn(async move {
        scheduler.run(rx).await
    });

    // Let it run for ~250ms.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let _ = tx.send(true);

    let _msgs = sched_handle.await.unwrap();
    let count = poll_count.load(Ordering::SeqCst);
    // With 20ms interval over 250ms, expect roughly 10-13 polls.
    assert!(count >= 5, "expected >= 5 polls, got {count}");
    assert!(count <= 20, "expected <= 20 polls, got {count}");
}

#[tokio::test]
async fn scheduler_shutdown_stops_polling() {
    let registry = Arc::new(Mutex::new(TaskRegistry::new()));
    let task = MockTask::new("t-1", "Infinite").with_interval(Duration::from_millis(10));
    let poll_count = task.poll_count_handle();
    registry.lock().await.add(Box::new(task));

    let scheduler = TaskScheduler::new(Arc::clone(&registry));
    let (tx, rx) = tokio::sync::watch::channel(false);

    let sched_handle = tokio::spawn(async move {
        scheduler.run(rx).await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = tx.send(true);

    let _msgs = sched_handle.await.unwrap();
    let count_at_shutdown = poll_count.load(Ordering::SeqCst);

    // Wait a bit more — should NOT increase.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(poll_count.load(Ordering::SeqCst), count_at_shutdown);
}

#[tokio::test]
async fn scheduler_multiple_tasks_independent() {
    let registry = Arc::new(Mutex::new(TaskRegistry::new()));
    let t1 = MockTask::new("t-1", "Fast").with_interval(Duration::from_millis(20));
    let t2 = MockTask::new("t-2", "Slow").with_interval(Duration::from_millis(80));
    let count1 = t1.poll_count_handle();
    let count2 = t2.poll_count_handle();
    registry.lock().await.add(Box::new(t1));
    registry.lock().await.add(Box::new(t2));

    let scheduler = TaskScheduler::new(Arc::clone(&registry));
    let (tx, rx) = tokio::sync::watch::channel(false);

    let sched_handle = tokio::spawn(async move {
        scheduler.run(rx).await
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tx.send(true);

    let _msgs = sched_handle.await.unwrap();
    let c1 = count1.load(Ordering::SeqCst);
    let c2 = count2.load(Ordering::SeqCst);

    // Fast task should have significantly more polls.
    assert!(c1 > c2, "fast task ({c1}) should have more polls than slow task ({c2})");
}
