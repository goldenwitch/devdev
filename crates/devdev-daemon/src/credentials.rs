//! Credential snapshot for repository-host authentication.
//!
//! # Lifecycle contract
//!
//! Credentials are sampled **once**, at `devdev up` time, by running
//! a fixed set of [`CredentialProvider`]s against a fixed set of
//! [`RepoHostId`]s. The resulting [`CredentialStore`] is then
//! frozen — its inner `HashMap` is wrapped in `Arc`, callers receive
//! shared references, and there is no public mutation API after
//! [`CredentialStore::snapshot`] returns.
//!
//! This makes per-request credential reads deterministic. Mutating
//! `GH_TOKEN` (or any other env var or CLI session) after the daemon
//! has booted has **no effect** on tokens served from the snapshot.
//! Callers that need rotation must restart the daemon.
//!
//! # Provider model
//!
//! A [`CredentialProvider`] is bound to one [`RepoHostId`] and knows
//! how to fetch a token for it. Built-in providers:
//! * [`EnvVarProvider`] — reads a named environment variable.
//! * [`GhCliProvider`]  — shells out to `gh auth token` (github.com only).
//! * [`AzCliProvider`]  — shells out to `az account get-access-token`
//!   with the ADO resource id (Azure DevOps only).
//!
//! The [`CredentialProvider`] trait is `async` and returns `Result<
//! Option<Credential>, io::Error>`: `Ok(None)` for "this provider had
//! nothing to offer" (so the next provider for the same host is
//! tried), `Err` only for genuine I/O failures.
//!
//! # Token redaction
//!
//! Tokens are wrapped in [`RedactedString`], which redacts on `Debug`
//! and `Display` and only releases the raw value via
//! [`RedactedString::expose`]. This keeps tokens out of trace logs
//! and panic messages by default; callers must opt in explicitly.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use devdev_integrations::host::RepoHostId;
use tokio::process::Command;

/// A `String` newtype whose `Debug`/`Display` impls redact the value.
///
/// Construct via [`RedactedString::new`]; expose the raw value via
/// [`RedactedString::expose`]. `Clone` is intentional; equality is
/// not implemented (callers should not branch on token contents).
#[derive(Clone)]
pub struct RedactedString(String);

impl RedactedString {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the raw string. Use sparingly; the redaction is the
    /// whole point of this type.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RedactedString([redacted; {} bytes])", self.0.len())
    }
}

impl std::fmt::Display for RedactedString {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(()) // empty — never print a token by accident
    }
}

/// Where a credential came from. Recorded for diagnostics and
/// observable via [`Credential::source`]; not used for routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// Read from environment variable `name`.
    EnvVar { name: String },
    /// Captured via `gh auth token` (github.com).
    GhCli,
    /// Captured via `az account get-access-token` (Azure DevOps).
    AzCli,
    /// Test-only: injected directly via [`CredentialStore::with_entry`].
    Injected,
}

/// One credential entry in the snapshot. `sampled_at` is the wall-
/// clock time at snapshot construction; `expires_at_hint` exists to
/// train downstream consumers not to cache without bound, but is not
/// enforced here.
#[derive(Debug, Clone)]
pub struct Credential {
    host_id: RepoHostId,
    token: RedactedString,
    source: TokenSource,
    sampled_at: SystemTime,
}

impl Credential {
    /// Construct a credential from raw parts. Callers in production
    /// should let [`CredentialProvider`]s build credentials; this
    /// constructor is exposed for test injection and provider impls.
    pub fn new(
        host_id: RepoHostId,
        token: impl Into<String>,
        source: TokenSource,
    ) -> Self {
        Self {
            host_id,
            token: RedactedString::new(token),
            source,
            sampled_at: SystemTime::now(),
        }
    }

    pub fn host_id(&self) -> &RepoHostId {
        &self.host_id
    }

    pub fn token(&self) -> &RedactedString {
        &self.token
    }

    pub fn source(&self) -> &TokenSource {
        &self.source
    }

    pub fn sampled_at(&self) -> SystemTime {
        self.sampled_at
    }

    /// Wall-clock seconds since epoch when this credential was sampled.
    pub fn sampled_at_unix(&self) -> Option<u64> {
        self.sampled_at
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    }

    /// Conservative one-hour validity hint, surfaced to MCP clients
    /// via the `expires_at` field on `AskResponse::Approved`.
    pub fn expires_at_hint(&self) -> Option<u64> {
        const ONE_HOUR_SECS: u64 = 3600;
        self.sampled_at_unix().map(|t| t + ONE_HOUR_SECS)
    }
}

/// Async fetcher for one [`RepoHostId`]'s credential.
///
/// Implementations are constructed with the host id they serve and
/// returned from [`CredentialProvider::host_id`] — the snapshot
/// driver uses that to key entries in the resulting store.
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    fn host_id(&self) -> &RepoHostId;

    /// Try to produce a credential. `Ok(None)` means "no credential
    /// available from this provider"; the snapshot driver moves on
    /// to the next provider for the same host (if any).
    async fn fetch(&self) -> std::io::Result<Option<Credential>>;
}

// ── Built-in providers ──────────────────────────────────────────

/// Reads a token from a named environment variable. Empty values
/// are treated as missing.
pub struct EnvVarProvider {
    host_id: RepoHostId,
    var_name: String,
}

impl EnvVarProvider {
    pub fn new(host_id: RepoHostId, var_name: impl Into<String>) -> Self {
        Self {
            host_id,
            var_name: var_name.into(),
        }
    }

    pub fn var_name(&self) -> &str {
        &self.var_name
    }
}

#[async_trait]
impl CredentialProvider for EnvVarProvider {
    fn host_id(&self) -> &RepoHostId {
        &self.host_id
    }

    async fn fetch(&self) -> std::io::Result<Option<Credential>> {
        match std::env::var(&self.var_name) {
            Ok(v) if !v.is_empty() => Ok(Some(Credential::new(
                self.host_id.clone(),
                v,
                TokenSource::EnvVar {
                    name: self.var_name.clone(),
                },
            ))),
            _ => Ok(None),
        }
    }
}

/// Shells out to `gh auth token`. Only meaningful for github.com;
/// GHE installs need explicit env vars (or a future GHE-aware CLI
/// provider) since `gh` doesn't model multi-host credentials cleanly.
pub struct GhCliProvider {
    host_id: RepoHostId,
    timeout: Duration,
}

impl GhCliProvider {
    pub fn new(host_id: RepoHostId) -> Self {
        Self {
            host_id,
            timeout: Duration::from_secs(5),
        }
    }

    /// Override the subprocess timeout. Defaults to 5 s.
    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }
}

#[async_trait]
impl CredentialProvider for GhCliProvider {
    fn host_id(&self) -> &RepoHostId {
        &self.host_id
    }

    async fn fetch(&self) -> std::io::Result<Option<Credential>> {
        let mut cmd = Command::new("gh");
        cmd.arg("auth").arg("token");
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let output = match tokio::time::timeout(self.timeout, cmd.output()).await {
            Ok(r) => r?,
            Err(_) => return Ok(None),
        };
        if !output.status.success() {
            return Ok(None);
        }
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Credential::new(
                self.host_id.clone(),
                token,
                TokenSource::GhCli,
            )))
        }
    }
}

/// Shells out to `az account get-access-token --resource <ado-app-id>`
/// to obtain an AAD bearer token usable against ADO REST endpoints.
///
/// Note: ADO PATs are typically used for non-interactive automation
/// today, but `az`-issued AAD tokens are also accepted and have a
/// shorter blast radius. Operators preferring PATs should configure
/// an [`EnvVarProvider`] on `ADO_TOKEN` instead.
pub struct AzCliProvider {
    host_id: RepoHostId,
    timeout: Duration,
}

impl AzCliProvider {
    /// ADO's well-known AAD application id; used as `--resource`.
    pub const ADO_RESOURCE_ID: &'static str = "499b84ac-1321-427f-aa17-267ca6975798";

    pub fn new(host_id: RepoHostId) -> Self {
        Self {
            host_id,
            timeout: Duration::from_secs(10),
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }
}

#[async_trait]
impl CredentialProvider for AzCliProvider {
    fn host_id(&self) -> &RepoHostId {
        &self.host_id
    }

    async fn fetch(&self) -> std::io::Result<Option<Credential>> {
        let mut cmd = Command::new("az");
        cmd.arg("account")
            .arg("get-access-token")
            .arg("--resource")
            .arg(Self::ADO_RESOURCE_ID)
            .arg("--query")
            .arg("accessToken")
            .arg("-o")
            .arg("tsv");
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let output = match tokio::time::timeout(self.timeout, cmd.output()).await {
            Ok(r) => r?,
            Err(_) => return Ok(None),
        };
        if !output.status.success() {
            return Ok(None);
        }
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Credential::new(
                self.host_id.clone(),
                token,
                TokenSource::AzCli,
            )))
        }
    }
}

// ── Frozen store ────────────────────────────────────────────────

/// Frozen, snapshot-once credential lookup table.
///
/// Construction goes through [`CredentialStore::snapshot`] (production)
/// or [`CredentialStore::with_entries`] / [`CredentialStore::with_entry`]
/// (tests). After construction the inner table is `Arc`-wrapped and
/// shared by reference; there is no mutation API.
#[derive(Debug, Clone, Default)]
pub struct CredentialStore {
    entries: Arc<HashMap<RepoHostId, Credential>>,
}

impl CredentialStore {
    /// Empty store. Useful for tests and for `--no-credentials` boots.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a store directly from a list of credentials. Last entry
    /// for a given host wins. Test-only sugar.
    pub fn with_entries(creds: impl IntoIterator<Item = Credential>) -> Self {
        let mut map = HashMap::new();
        for c in creds {
            map.insert(c.host_id().clone(), c);
        }
        Self {
            entries: Arc::new(map),
        }
    }

    /// Convenience: a one-entry store. Equivalent to
    /// `with_entries([Credential::new(host, token, Injected)])`.
    pub fn with_entry(host_id: RepoHostId, token: impl Into<String>) -> Self {
        Self::with_entries([Credential::new(host_id, token, TokenSource::Injected)])
    }

    /// Run all providers in declaration order, keeping the **first**
    /// non-`None` result per [`RepoHostId`]. Errors from individual
    /// providers are logged and treated as `None`, so a single broken
    /// provider can't take down boot.
    ///
    /// Multiple providers for the same host id are allowed and form
    /// a fallback chain. Distinct host ids are independent.
    pub async fn snapshot(providers: Vec<Arc<dyn CredentialProvider>>) -> Self {
        let mut map: HashMap<RepoHostId, Credential> = HashMap::new();
        for provider in providers {
            let host = provider.host_id().clone();
            if map.contains_key(&host) {
                continue; // earlier provider already won
            }
            match provider.fetch().await {
                Ok(Some(cred)) => {
                    map.insert(host, cred);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        host = %host,
                        error = %e,
                        "credential provider failed; treating as no credential",
                    );
                }
            }
        }
        Self {
            entries: Arc::new(map),
        }
    }

    /// Resolve a credential by host id. Returns `None` if no provider
    /// produced one at snapshot time.
    pub fn get(&self, host_id: &RepoHostId) -> Option<&Credential> {
        self.entries.get(host_id)
    }

    /// Number of entries. Diagnostic only.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate entries (host_id, credential). Order is unspecified.
    pub fn iter(&self) -> impl Iterator<Item = (&RepoHostId, &Credential)> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh() -> RepoHostId {
        RepoHostId::github_com()
    }

    fn ado() -> RepoHostId {
        RepoHostId::azure_devops()
    }

    // ── RedactedString ─────────────────────────────────────────

    #[test]
    fn redacted_string_does_not_leak_via_debug_or_display() {
        let r = RedactedString::new("ghp_super_secret");
        let d = format!("{r:?}");
        let s = format!("{r}");
        assert!(!d.contains("ghp_super_secret"), "debug leaked: {d}");
        assert!(!s.contains("ghp_super_secret"), "display leaked: {s}");
        assert_eq!(r.expose(), "ghp_super_secret");
    }

    // ── Snapshot lifecycle ─────────────────────────────────────

    struct StaticProvider {
        host_id: RepoHostId,
        token: Option<&'static str>,
    }

    #[async_trait]
    impl CredentialProvider for StaticProvider {
        fn host_id(&self) -> &RepoHostId {
            &self.host_id
        }
        async fn fetch(&self) -> std::io::Result<Option<Credential>> {
            Ok(self
                .token
                .map(|t| Credential::new(self.host_id.clone(), t, TokenSource::Injected)))
        }
    }

    #[tokio::test]
    async fn snapshot_records_token_for_each_host() {
        let store = CredentialStore::snapshot(vec![
            Arc::new(StaticProvider {
                host_id: gh(),
                token: Some("gh-tok"),
            }),
            Arc::new(StaticProvider {
                host_id: ado(),
                token: Some("ado-tok"),
            }),
        ])
        .await;

        assert_eq!(store.len(), 2);
        assert_eq!(store.get(&gh()).unwrap().token().expose(), "gh-tok");
        assert_eq!(store.get(&ado()).unwrap().token().expose(), "ado-tok");
    }

    #[tokio::test]
    async fn snapshot_falls_through_on_none_within_chain() {
        let store = CredentialStore::snapshot(vec![
            Arc::new(StaticProvider {
                host_id: gh(),
                token: None,
            }),
            Arc::new(StaticProvider {
                host_id: gh(),
                token: Some("fallback"),
            }),
        ])
        .await;
        assert_eq!(store.get(&gh()).unwrap().token().expose(), "fallback");
    }

    #[tokio::test]
    async fn snapshot_first_provider_wins_when_both_have_token() {
        let store = CredentialStore::snapshot(vec![
            Arc::new(StaticProvider {
                host_id: gh(),
                token: Some("primary"),
            }),
            Arc::new(StaticProvider {
                host_id: gh(),
                token: Some("backup"),
            }),
        ])
        .await;
        assert_eq!(store.get(&gh()).unwrap().token().expose(), "primary");
    }

    #[tokio::test]
    async fn store_is_immutable_after_snapshot_env_mutation() {
        // SAFETY: this test single-threadedly writes/reads the env
        // var; no other test in this binary touches it.
        let var = "DEVDEV_TEST_CRED_SNAPSHOT_TOKEN";
        unsafe { std::env::set_var(var, "before") };

        let store = CredentialStore::snapshot(vec![Arc::new(EnvVarProvider::new(gh(), var))]).await;
        assert_eq!(store.get(&gh()).unwrap().token().expose(), "before");

        // Mutate the env after snapshot; the store must not change.
        unsafe { std::env::set_var(var, "after") };
        assert_eq!(store.get(&gh()).unwrap().token().expose(), "before");

        // Cleanup.
        unsafe { std::env::remove_var(var) };
    }

    // ── EnvVarProvider ─────────────────────────────────────────

    #[tokio::test]
    async fn env_var_provider_returns_none_for_unset_var() {
        let var = "DEVDEV_TEST_CRED_UNSET_VAR_ABC";
        unsafe { std::env::remove_var(var) };
        let p = EnvVarProvider::new(gh(), var);
        assert!(p.fetch().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn env_var_provider_returns_none_for_empty_var() {
        let var = "DEVDEV_TEST_CRED_EMPTY_VAR";
        unsafe { std::env::set_var(var, "") };
        let p = EnvVarProvider::new(gh(), var);
        assert!(p.fetch().await.unwrap().is_none());
        unsafe { std::env::remove_var(var) };
    }

    #[tokio::test]
    async fn env_var_provider_records_source() {
        let var = "DEVDEV_TEST_CRED_RECORD_SRC";
        unsafe { std::env::set_var(var, "ok") };
        let p = EnvVarProvider::new(gh(), var);
        let cred = p.fetch().await.unwrap().unwrap();
        match cred.source() {
            TokenSource::EnvVar { name } => assert_eq!(name, var),
            other => panic!("wrong source: {other:?}"),
        }
        unsafe { std::env::remove_var(var) };
    }

    // ── with_entries / with_entry ──────────────────────────────

    #[test]
    fn with_entry_round_trips() {
        let store = CredentialStore::with_entry(gh(), "tok");
        assert_eq!(store.get(&gh()).unwrap().token().expose(), "tok");
        assert_eq!(store.get(&gh()).unwrap().source(), &TokenSource::Injected);
        assert!(store.get(&ado()).is_none());
    }

    #[test]
    fn empty_store_returns_none_and_is_clonable() {
        let store = CredentialStore::empty();
        assert!(store.is_empty());
        let clone = store.clone();
        assert!(clone.get(&gh()).is_none());
    }

    // ── Hint timing ────────────────────────────────────────────

    #[test]
    fn expires_at_hint_is_one_hour_after_sample() {
        let cred = Credential::new(gh(), "x", TokenSource::Injected);
        let sampled = cred.sampled_at_unix().unwrap();
        let exp = cred.expires_at_hint().unwrap();
        assert_eq!(exp - sampled, 3600);
    }
}
