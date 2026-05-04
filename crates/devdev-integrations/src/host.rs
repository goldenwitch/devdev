//! Host identification for repository forges.
//!
//! A `RepoHostId` pairs a forge family ([`RepoHostKind`]) with the
//! base URL of a specific instance. It is the routing key used by the
//! daemon's host registry to dispatch agent tool calls and watch-repo
//! event polling to the correct adapter implementation.
//!
//! Classification rules:
//! * `github.com` and any `*.ghe.com` host → [`RepoHostKind::GitHub`]
//!   with API base `https://<host>/api/v3` (GHE) or
//!   `https://api.github.com` (github.com).
//! * `dev.azure.com` and `*.visualstudio.com` →
//!   [`RepoHostKind::AzureDevOps`].
//! * Anything else → unclassified; callers must supply the kind
//!   explicitly via configuration.

use serde::{Deserialize, Serialize};

/// Family of repository forge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoHostKind {
    /// GitHub.com or a GitHub Enterprise Server instance. Both speak
    /// the same REST surface; only the API base URL differs.
    GitHub,
    /// Azure DevOps Services (`dev.azure.com`) or a legacy
    /// Visual Studio Team Services host.
    AzureDevOps,
}

/// Stable identifier for a forge instance: a kind + the canonical
/// API base URL. Used as a `HashMap` key in the daemon registry and
/// embedded in ledger entries / `PrRef` values.
///
/// Constructed via [`RepoHostId::github_com`], [`RepoHostId::ghe`], or
/// [`RepoHostId::azure_devops`]; or via [`RepoHostId::from_browse_url`]
/// for URL-driven dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoHostId {
    pub kind: RepoHostKind,
    /// API base URL **without** trailing slash, e.g.
    /// `https://api.github.com` or `https://ghe.example.com/api/v3` or
    /// `https://dev.azure.com`.
    pub api_base: String,
    /// Browse-URL host, e.g. `github.com`, `ghe.example.com`,
    /// `dev.azure.com`. Used for ledger keys and human display.
    pub host: String,
}

impl RepoHostId {
    /// `https://api.github.com` against `github.com`.
    pub fn github_com() -> Self {
        Self {
            kind: RepoHostKind::GitHub,
            api_base: "https://api.github.com".to_string(),
            host: "github.com".to_string(),
        }
    }

    /// GitHub Enterprise Server instance hosted at `host` (e.g.
    /// `ghe.example.com`). API base is `https://<host>/api/v3`.
    pub fn ghe(host: impl Into<String>) -> Self {
        let host = host.into();
        let api_base = format!("https://{host}/api/v3");
        Self {
            kind: RepoHostKind::GitHub,
            api_base,
            host,
        }
    }

    /// Azure DevOps Services. The API base is the same for every org
    /// (`https://dev.azure.com`); per-org routing happens in the URL
    /// path, not the host.
    pub fn azure_devops() -> Self {
        Self {
            kind: RepoHostKind::AzureDevOps,
            api_base: "https://dev.azure.com".to_string(),
            host: "dev.azure.com".to_string(),
        }
    }

    /// Stable string key suitable for use in the idempotency ledger
    /// or any other deduplication store. Format: `<kind>:<host>`.
    pub fn ledger_key(&self) -> String {
        let kind = match self.kind {
            RepoHostKind::GitHub => "github",
            RepoHostKind::AzureDevOps => "ado",
        };
        format!("{kind}:{}", self.host)
    }

    /// Best-effort classification of a *browse* URL host (the `host`
    /// portion of a URL like `https://ghe.example.com/owner/repo`).
    ///
    /// Returns `None` when the host doesn't match any known forge.
    /// Callers should fall through to explicit configuration in that
    /// case rather than guessing.
    pub fn classify_host(host: &str) -> Option<RepoHostKind> {
        let host = host.to_ascii_lowercase();
        if host == "github.com" || host == "www.github.com" {
            return Some(RepoHostKind::GitHub);
        }
        if host == "dev.azure.com" || host.ends_with(".visualstudio.com") {
            return Some(RepoHostKind::AzureDevOps);
        }
        // Heuristic: `ghe.*` or `github.*` (GitHub Enterprise Server
        // installs commonly use these prefixes). Conservative — only
        // hits when the segment is the literal "ghe" or "github".
        if host.starts_with("ghe.") || host.starts_with("github.") {
            return Some(RepoHostKind::GitHub);
        }
        None
    }

    /// Build a [`RepoHostId`] from a browse-URL host string. Returns
    /// `None` when classification fails.
    pub fn from_browse_host(host: &str) -> Option<Self> {
        match Self::classify_host(host)? {
            RepoHostKind::GitHub => {
                if host.eq_ignore_ascii_case("github.com")
                    || host.eq_ignore_ascii_case("www.github.com")
                {
                    Some(Self::github_com())
                } else {
                    Some(Self::ghe(host))
                }
            }
            RepoHostKind::AzureDevOps => Some(Self::azure_devops()),
        }
    }
}

impl std::fmt::Display for RepoHostId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.ledger_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_github_com() {
        assert_eq!(
            RepoHostId::classify_host("github.com"),
            Some(RepoHostKind::GitHub)
        );
        assert_eq!(
            RepoHostId::classify_host("GitHub.com"),
            Some(RepoHostKind::GitHub)
        );
    }

    #[test]
    fn classify_ghe() {
        assert_eq!(
            RepoHostId::classify_host("ghe.example.com"),
            Some(RepoHostKind::GitHub)
        );
        assert_eq!(
            RepoHostId::classify_host("github.example.com"),
            Some(RepoHostKind::GitHub)
        );
    }

    #[test]
    fn classify_ado() {
        assert_eq!(
            RepoHostId::classify_host("dev.azure.com"),
            Some(RepoHostKind::AzureDevOps)
        );
        assert_eq!(
            RepoHostId::classify_host("contoso.visualstudio.com"),
            Some(RepoHostKind::AzureDevOps)
        );
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(RepoHostId::classify_host("gitlab.com"), None);
        assert_eq!(RepoHostId::classify_host("bitbucket.org"), None);
    }

    #[test]
    fn ghe_api_base_path() {
        let id = RepoHostId::ghe("ghe.example.com");
        assert_eq!(id.api_base, "https://ghe.example.com/api/v3");
        assert_eq!(id.host, "ghe.example.com");
    }

    #[test]
    fn ledger_key_format() {
        assert_eq!(RepoHostId::github_com().ledger_key(), "github:github.com");
        assert_eq!(
            RepoHostId::ghe("ghe.acme.io").ledger_key(),
            "github:ghe.acme.io"
        );
        assert_eq!(
            RepoHostId::azure_devops().ledger_key(),
            "ado:dev.azure.com"
        );
    }

    #[test]
    fn from_browse_host_routes_correctly() {
        assert_eq!(
            RepoHostId::from_browse_host("github.com"),
            Some(RepoHostId::github_com())
        );
        assert_eq!(
            RepoHostId::from_browse_host("ghe.example.com"),
            Some(RepoHostId::ghe("ghe.example.com"))
        );
        assert_eq!(
            RepoHostId::from_browse_host("dev.azure.com"),
            Some(RepoHostId::azure_devops())
        );
        assert_eq!(RepoHostId::from_browse_host("gitlab.com"), None);
    }
}
