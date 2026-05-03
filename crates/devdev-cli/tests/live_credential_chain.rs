//! Live credential chain — proves the production credential
//! providers resolve real tokens against real CLIs.
//!
//! This test does **not** assert provider precedence (that's a unit
//! test in `devdev-daemon::credentials`). It just proves each
//! provider, on its own, can talk to the CLI it shells out to and
//! produce a non-empty token under realistic conditions.
//!
//! ## Scope
//!
//! * `gh_cli_provider_yields_token` — runs `gh auth token` via
//!   [`GhCliProvider`]. Gated by `DEVDEV_LIVE_CRED_GH=1`. Requires
//!   `gh` on PATH and an authenticated session (the CI workflow
//!   primes this with `gh auth login --with-token <DEVDEV_COPILOT_GH_TOKEN>`).
//! * `az_cli_provider_yields_token` — runs `az account get-access-token`
//!   via [`AzCliProvider`]. Gated by `DEVDEV_LIVE_CRED_AZ=1`. Requires
//!   `az` on PATH and a logged-in session (CI uses `azure/login`).
//!
//! Both tests assert the resulting credential's `host_id` matches the
//! one the provider was constructed for; the token itself is touched
//! through `expose()` only to verify it's non-empty.

use devdev_daemon::credentials::{AzCliProvider, CredentialProvider, GhCliProvider, TokenSource};
use devdev_integrations::host::RepoHostId;

fn flag(var: &str) -> bool {
    std::env::var(var)
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "live: requires gh CLI signed in; run with DEVDEV_LIVE_CRED_GH=1 --ignored"]
async fn gh_cli_provider_yields_token() {
    if !flag("DEVDEV_LIVE_CRED_GH") {
        eprintln!("SKIP: DEVDEV_LIVE_CRED_GH not set");
        return;
    }
    let host = RepoHostId::github_com();
    let provider = GhCliProvider::new(host.clone());

    let cred = provider
        .fetch()
        .await
        .expect("gh CLI provider returned an I/O error")
        .expect("gh CLI provider returned None — is `gh auth login` complete?");

    assert_eq!(cred.host_id(), &host, "credential stamped wrong host_id");
    assert!(
        matches!(cred.source(), TokenSource::GhCli),
        "credential source not stamped as GhCli: {:?}",
        cred.source()
    );
    let token = cred.token().expose();
    assert!(!token.is_empty(), "gh auth token returned an empty token");
    // Light shape check: tokens issued by gh start with one of a few
    // known prefixes. Don't assert the prefix value — that's an
    // implementation detail of GitHub. Just that it parses as ASCII.
    assert!(
        token.is_ascii(),
        "non-ASCII payload in gh-issued token (suspicious)"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "live: requires az CLI signed in; run with DEVDEV_LIVE_CRED_AZ=1 --ignored"]
async fn az_cli_provider_yields_token() {
    if !flag("DEVDEV_LIVE_CRED_AZ") {
        eprintln!("SKIP: DEVDEV_LIVE_CRED_AZ not set");
        return;
    }
    let host = RepoHostId::azure_devops();
    let provider = AzCliProvider::new(host.clone());

    let cred = provider
        .fetch()
        .await
        .expect("az CLI provider returned an I/O error")
        .expect("az CLI provider returned None — is `az login` complete?");

    assert_eq!(cred.host_id(), &host);
    assert!(
        matches!(cred.source(), TokenSource::AzCli),
        "credential source not stamped as AzCli: {:?}",
        cred.source()
    );
    let token = cred.token().expose();
    assert!(!token.is_empty(), "az get-access-token returned empty");
    assert!(token.is_ascii(), "non-ASCII payload in AAD token");
}
