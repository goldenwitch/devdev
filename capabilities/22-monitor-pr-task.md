---
id: monitor-pr-task
title: "MonitorPR Task"
status: not-started
type: composition
phase: 2
crate: devdev-tasks
priority: P0
depends-on: [task-manager, github-adapter, session-router]
effort: L
---

# P2-07 — MonitorPR Task

The first real task implementation. Monitors a single GitHub PR: loads the repo into VFS, fetches the diff, has the agent review it, drafts review comments, and watches for new pushes. This is the minimum viable product for DevDev Phase 2.

## Scope

**In:**
- `MonitorPrTask` implementing `Task` trait.
- On creation: parse PR reference (owner/repo#number or URL), load repo into VFS, fetch PR diff.
- On first poll: send diff + codebase to agent, collect review, produce `TaskMessage` with review text.
- On subsequent polls: check `get_pr_head_sha` for new commits. If SHA changed, fetch updated diff, re-review, notify user.
- Draft review comments: agent produces structured review → task creates `Review` struct with inline comments.
- External action: posting review goes through ApprovalGate. User must approve (or task uses `--auto-approve`).
- PR closed/merged: task detects via `get_pr_status` → transitions to `Completed`.
- Serialization: persist owner, repo, PR number, last-seen SHA, accumulated observations for checkpoint.

**Out:**
- Monitoring multiple PRs (that's multiple MonitorPrTask instances).
- Creating PRs, pushing code, or making changes to the repo.
- CI integration (reading check results is in scope; re-running checks is not).
- Review threading (responding to comment threads — future feature).

## Interface

```rust
pub struct MonitorPrTask {
    id: String,
    owner: String,
    repo: String,
    pr_number: u64,
    status: TaskStatus,
    last_sha: Option<String>,
    observations: Vec<String>,   // accumulated review history
    poll_interval: Duration,
    approval_policy: ApprovalPolicy,
}

impl MonitorPrTask {
    /// Create from a PR reference like "org/repo#247" or a full URL.
    pub fn new(id: String, pr_ref: &str, policy: ApprovalPolicy) -> Result<Self, TaskError>;
}
```

### Task Lifecycle (State Machine)

```
                     ┌──────────┐
                     │ Created  │
                     └─────┬────┘
                           │ first poll
                     ┌─────▼────┐
              ┌──────│ Loading  │  load repo into VFS, fetch PR
              │      └─────┬────┘
              │            │ success
              │      ┌─────▼────┐
              │      │Reviewing │  agent reviews diff
              │      └─────┬────┘
              │            │ review complete
              │      ┌─────▼────┐
              │      │  Idle    │◄─────────────────┐
              │      └─────┬────┘                  │
              │            │ poll detects change   │
              │      ┌─────▼────────┐              │
              │      │ Re-reviewing │──────────────┘
              │      └─────┬────────┘  no change → back to idle
              │            │ PR merged/closed
              │      ┌─────▼──────┐
              │      │ Completed  │
              │      └────────────┘
              │ error at any point
              │      ┌────────────┐
              └──────│  Errored   │
                     └────────────┘
```

### Poll Behavior

```rust
impl Task for MonitorPrTask {
    async fn poll(&mut self, ctx: &mut TaskContext) -> Result<Vec<TaskMessage>, TaskError> {
        match self.status {
            TaskStatus::Created => {
                // 1. Load repo into VFS at /repos/{owner}/{repo}/
                // 2. Fetch PR diff via GitHub adapter
                // 3. Send to agent: system prompt + diff + "review this PR"
                // 4. Collect review
                // 5. If review has actionable comments:
                //    - Build Review struct
                //    - Go through ApprovalGate
                //    - If approved: post via GitHub adapter
                // 6. Store last_sha, transition to Idle
                // Return: TaskMessage with review text
            }
            TaskStatus::Idle => {
                // 1. Check get_pr_head_sha — same as last_sha?
                //    - Same → return empty vec (nothing to report)
                //    - Different → fetch new diff, re-review
                // 2. Check get_pr_status — merged/closed?
                //    - Yes → transition to Completed
                // Return: TaskMessage if new review, empty if no change
            }
            // ...
        }
    }
}
```

### Agent Prompt Construction

The agent receives:

1. **System prompt:** "You are reviewing a pull request. The codebase is loaded in the sandbox under /repos/{owner}/{repo}/. The PR diff is provided below. Use the sandbox tools to explore the code. Produce a structured review."
2. **PR context:** title, author, description, base branch.
3. **Diff:** unified diff from GitHub adapter.
4. **Prior observations:** (on re-review) "Previously you noted: [prior review text]. The author has pushed new commits. Review the changes."

### Review Parsing

The agent's response is free-form text. To create structured `ReviewComment` objects:

1. The prompt asks the agent to format comments as `[file:line] comment text`.
2. The task parses these out of the response.
3. Unparseable text becomes the review body.
4. If parsing fails completely, post the entire response as a review body (no inline comments).

This is fragile and will need iteration. The important thing is it works end-to-end, not that parsing is perfect.

## Implementation Notes

- **Repo loading:** On first poll, call `vfs.mount("/repos/{owner}/{repo}/", &host_path)` (P2-01). But wait — where is the host repo? Two options:
  1. Clone the repo to a temp dir, then load into VFS. (Works but slow.)
  2. Fetch only the diff from GitHub, load base branch into VFS from a local clone. (Requires the user to have the repo locally.)
  For v1: require the user to have the repo cloned locally. The task resolves the local path from a config or convention. If not found, error with "clone the repo first." **This is a known UX paper cut** that Phase 3 will fix with server-side cloning.
- **Diff application:** The agent doesn't need the diff applied to the VFS. It reads the base code from VFS and the diff as text. It can `cat` files and cross-reference with the diff.
- **Observations accumulation:** Each review's key findings are appended to `self.observations`. On re-review, the agent sees what it said before. This prevents "I already told you about this" loops.
- **Poll interval:** Default 60s for PR monitoring. Configurable via constructor.

## Files

```
crates/devdev-tasks/src/monitor_pr.rs   — MonitorPrTask implementation
crates/devdev-tasks/src/pr_ref.rs       — Parse "org/repo#247" or URL into (owner, repo, number)
crates/devdev-tasks/src/review.rs       — Review parsing from agent response
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-07-1 | §3.3 | First task implementation: MonitorPrTask |
| SR-07-2 | §4 Scenario A | Load repo, fetch diff, create session, review, ask to post |
| SR-07-3 | §4 Scenario A | Detect new push, re-review, notify user |
| SR-07-4 | §4 Scenario B | Headless: auto-approve posts review without user interaction |
| SR-07-5 | §4 Scenario A | PR merged/closed → task completes |
| SR-07-6 | §4 (Task Manager row) | Given no changes, stays quiet |
| SR-07-7 | §4 (Task Manager row) | Serialize/deserialize for checkpoint |

## Acceptance Tests

### Unit (with MockGitHubAdapter + fake agent)

- [ ] `parse_pr_ref_from_shorthand` — "org/repo#247" → (org, repo, 247)
- [ ] `parse_pr_ref_from_url` — "https://github.com/org/repo/pull/247" → (org, repo, 247)
- [ ] `parse_pr_ref_invalid_errors` — "not_a_ref" → error
- [ ] `first_poll_loads_and_reviews` — mock PR + diff + fake agent → TaskMessage with review text
- [ ] `first_poll_posts_review_when_approved` — approval=AutoApprove → mock adapter's `posted_reviews()` non-empty
- [ ] `first_poll_skips_post_when_rejected` — approval=Ask, respond with reject → no posted reviews
- [ ] `subsequent_poll_no_change_quiet` — same SHA → empty messages
- [ ] `subsequent_poll_new_sha_re_reviews` — different SHA → new TaskMessage with updated review
- [ ] `pr_merged_transitions_to_completed` — mock status=Merged → TaskStatus::Completed
- [ ] `pr_closed_transitions_to_completed` — mock status=Closed → TaskStatus::Completed
- [ ] `observations_accumulate` — two reviews → second prompt includes first review's findings
- [ ] `serialize_deserialize_roundtrip` — create task with state → serialize → deserialize → same state
- [ ] `dry_run_never_posts` — approval=DryRun → no posted reviews, log shows what would happen

### Review Parsing

- [ ] `parse_structured_review` — agent responds with `[src/config.rs:42] bad validation` → ReviewComment extracted
- [ ] `parse_fallback_body_only` — agent responds with unstructured text → review body, no inline comments
- [ ] `parse_mixed` — some structured, some not → both inline comments and body text

## Spec Compliance Checklist

- [ ] SR-07-1 through SR-07-7: all requirements covered
- [ ] All acceptance tests passing
