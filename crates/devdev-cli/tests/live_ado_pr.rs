//! Live ADO PR test — proves [`AzureDevOpsAdapter`] talks to real
//! Azure DevOps Services and that the per-PR thread/comment surface
//! works end-to-end.
//!
//! ## Read-mode (default)
//!
//! Resolves `DEVDEV_ADO_PR_URL` via [`PrRef::parse`], constructs the
//! adapter, and exercises:
//!
//! 1. `get_pr` — round-trips title and number.
//! 2. `list_pr_comments` — succeeds (empty or non-empty is fine; the
//!    cleanup job deletes tagged comments after every run).
//!
//! Gated by `DEVDEV_LIVE_HOSTS=1`.
//!
//! ## Write-mode
//!
//! Additionally posts a *tagged* comment of the form
//! `[devdev-live-test:live_ado_pr:<nonce>] hello from <ts>`, then
//! verifies it appears in the next `list_pr_comments` response.
//!
//! Gated by `DEVDEV_LIVE_WRITE=1`. The CI workflow's `cleanup` job
//! sweeps the comment afterwards via `devdev-test-env reset-comments`.
//! Locally, you have to clean up by hand or re-run with the same
//! admin token.
//!
//! Marked `#[serial]` because the comment-tag nonce uses a wall-clock
//! second; running concurrent invocations could (in theory) collide.

use devdev_integrations::host::RepoHostId;
use devdev_integrations::{AzureDevOpsAdapter, RepoHostAdapter};
use devdev_tasks::pr_ref::PrRef;
use serial_test::serial;

fn flag(var: &str) -> bool {
    std::env::var(var)
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn require_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("SKIP: {name} not set");
            None
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "live: requires ADO fixture; run with DEVDEV_LIVE_HOSTS=1 --ignored"]
#[serial]
async fn ado_canonical_pr_read_path() {
    if !flag("DEVDEV_LIVE_HOSTS") {
        eprintln!("SKIP: DEVDEV_LIVE_HOSTS not set");
        return;
    }
    let token = match require_env("DEVDEV_ADO_TOKEN") {
        Some(v) => v,
        None => return,
    };
    let pr_url = match require_env("DEVDEV_ADO_PR_URL") {
        Some(v) => v,
        None => return,
    };

    let parsed = PrRef::parse(&pr_url).expect("PrRef::parse fixture URL");
    assert_eq!(parsed.host_id, RepoHostId::azure_devops());

    let adapter = AzureDevOpsAdapter::new(token);
    let pr = adapter
        .get_pr(&parsed.owner, &parsed.repo, parsed.number)
        .await
        .expect("get_pr");
    assert_eq!(pr.number, parsed.number);

    // Read-side smoke: list_pr_comments must succeed regardless of
    // whether the cleanup left it empty.
    let _ = adapter
        .list_pr_comments(&parsed.owner, &parsed.repo, parsed.number)
        .await
        .expect("list_pr_comments");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "live: posts a tagged comment; run with DEVDEV_LIVE_HOSTS=1 DEVDEV_LIVE_WRITE=1 --ignored"]
#[serial]
async fn ado_canonical_pr_write_path() {
    if !flag("DEVDEV_LIVE_HOSTS") {
        eprintln!("SKIP: DEVDEV_LIVE_HOSTS not set");
        return;
    }
    if !flag("DEVDEV_LIVE_WRITE") {
        eprintln!("SKIP: DEVDEV_LIVE_WRITE not set");
        return;
    }
    let token = match require_env("DEVDEV_ADO_TOKEN") {
        Some(v) => v,
        None => return,
    };
    let pr_url = match require_env("DEVDEV_ADO_PR_URL") {
        Some(v) => v,
        None => return,
    };

    let parsed = PrRef::parse(&pr_url).expect("PrRef::parse fixture URL");
    let adapter = AzureDevOpsAdapter::new(token);

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!(
        "[devdev-live-test:live_ado_pr:{nonce}] hello from devdev-cli live test"
    );

    adapter
        .post_comment(&parsed.owner, &parsed.repo, parsed.number, &body)
        .await
        .expect("post_comment");

    // Verify it landed.
    let comments = adapter
        .list_pr_comments(&parsed.owner, &parsed.repo, parsed.number)
        .await
        .expect("list_pr_comments after post");

    let needle = format!("live_ado_pr:{nonce}");
    let found = comments.iter().any(|c| c.body.contains(&needle));
    assert!(
        found,
        "tagged comment not visible in PR thread (n={}, needle={needle})",
        comments.len()
    );
}
