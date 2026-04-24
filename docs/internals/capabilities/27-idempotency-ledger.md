---
id: idempotency-ledger
title: "Idempotency Ledger"
status: not-started
type: leaf
phase: 2
crate: devdev-daemon
priority: P1
depends-on: [daemon-lifecycle]
effort: M
---

# P2-10 — Idempotency Ledger

`spirit/outline.md` §4 ("The Silent Watcher") promises: *"A local ledger ensures DevDev never evaluates or complains about the same exact commit or ticket state twice."* MonitorPR (P2-07) currently caches `last_sha` per task instance, which means two concurrent tasks watching overlapping events, or a one-shot `devdev send` racing against a background task, can double-process the same commit. This capability adds a shared, daemon-scoped ledger.

## Scope

**In:**
- `IdempotencyLedger` trait + concrete `SledLedger` (or BTreeMap-on-disk; use whatever's already in the workspace deps — pick simplest).
- Keys: `(adapter, resource_type, resource_id, state_hash)`. Examples:
  - `("github", "pr_review", "owner/repo#247", "sha:abcd1234")` — "we reviewed PR 247 at sha abcd1234"
  - `("github", "issue_comment", "owner/repo#247#comment-99", "hash:..")` — "we reacted to comment 99 in its current form"
- API: `seen(key) -> bool`, `record(key, metadata)`, `prune(older_than: Duration)`.
- Persisted in the daemon's data directory (alongside checkpoints).
- Survives daemon restart (the whole point — outline calls it "local ledger", not "in-memory cache").
- Wired into MonitorPR: task asks the ledger before evaluating; records after successful action.
- Wired into `devdev send` / interactive chat too (one-shot reviews shouldn't re-review what a task already covered).

**Out:**
- Distributed ledger (multiple daemons sharing). Single-daemon scope.
- Retroactive expiry of past actions. `prune` removes old records, doesn't undo them.
- Replay / audit log. The ledger is "have we seen this", not "what did we do" — that's a Phase 6 observability concern.

## Interface

```rust
pub trait IdempotencyLedger: Send + Sync {
    fn seen(&self, key: &LedgerKey) -> Result<bool>;
    fn record(&self, key: &LedgerKey, metadata: serde_json::Value) -> Result<()>;
    fn prune(&self, older_than: Duration) -> Result<usize>;
}

pub struct LedgerKey {
    pub adapter: String,        // "github", "jira", ...
    pub resource_type: String,  // "pr_review", "issue_comment", ...
    pub resource_id: String,    // "owner/repo#247"
    pub state_hash: String,     // sha or content hash that defines "this exact state"
}
```

## Dependencies

- **P2-02 (daemon-lifecycle)** — needs the daemon's data directory and lifecycle hooks for prune-on-startup.

## Acceptance Criteria

- Two `MonitorPrTask` instances pointed at the same PR + same head SHA: only one evaluates; the second sees `seen() == true` and skips.
- Restart daemon: ledger entries survive; previously-seen events still skipped.
- `devdev send "review PR #247"` followed by background task watching the same PR at the same SHA: only one evaluation occurs.
- Pruning: `prune(Duration::from_days(90))` removes entries older than 90 days; recent entries untouched.
- Concurrent access from multiple tasks doesn't corrupt the ledger.

## Why Now (or Not Yet)

Originally implicit in the outline; surfaced during the 2026-04-22 alignment review. Should land alongside or just after P2-07 — without it, the first real production-shaped use case (background MonitorPR + ad-hoc `devdev send`) double-fires. Could be deferred until users actually hit the duplicate-action problem, but the cost of building it is small enough that the alignment-with-spec value wins.
