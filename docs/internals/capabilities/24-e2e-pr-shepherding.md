---
id: e2e-pr-shepherding
title: "E2E PR Shepherding"
status: in-progress
type: composition
phase: 2
crate: tests
priority: P0
depends-on: [monitor-pr-task]
effort: L
---

# P2-09 — E2E PR Shepherding

The full end-to-end validation. Three scenarios exercised: interactive (TUI), headless (CI/scripting), and one-shot. This is not a feature — it's a test suite that proves the entire stack works together. If this passes, Phase 2 ships.

## Scope

**In:**
- **Scenario A (Interactive):** daemon → TUI → user says "monitor PR" → agent reviews → user approves → review posted → new push detected → re-review → daemon down with checkpoint.
- **Scenario B (Headless):** daemon → `devdev task add --auto-approve "Monitor PR #247"` → review posted automatically → `devdev status --json` → `devdev task log` → daemon down.
- **Scenario C (One-shot):** `devdev up && devdev send --auto-approve --json "Review PR #247" && devdev down` → structured JSON result.
- **Deterministic tests:** MockGitHubAdapter + duplex-based fake agent. No network, no tokens, fast. Can run in CI.
- **E2E tests:** Real GitHub API + fake agent (or real Copilot if available). Gated behind `DEVDEV_E2E`.
- **Checkpoint recovery:** Scenario A extended: daemon down → daemon up --checkpoint → verify task resumes, VFS intact.

**Out:**
- Performance benchmarks (separate concern).
- Multi-PR / multi-repo scenarios (future — P2-09 tests one PR).
- Real Copilot agent testing (use fake agent for determinism; real agent is manual QA).

## Test Infrastructure

### Fake Agent (Extended)

The existing `devdev-fake-agent` binary handles single-turn ACP. Extend it for multi-turn:

```rust
/// Scripted fake agent that responds to a sequence of prompts.
struct MultiTurnFakeAgent {
    responses: VecDeque<FakeResponse>,
}

struct FakeResponse {
    /// Match prompt by substring.
    prompt_contains: String,
    /// Agent response text.
    text: String,
    /// Optional tool calls the agent "makes" (to exercise sandbox).
    tool_calls: Vec<FakeTool>,
}
```

The fake agent:
1. Receives `session/new` → responds with session.
2. Receives a prompt containing "review this PR" → responds with a canned review including structured `[file:line]` comments.
3. Receives a prompt containing "new commits" → responds with an updated review.
4. Handles `session/destroy` cleanly.

### Mock GitHub Adapter

Already defined in P2-05. Configure it with:
- A test PR (owner=test-org, repo=test-repo, number=1).
- An initial diff.
- An updated head SHA (to simulate a new push).
- Verification: `posted_reviews()` captures what was posted.

### Test Harness

```rust
struct E2EHarness {
    daemon: Daemon,
    github: Arc<MockGitHubAdapter>,
    agent_transport: Transport,  // duplex to fake agent
}

impl E2EHarness {
    /// Set up daemon with mock everything.
    async fn new() -> Self;
    /// Get an IPC client connected to the daemon.
    async fn connect(&self) -> DaemonConnection;
    /// Advance time to trigger task polls.
    async fn advance_polls(&self, count: usize);
    /// Stop daemon and return checkpoint data.
    async fn stop(self) -> Vec<u8>;
}
```

## Test Scenarios

### Scenario A: Interactive (Deterministic)

```rust
#[tokio::test]
async fn e2e_interactive_pr_monitoring() {
    let harness = E2EHarness::new().await;
    let mut conn = harness.connect().await;
    conn.attach().await.unwrap();

    // User says "monitor PR"
    conn.send_message("Monitor PR #1 in test-org/test-repo").await.unwrap();

    // Agent reviews (via fake agent)
    let events = collect_until_done(&mut conn).await;
    assert!(events.iter().any(|e| matches!(e, DaemonEvent::AgentDone { .. })));

    // Approval request appears
    let approval = events.iter().find(|e| matches!(e, DaemonEvent::ApprovalRequest { .. }));
    assert!(approval.is_some());

    // User approves
    conn.send_approval(true).await.unwrap();

    // Verify review was posted
    assert_eq!(harness.github.posted_reviews().len(), 1);

    // Simulate new push (change head SHA in mock)
    harness.github.update_head_sha("test-org", "test-repo", 1, "new-sha-456");

    // Advance polls to detect new push
    harness.advance_polls(1).await;

    // New review appears
    let events = collect_until_done(&mut conn).await;
    assert!(events.iter().any(|e| matches!(e, DaemonEvent::AgentDone { .. })));

    // Stop with checkpoint
    let checkpoint = harness.stop().await;
    assert!(!checkpoint.is_empty());
}
```

### Scenario B: Headless (Deterministic)

```rust
#[tokio::test]
async fn e2e_headless_auto_approve() {
    let harness = E2EHarness::new().await;
    let mut conn = harness.connect().await;

    // Create task via IPC (not TUI)
    let resp = conn.send_ipc(json!({
        "method": "task/add",
        "params": {
            "description": "Monitor PR #1 in test-org/test-repo",
            "auto_approve": true
        }
    })).await.unwrap();
    let task_id = resp["result"]["task_id"].as_str().unwrap();

    // Wait for task to poll and review
    harness.advance_polls(1).await;

    // Review was posted automatically (no approval wait)
    assert_eq!(harness.github.posted_reviews().len(), 1);

    // Status shows task
    let status = conn.send_ipc(json!({"method": "status"})).await.unwrap();
    assert!(status["result"]["tasks"].as_u64().unwrap() >= 1);

    // Task log has the review
    let log = conn.send_ipc(json!({
        "method": "task/log",
        "params": {"task_id": task_id}
    })).await.unwrap();
    assert!(!log["result"]["entries"].as_array().unwrap().is_empty());

    harness.stop().await;
}
```

### Scenario C: One-Shot (Deterministic)

```rust
#[tokio::test]
async fn e2e_one_shot_review() {
    let harness = E2EHarness::new().await;
    let mut conn = harness.connect().await;

    // One-shot send
    let resp = conn.send_ipc(json!({
        "method": "send",
        "params": {
            "text": "Review PR #1 in test-org/test-repo",
            "auto_approve": true
        }
    })).await.unwrap();

    // Response contains review text
    let response_text = resp["result"]["response"].as_str().unwrap();
    assert!(!response_text.is_empty());

    harness.stop().await;
}
```

### Checkpoint Recovery

```rust
#[tokio::test]
async fn e2e_checkpoint_recovery() {
    // Phase 1: start, create task, stop with checkpoint
    let harness = E2EHarness::new().await;
    let mut conn = harness.connect().await;
    conn.send_ipc(json!({
        "method": "task/add",
        "params": {"description": "Monitor PR #1 in test-org/test-repo", "auto_approve": true}
    })).await.unwrap();
    harness.advance_polls(1).await;
    let checkpoint = harness.stop().await;

    // Phase 2: restart from checkpoint, verify task resumes
    let harness2 = E2EHarness::from_checkpoint(checkpoint).await;
    let mut conn2 = harness2.connect().await;
    let status = conn2.send_ipc(json!({"method": "status"})).await.unwrap();
    assert!(status["result"]["tasks"].as_u64().unwrap() >= 1);

    // Simulate new push and verify re-review works
    harness2.github.update_head_sha("test-org", "test-repo", 1, "new-sha-789");
    harness2.advance_polls(1).await;
    assert!(harness2.github.posted_reviews().len() >= 2); // original + re-review

    harness2.stop().await;
}
```

### E2E (Real GitHub, gated)

```rust
#[tokio::test]
#[ignore = "requires DEVDEV_E2E and GH_TOKEN"]
async fn e2e_real_github_pr_review() {
    // Uses a known test repo with a test PR
    // Exercises the real GitHub adapter
    // Uses fake agent for determinism
    // Posts a real review comment, then cleans it up
}
```

## Files

```
tests/e2e_pr_shepherding.rs          — all scenarios above
tests/harness.rs                     — E2EHarness setup
crates/devdev-cli/src/bin/devdev-fake-agent.rs  — extended for multi-turn
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-09-1 | §4 Scenario A | Interactive: TUI → monitor → review → approve → re-review → checkpoint |
| SR-09-2 | §4 Scenario B | Headless: auto-approve → review posted → status/log → down |
| SR-09-3 | §4 Scenario C | One-shot: send → JSON response → exit |
| SR-09-4 | §4 E2E test | Scripted Scenario B with test repo, gated behind DEVDEV_E2E |
| SR-09-5 | §4 Deterministic test | Mock adapter + fake agent, exercises TUI + headless paths |
| SR-09-6 | §4 | Checkpoint: stop → restart → task resumes with VFS intact |

## Acceptance Tests

- [ ] `e2e_interactive_pr_monitoring` — Scenario A, full flow, deterministic
- [ ] `e2e_headless_auto_approve` — Scenario B, auto-approve, deterministic
- [ ] `e2e_one_shot_review` — Scenario C, send + response, deterministic
- [ ] `e2e_checkpoint_recovery` — checkpoint → restart → task resumes
- [ ] `e2e_headless_approval_protocol` — approval request on stdout, approval response on stdin
- [ ] `e2e_dry_run_no_side_effects` — `--dry-run` → review text produced but nothing posted
- [ ] `e2e_real_github_pr_review` — (DEVDEV_E2E) real API, fake agent, post + cleanup
- [ ] `e2e_tui_rendering` — (via ratatui test backend) verify TUI renders messages correctly during Scenario A

## Spec Compliance Checklist

- [ ] SR-09-1 through SR-09-6: all requirements covered
- [ ] All deterministic tests passing
- [ ] E2E tests passing when DEVDEV_E2E is set
- [ ] Phase 2 ships
