---
id: task-manager
title: "Task Manager & Approval Gate"
status: done
type: composition
phase: 2
crate: devdev-tasks
priority: P0
depends-on: [daemon-lifecycle]
effort: L
---

# P2-04 — Task Manager & Approval Gate

**New crate: `devdev-tasks`.** Manages long-lived background work. A task is a unit of ongoing activity ("monitor this PR", "watch for dependency updates") that polls on a schedule, reacts to changes, and produces output for the user.

## Scope

**In:**
- `Task` trait: id, description, poll, serialize/deserialize.
- `TaskRegistry`: stores active tasks, tracks state, drives scheduling.
- `TaskScheduler`: calls `Task::poll()` on interval. Fixed interval for Phase 2 (configurable per-task, default 30s).
- `TaskContext`: gives tasks access to sandbox (VFS + shell + tools + git), agent (ACP session), and integration adapters.
- `ApprovalGate`: intercepts external actions, applies policy (ask / auto-approve / dry-run).
- Task lifecycle: created → polling → idle → completed / cancelled / errored.
- Task persistence: `TaskRegistry::serialize()` / `deserialize()` for checkpoint integration.
- `TaskMessage`: output from a task poll — text, approval request, status change.

**Out:**
- Specific task implementations (MonitorPR is P2-07).
- Adaptive polling / backoff (Phase 3 optimization).
- Event-driven / webhook triggers (Phase 3).
- Task priorities / dependency ordering between tasks.

## Interface

### Task Trait

```rust
/// A long-lived unit of background work.
#[async_trait]
pub trait Task: Send + Sync {
    /// Unique identifier for this task instance (e.g., "t-1").
    fn id(&self) -> &str;

    /// Human-readable description (e.g., "Monitoring PR #247 in org/repo").
    fn describe(&self) -> String;

    /// Current status.
    fn status(&self) -> TaskStatus;

    /// Called on schedule. Inspect state, optionally invoke agent, produce messages.
    async fn poll(&mut self, ctx: &mut TaskContext) -> Result<Vec<TaskMessage>, TaskError>;

    /// Serialize task state for checkpoint.
    fn serialize(&self) -> Result<serde_json::Value, TaskError>;

    /// Requested polling interval. Scheduler honors this.
    fn poll_interval(&self) -> Duration;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Created,
    Polling,
    Idle,
    Completed,
    Cancelled,
    Errored(String),
}
```

### Task Registry

```rust
pub struct TaskRegistry {
    tasks: HashMap<String, Box<dyn Task>>,
    next_id: u64,
}

impl TaskRegistry {
    pub fn new() -> Self;

    /// Add a task, return its ID.
    pub fn add(&mut self, task: Box<dyn Task>) -> String;

    /// Cancel a task by ID.
    pub fn cancel(&mut self, id: &str) -> Result<(), TaskError>;

    /// Get task by ID.
    pub fn get(&self, id: &str) -> Option<&dyn Task>;

    /// List all tasks.
    pub fn list(&self) -> Vec<&dyn Task>;

    /// Serialize all tasks for checkpoint.
    pub fn serialize(&self) -> Result<serde_json::Value, TaskError>;

    /// Deserialize tasks from checkpoint.
    pub fn deserialize(data: &serde_json::Value, factories: &TaskFactories) -> Result<Self, TaskError>;
}
```

### Task Context

```rust
/// Provided to Task::poll(). Gives access to the sandbox and integrations.
pub struct TaskContext {
    pub vfs: Arc<Mutex<MemFs>>,
    pub shell: ShellWorker,
    pub tools: Arc<dyn ToolEngine>,
    pub git: Arc<Mutex<dyn VirtualGit>>,
    // pub agent: SessionHandle,          // wired in P2-06
    // pub github: Arc<dyn GitHubAdapter>, // wired in P2-05
    pub approval: ApprovalGate,
}
```

### Approval Gate

```rust
#[derive(Debug, Clone, Copy)]
pub enum ApprovalPolicy {
    /// Queue the action, emit approval request, wait for response.
    Ask,
    /// Execute immediately, log the action.
    AutoApprove,
    /// Log what would happen, never execute.
    DryRun,
}

pub struct ApprovalGate {
    policy: ApprovalPolicy,
    timeout: Duration,                       // for Ask mode in headless
    sender: mpsc::Sender<ApprovalRequest>,   // sends to TUI/headless
    receiver: mpsc::Receiver<ApprovalResponse>, // receives from TUI/headless
}

pub struct ApprovalRequest {
    pub id: String,
    pub action: String,            // e.g., "post_review"
    pub details: serde_json::Value,
}

pub struct ApprovalResponse {
    pub id: String,
    pub approve: bool,
}

impl ApprovalGate {
    /// Request approval for an external action.
    /// - Ask: sends request, waits for response (or timeout → drop).
    /// - AutoApprove: returns Ok immediately.
    /// - DryRun: logs the action, returns Err(DryRun).
    pub async fn request_approval(&self, action: &str, details: serde_json::Value) -> Result<(), ApprovalError>;
}

pub enum ApprovalError {
    Rejected,
    Timeout,
    DryRun { action: String, details: serde_json::Value },
}
```

### Scheduler

```rust
pub struct TaskScheduler {
    registry: Arc<Mutex<TaskRegistry>>,
    context_factory: Arc<dyn Fn(&str) -> TaskContext + Send + Sync>,
}

impl TaskScheduler {
    /// Run the scheduling loop. Calls poll() on each task at its requested interval.
    /// Returns when all tasks are completed/cancelled or shutdown is signaled.
    pub async fn run(&self, shutdown: tokio::sync::watch::Receiver<bool>) -> Result<(), TaskError>;
}
```

## Implementation Notes

- **Scheduling:** Simple `tokio::time::interval` per task. Each task gets its own tokio task (`tokio::spawn`). The scheduler monitors them and collects messages.
- **Approval channel:** `mpsc::channel` pair connecting the approval gate to the TUI/headless event loop. The daemon wires these together on startup.
- **Timeout for Ask:** In headless mode, if no approval response arrives within `timeout` (default 60s), the action is dropped. In TUI mode, the prompt stays visible until the user responds (no timeout — they're at the keyboard).
- **Task serialization:** Each `Task` impl serializes its own state as JSON. The registry wraps each task's JSON with `{"type": "monitor_pr", "state": {...}}` so deserialization knows which factory to call.
- **TaskFactories:** A registry of `fn(serde_json::Value) -> Box<dyn Task>` keyed by task type string. Used during checkpoint restore.
- **Cancellation:** Sets `TaskStatus::Cancelled` and drops the task's tokio task. The next poll loop iteration skips it.
- **Error handling:** If `poll()` returns `Err`, the task transitions to `Errored`. The scheduler logs the error and stops polling. The user can see the error via `devdev task list`.

## Files

```
crates/devdev-tasks/Cargo.toml
crates/devdev-tasks/src/lib.rs          — re-exports
crates/devdev-tasks/src/task.rs         — Task trait, TaskStatus, TaskMessage, TaskError
crates/devdev-tasks/src/registry.rs     — TaskRegistry, task storage, serialization
crates/devdev-tasks/src/scheduler.rs    — TaskScheduler, polling loop
crates/devdev-tasks/src/approval.rs     — ApprovalGate, ApprovalPolicy, ApprovalRequest/Response
crates/devdev-tasks/src/context.rs      — TaskContext
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-04-1 | §3.3 | Task trait: id, describe, poll, serialize |
| SR-04-2 | §3.3 | Task registry: track active tasks, state, polling intervals |
| SR-04-3 | §3.3 | Scheduler: drive polling loops |
| SR-04-4 | §3.3 | Task persistence: serialize into checkpoint |
| SR-04-5 | §3.3 (approval policy) | Approval gate: ask / auto-approve / dry-run |
| SR-04-6 | §3.3 (approval policy) | Headless approval: NDJSON request/response, timeout → drop |
| SR-04-7 | §4 (Approval gate row) | External actions only fire when approved or auto-approved |
| SR-04-8 | §4 (Approval gate row) | Dry-run never mutates |
| SR-04-9 | §4 (Approval gate row) | Timeout is fail-safe (drop, not auto-approve) |
| SR-04-10 | §4 (Task Manager row) | Tasks poll correctly, produce expected messages, serialize/deserialize |

## Acceptance Tests

### Task Lifecycle

- [ ] `task_add_and_list` — add a mock task → `list()` returns it with `Created` status
- [ ] `task_cancel` — add task, cancel by ID → status becomes `Cancelled`, poll not called again
- [ ] `task_cancel_nonexistent_errors` — cancel unknown ID → error
- [ ] `task_poll_updates_status` — scheduler polls mock task → status transitions to `Polling`
- [ ] `task_poll_returns_messages` — mock task returns `TaskMessage` → scheduler collects them
- [ ] `task_poll_error_transitions_to_errored` — mock task returns Err → `Errored` status
- [ ] `task_completed_stops_polling` — mock task sets Completed → scheduler stops calling poll

### Approval Gate

- [ ] `approval_auto_approve_executes` — policy=AutoApprove → `request_approval` returns Ok immediately
- [ ] `approval_dry_run_never_executes` — policy=DryRun → returns `Err(DryRun)` with details
- [ ] `approval_ask_waits_for_response` — policy=Ask → blocks until response, returns Ok on approve
- [ ] `approval_ask_rejected` — policy=Ask → response with approve=false → returns `Err(Rejected)`
- [ ] `approval_ask_timeout_drops` — policy=Ask, no response within timeout → returns `Err(Timeout)`
- [ ] `approval_request_emitted_to_channel` — verify ApprovalRequest appears on the sender channel

### Serialization

- [ ] `registry_serialize_roundtrip` — add 3 mock tasks → serialize → deserialize → 3 tasks present with same state
- [ ] `registry_serialize_empty` — serialize empty registry → deserialize → empty registry
- [ ] `task_serialize_preserves_state` — mock task with internal state → serialize → deserialize → state matches

### Scheduler

- [ ] `scheduler_respects_poll_interval` — task with 100ms interval → verify ~10 polls in 1 second
- [ ] `scheduler_shutdown_stops_polling` — signal shutdown → scheduler exits, no more polls
- [ ] `scheduler_multiple_tasks_independent` — two tasks with different intervals → both poll correctly

## Spec Compliance Checklist

- [ ] SR-04-1 through SR-04-10: all requirements covered
- [ ] All acceptance tests passing
