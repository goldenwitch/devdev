//! Live host probe — smoke test that the hand-rolled REST clients in
//! `devdev-integrations` can authenticate against both fixture hosts
//! and round-trip a known PR.
//!
//! ## Scope
//!
//! Read-only. Resolves the canonical fixture PR on each host, calls
//! `get_pr` + `list_open_prs`, and asserts the host_id stamp on the
//! returned record matches the host the adapter was constructed for.
//!
//! ## Running
//!
//! Opt-in, `--ignored`, gated by `DEVDEV_LIVE_HOSTS=1`. The CI
//! workflow's `live-tests` job exports the per-host fixture
//! coordinates via `devdev-test-env print-env` after `provision`
//! lands the manifest.lock. Locally:
//!
//! ```pwsh
//! $env:DEVDEV_LIVE_HOSTS = "1"
//! $env:DEVDEV_GH_TOKEN = "<consumer pat>"
//! $env:DEVDEV_ADO_TOKEN = "<consumer pat>"
//! cargo run -p devdev-test-env -- print-env | Out-File .env.live
//! . ./.env.live
//! cargo test -p devdev-cli --test live_host_probe -- --ignored --nocapture
//! ```
//!
//! If `DEVDEV_LIVE_HOSTS` is not `1`, both tests skip with a clear
//! message rather than masquerading as a pass.

use devdev_integrations::host::RepoHostId;
use devdev_integrations::{AzureDevOpsAdapter, GitHubAdapter, RepoHostAdapter};

fn live_enabled() -> bool {
    std::env::var("DEVDEV_LIVE_HOSTS")
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn require_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!(
                "SKIP: {name} not set; cannot run live host probe (did `devdev-test-env print-env` run?)"
            );
            None
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "live: requires fixture environment; run with DEVDEV_LIVE_HOSTS=1 --ignored"]
async fn github_canonical_pr_round_trips() {
    if !live_enabled() {
        eprintln!("SKIP: DEVDEV_LIVE_HOSTS not set");
        return;
    }
    let token = match require_env("DEVDEV_GH_TOKEN") {
        Some(v) => v,
        None => return,
    };
    let pr_url = match require_env("DEVDEV_GH_PR_URL") {
        Some(v) => v,
        None => return,
    };

    // Parse: https://github.com/<org>/<repo>/pull/<n>
    let parsed = devdev_tasks::pr_ref::PrRef::parse(&pr_url)
        .unwrap_or_else(|e| panic!("PrRef::parse({pr_url:?}): {e}"));
    assert_eq!(
        parsed.host_id,
        RepoHostId::github_com(),
        "fixture URL classified as the wrong host"
    );

    let adapter = GitHubAdapter::new(RepoHostId::github_com(), token);
    let pr = adapter
        .get_pr(&parsed.owner, &parsed.repo, parsed.number)
        .await
        .unwrap_or_else(|e| panic!("get_pr against canonical fixture failed: {e}"));

    assert_eq!(pr.number, parsed.number, "round-tripped wrong PR number");
    assert!(
        pr.title.contains("Canonical fixture"),
        "fixture PR title drifted: {:?}",
        pr.title
    );
    assert_eq!(
        adapter.host_id(),
        &RepoHostId::github_com(),
        "adapter host_id stamp drifted"
    );

    let open = adapter
        .list_open_prs(&parsed.owner, &parsed.repo)
        .await
        .unwrap_or_else(|e| panic!("list_open_prs failed: {e}"));
    assert!(
        open.iter().any(|p| p.number == parsed.number),
        "canonical PR not in list_open_prs result (n={})",
        open.len()
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "live: requires fixture environment; run with DEVDEV_LIVE_HOSTS=1 --ignored"]
async fn ado_canonical_pr_round_trips() {
    if !live_enabled() {
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

    let parsed = devdev_tasks::pr_ref::PrRef::parse(&pr_url)
        .unwrap_or_else(|e| panic!("PrRef::parse({pr_url:?}): {e}"));
    assert_eq!(
        parsed.host_id,
        RepoHostId::azure_devops(),
        "fixture URL classified as the wrong host"
    );

    let adapter = AzureDevOpsAdapter::new(token);
    let pr = adapter
        .get_pr(&parsed.owner, &parsed.repo, parsed.number)
        .await
        .unwrap_or_else(|e| panic!("get_pr against canonical ADO fixture failed: {e}"));

    assert_eq!(pr.number, parsed.number);
    assert!(
        pr.title.contains("Canonical fixture"),
        "fixture PR title drifted: {:?}",
        pr.title
    );
    assert_eq!(adapter.host_id(), &RepoHostId::azure_devops());

    let open = adapter
        .list_open_prs(&parsed.owner, &parsed.repo)
        .await
        .unwrap_or_else(|e| panic!("list_open_prs failed: {e}"));
    assert!(
        open.iter().any(|p| p.number == parsed.number),
        "canonical PR not in list_open_prs result (n={})",
        open.len()
    );
}
