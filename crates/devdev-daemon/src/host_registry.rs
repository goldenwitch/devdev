//! Routes `RepoHostAdapter` lookups by `RepoHostId`.
//!
//! The registry is the single source of truth for "given this PR
//! URL or host id, which adapter speaks its API?". It owns no
//! credential material; tokens flow through [`crate::credentials`]
//! and are correlated by the same [`RepoHostId`] keys.
//!
//! ## Identity model
//!
//! Adapters are keyed by a fully-resolved [`RepoHostId`]
//! (`{kind, api_base, host}`). Two registry entries with the same
//! `host` but different `kind`/`api_base` are not currently
//! supported (and would indicate a mis-configuration upstream).
//!
//! ## URL routing
//!
//! [`RepoHostRegistry::for_url`] strips the scheme + path and asks
//! [`RepoHostId::from_browse_host`] to classify the bare host. If
//! classification succeeds AND the registry has an entry for the
//! resulting host id, the adapter is returned. Unknown hosts and
//! unregistered-but-known hosts both yield `None` so callers can
//! tell "we don't know that host" apart from "we recognise it but
//! aren't watching it" by also consulting `RepoHostId::from_browse_host`.

use std::collections::HashMap;
use std::sync::Arc;

use devdev_integrations::RepoHostAdapter;
use devdev_integrations::host::RepoHostId;

/// Read-only registry of [`RepoHostAdapter`]s keyed by [`RepoHostId`].
///
/// Built once at `devdev up` from preferences; immutable afterward
/// to mirror the [`crate::credentials::CredentialStore`] lifecycle
/// model. Mutation would invite the same race window we eliminated
/// in Phase 4 (a fetch operation observing a partially-installed
/// registry), so we don't expose any.
#[derive(Clone)]
pub struct RepoHostRegistry {
    adapters: Arc<HashMap<RepoHostId, Arc<dyn RepoHostAdapter>>>,
}

impl RepoHostRegistry {
    /// Build a registry from a fully-prepared map. Callers use
    /// [`RepoHostRegistryBuilder`] for incremental construction.
    pub fn from_map(adapters: HashMap<RepoHostId, Arc<dyn RepoHostAdapter>>) -> Self {
        Self {
            adapters: Arc::new(adapters),
        }
    }

    /// Empty registry — only useful as a placeholder in tests that
    /// never hit the adapter lookup path.
    pub fn empty() -> Self {
        Self::from_map(HashMap::new())
    }

    /// Convenience constructor for the common single-host case.
    pub fn single(host_id: RepoHostId, adapter: Arc<dyn RepoHostAdapter>) -> Self {
        let mut m = HashMap::new();
        m.insert(host_id, adapter);
        Self::from_map(m)
    }

    /// Number of registered adapters.
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    /// True when no adapters are registered.
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    /// Look up an adapter by its host id.
    pub fn for_host(&self, host_id: &RepoHostId) -> Option<&Arc<dyn RepoHostAdapter>> {
        self.adapters.get(host_id)
    }

    /// Look up an adapter for a browser-shaped URL or bare host
    /// string. Returns `None` if the host can't be classified or
    /// isn't registered.
    pub fn for_url(&self, url_or_host: &str) -> Option<&Arc<dyn RepoHostAdapter>> {
        let host = extract_host(url_or_host)?;
        let host_id = RepoHostId::from_browse_host(host)?;
        self.for_host(&host_id)
    }

    /// Iterate over `(host_id, adapter)` pairs in arbitrary order.
    pub fn iter(&self) -> impl Iterator<Item = (&RepoHostId, &Arc<dyn RepoHostAdapter>)> {
        self.adapters.iter()
    }
}

/// Strip scheme + path, return the bare host. Accepts both full
/// URLs (`https://github.com/o/r/pull/1`) and bare hosts
/// (`github.com`). Empty input returns `None`.
fn extract_host(input: &str) -> Option<&str> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let after_scheme = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let host = after_scheme.split('/').next()?;
    if host.is_empty() { None } else { Some(host) }
}

/// Incremental builder for [`RepoHostRegistry`]. Insertions overwrite
/// silently; if you need conflict detection in the future, layer it
/// on the calling side.
#[derive(Default)]
pub struct RepoHostRegistryBuilder {
    adapters: HashMap<RepoHostId, Arc<dyn RepoHostAdapter>>,
}

impl RepoHostRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, host_id: RepoHostId, adapter: Arc<dyn RepoHostAdapter>) -> Self {
        self.adapters.insert(host_id, adapter);
        self
    }

    pub fn insert(&mut self, host_id: RepoHostId, adapter: Arc<dyn RepoHostAdapter>) {
        self.adapters.insert(host_id, adapter);
    }

    pub fn build(self) -> RepoHostRegistry {
        RepoHostRegistry::from_map(self.adapters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use devdev_integrations::{Comment, PrStatus, PullRequest, RepoHostError, Review};

    /// Adapter stub that records its identity for lookup assertions.
    /// Only `host_id` is meaningful; everything else returns NotFound
    /// so any accidental call panics loudly in tests.
    struct StubAdapter {
        host_id: RepoHostId,
    }

    #[async_trait]
    impl RepoHostAdapter for StubAdapter {
        fn host_id(&self) -> &RepoHostId {
            &self.host_id
        }
        async fn get_pr(&self, _o: &str, _r: &str, _n: u64) -> Result<PullRequest, RepoHostError> {
            Err(RepoHostError::NotFound("stub".into()))
        }
        async fn get_pr_diff(&self, _o: &str, _r: &str, _n: u64) -> Result<String, RepoHostError> {
            Err(RepoHostError::NotFound("stub".into()))
        }
        async fn list_pr_comments(
            &self,
            _o: &str,
            _r: &str,
            _n: u64,
        ) -> Result<Vec<Comment>, RepoHostError> {
            Ok(vec![])
        }
        async fn post_review(
            &self,
            _o: &str,
            _r: &str,
            _n: u64,
            _review: Review,
        ) -> Result<(), RepoHostError> {
            Ok(())
        }
        async fn post_comment(
            &self,
            _o: &str,
            _r: &str,
            _n: u64,
            _body: &str,
        ) -> Result<(), RepoHostError> {
            Ok(())
        }
        async fn get_pr_status(
            &self,
            _o: &str,
            _r: &str,
            _n: u64,
        ) -> Result<PrStatus, RepoHostError> {
            Err(RepoHostError::NotFound("stub".into()))
        }
        async fn get_pr_head_sha(
            &self,
            _o: &str,
            _r: &str,
            _n: u64,
        ) -> Result<String, RepoHostError> {
            Err(RepoHostError::NotFound("stub".into()))
        }
        async fn list_open_prs(
            &self,
            _o: &str,
            _r: &str,
        ) -> Result<Vec<PullRequest>, RepoHostError> {
            Ok(vec![])
        }
    }

    fn stub(host_id: RepoHostId) -> Arc<dyn RepoHostAdapter> {
        Arc::new(StubAdapter { host_id })
    }

    #[test]
    fn empty_registry_returns_none() {
        let r = RepoHostRegistry::empty();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(r.for_host(&RepoHostId::github_com()).is_none());
        assert!(r.for_url("https://github.com/o/r").is_none());
    }

    #[test]
    fn single_round_trips() {
        let id = RepoHostId::github_com();
        let r = RepoHostRegistry::single(id.clone(), stub(id.clone()));
        assert_eq!(r.len(), 1);
        let got = r.for_host(&id).expect("present");
        assert_eq!(got.host_id(), &id);
    }

    #[test]
    fn for_url_routes_to_correct_adapter() {
        let gh = RepoHostId::github_com();
        let ghe = RepoHostId::ghe("ghe.acme.io");
        let ado = RepoHostId::azure_devops();
        let r = RepoHostRegistryBuilder::new()
            .with(gh.clone(), stub(gh.clone()))
            .with(ghe.clone(), stub(ghe.clone()))
            .with(ado.clone(), stub(ado.clone()))
            .build();

        assert_eq!(
            r.for_url("https://github.com/o/r/pull/1").unwrap().host_id(),
            &gh
        );
        assert_eq!(
            r.for_url("https://ghe.acme.io/o/r/pull/1").unwrap().host_id(),
            &ghe
        );
        assert_eq!(
            r.for_url("https://dev.azure.com/org/proj/_git/repo/pullrequest/1")
                .unwrap()
                .host_id(),
            &ado
        );
    }

    #[test]
    fn for_url_accepts_bare_host() {
        let id = RepoHostId::github_com();
        let r = RepoHostRegistry::single(id.clone(), stub(id));
        assert!(r.for_url("github.com").is_some());
        assert!(r.for_url("www.github.com").is_some());
    }

    #[test]
    fn for_url_returns_none_for_unknown_host() {
        let id = RepoHostId::github_com();
        let r = RepoHostRegistry::single(id.clone(), stub(id));
        assert!(r.for_url("https://gitlab.com/o/r").is_none());
        assert!(r.for_url("not a url").is_none());
        assert!(r.for_url("").is_none());
    }

    #[test]
    fn for_url_returns_none_for_unregistered_known_host() {
        // Host classifies but we never registered it.
        let id = RepoHostId::github_com();
        let r = RepoHostRegistry::single(id.clone(), stub(id));
        assert!(r.for_url("https://ghe.example.com/o/r").is_none());
    }

    #[test]
    fn builder_overwrite_keeps_last() {
        let id = RepoHostId::github_com();
        let other = RepoHostId::ghe("ghe.example.com");
        let r = RepoHostRegistryBuilder::new()
            .with(id.clone(), stub(other.clone()))
            .with(id.clone(), stub(id.clone()))
            .build();
        assert_eq!(r.for_host(&id).unwrap().host_id(), &id);
    }

    #[test]
    fn extract_host_strips_scheme_and_path() {
        assert_eq!(extract_host("https://github.com/o/r"), Some("github.com"));
        assert_eq!(extract_host("http://github.com"), Some("github.com"));
        assert_eq!(extract_host("github.com/o/r"), Some("github.com"));
        assert_eq!(extract_host("github.com"), Some("github.com"));
        assert_eq!(extract_host("  https://x.io/y  "), Some("x.io"));
        assert_eq!(extract_host(""), None);
        assert_eq!(extract_host("/leading-slash"), None);
    }
}
