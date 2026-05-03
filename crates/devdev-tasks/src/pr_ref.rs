//! Parse PR references from strings.
//!
//! # Supported syntaxes
//!
//! * Shorthand: `owner/repo#123` (assumed `github.com`).
//! * GitHub.com: `https://github.com/owner/repo/pull/123`
//! * GitHub Enterprise: `https://<ghe-host>/owner/repo/pull/123`
//!   (any host classified as GitHub by [`RepoHostId::classify_host`]
//!   that isn't `github.com`).
//! * Azure DevOps Services:
//!   `https://dev.azure.com/{org}/{project}/_git/{repo}/pullrequest/{id}`
//! * Legacy Azure DevOps:
//!   `https://{org}.visualstudio.com/{project}/_git/{repo}/pullrequest/{id}`
//!
//! For ADO, `(org, project, repo)` is collapsed into the trait's
//! `(owner, repo)` slot as `owner = "{org}/{project}"`, `repo = "{repo}"`.
//! This matches the encoding used by `AzureDevOpsAdapter` (see the
//! mappings table in that module).
//!
//! Every `PrRef` carries a [`RepoHostId`] so downstream callers can
//! route to the correct adapter and form ledger keys.

use devdev_integrations::host::{RepoHostId, RepoHostKind};

use crate::task::TaskError;

/// Parsed PR reference. `host_id` identifies the forge instance;
/// `owner` and `repo` are interpreted in that forge's idiom (see the
/// module rustdoc for the ADO encoding).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    pub host_id: RepoHostId,
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    /// Parse a PR reference. See the module rustdoc for accepted
    /// syntaxes.
    pub fn parse(input: &str) -> Result<Self, TaskError> {
        let input = input.trim();

        if input.starts_with("https://") || input.starts_with("http://") {
            return Self::parse_url(input);
        }

        Self::parse_shorthand(input)
    }

    /// Shorthand always means github.com — there's no concise host-
    /// disambiguating syntax for GHE/ADO, and the agent should be
    /// passing full URLs anyway in those cases.
    fn parse_shorthand(input: &str) -> Result<Self, TaskError> {
        let Some((repo_part, number_str)) = input.split_once('#') else {
            return Err(TaskError::PollFailed(format!(
                "invalid PR reference: {input} (expected owner/repo#number)"
            )));
        };

        let Some((owner, repo)) = repo_part.split_once('/') else {
            return Err(TaskError::PollFailed(format!(
                "invalid PR reference: {input} (expected owner/repo#number)"
            )));
        };

        let number: u64 = number_str
            .parse()
            .map_err(|_| TaskError::PollFailed(format!("invalid PR number: {number_str}")))?;

        if owner.is_empty() || repo.is_empty() || number == 0 {
            return Err(TaskError::PollFailed(format!(
                "invalid PR reference: {input}"
            )));
        }

        Ok(Self {
            host_id: RepoHostId::github_com(),
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        })
    }

    fn parse_url(input: &str) -> Result<Self, TaskError> {
        let after_scheme = input
            .strip_prefix("https://")
            .or_else(|| input.strip_prefix("http://"))
            .expect("caller checked scheme");

        let (host, path) = match after_scheme.split_once('/') {
            Some((h, p)) => (h, p),
            None => {
                return Err(TaskError::PollFailed(format!(
                    "invalid PR URL: {input} (missing path)"
                )));
            }
        };

        let host_id = RepoHostId::from_browse_host(host).ok_or_else(|| {
            TaskError::PollFailed(format!("unsupported PR URL host: {host} (in {input})"))
        })?;

        match host_id.kind {
            RepoHostKind::GitHub => Self::parse_github_path(host_id, path, input),
            RepoHostKind::AzureDevOps => Self::parse_ado_path(host_id, host, path, input),
        }
    }

    /// `owner/repo/pull/{number}` (trailing path segments allowed).
    fn parse_github_path(
        host_id: RepoHostId,
        path: &str,
        input: &str,
    ) -> Result<Self, TaskError> {
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() < 4 || parts[2] != "pull" {
            return Err(TaskError::PollFailed(format!(
                "invalid GitHub PR URL: {input} (expected .../owner/repo/pull/number)"
            )));
        }
        let owner = parts[0];
        let repo = parts[1];
        let number: u64 = parts[3].parse().map_err(|_| {
            TaskError::PollFailed(format!("invalid PR number in URL: {}", parts[3]))
        })?;
        if owner.is_empty() || repo.is_empty() || number == 0 {
            return Err(TaskError::PollFailed(format!("invalid PR URL: {input}")));
        }
        Ok(Self {
            host_id,
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        })
    }

    /// Two ADO URL shapes are accepted:
    ///
    /// * `dev.azure.com/{org}/{project}/_git/{repo}/pullrequest/{id}`
    /// * `{org}.visualstudio.com/{project}/_git/{repo}/pullrequest/{id}`
    ///   (legacy host; org is the leftmost host label).
    fn parse_ado_path(
        host_id: RepoHostId,
        host: &str,
        path: &str,
        input: &str,
    ) -> Result<Self, TaskError> {
        let parts: Vec<&str> = path.split('/').collect();
        let lower_host = host.to_ascii_lowercase();

        let (org, project, repo, id_str) = if lower_host == "dev.azure.com" {
            if parts.len() < 6 || parts[2] != "_git" || parts[4] != "pullrequest" {
                return Err(TaskError::PollFailed(format!(
                    "invalid ADO PR URL: {input} \
                     (expected /<org>/<project>/_git/<repo>/pullrequest/<id>)"
                )));
            }
            (parts[0], parts[1], parts[3], parts[5])
        } else if lower_host.ends_with(".visualstudio.com") {
            let org = lower_host
                .strip_suffix(".visualstudio.com")
                .expect("just checked suffix");
            if parts.len() < 5 || parts[1] != "_git" || parts[3] != "pullrequest" {
                return Err(TaskError::PollFailed(format!(
                    "invalid ADO PR URL: {input} \
                     (expected /<project>/_git/<repo>/pullrequest/<id> on visualstudio.com)"
                )));
            }
            // Detach from the borrow of `lower_host` by allocating now.
            let org_owned: String = org.to_string();
            return Self::ado_finalise(host_id, &org_owned, parts[0], parts[2], parts[4], input);
        } else {
            return Err(TaskError::PollFailed(format!(
                "unrecognised ADO host: {host}"
            )));
        };

        Self::ado_finalise(host_id, org, project, repo, id_str, input)
    }

    fn ado_finalise(
        host_id: RepoHostId,
        org: &str,
        project: &str,
        repo: &str,
        id_str: &str,
        input: &str,
    ) -> Result<Self, TaskError> {
        let number: u64 = id_str
            .parse()
            .map_err(|_| TaskError::PollFailed(format!("invalid ADO PR id: {id_str}")))?;
        if org.is_empty() || project.is_empty() || repo.is_empty() || number == 0 {
            return Err(TaskError::PollFailed(format!("invalid ADO PR URL: {input}")));
        }
        Ok(Self {
            host_id,
            owner: format!("{org}/{project}"),
            repo: repo.to_string(),
            number,
        })
    }
}

impl std::fmt::Display for PrRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Display intentionally omits host so existing log lines and
        // resource ids stay shape-stable. Use `host_id.ledger_key()`
        // when host is needed.
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh() -> RepoHostId {
        RepoHostId::github_com()
    }

    // ── Shorthand ───────────────────────────────────────────────

    #[test]
    fn shorthand_defaults_to_github_com() {
        let r = PrRef::parse("owner/repo#123").unwrap();
        assert_eq!(r.host_id, gh());
        assert_eq!(r.owner, "owner");
        assert_eq!(r.repo, "repo");
        assert_eq!(r.number, 123);
    }

    #[test]
    fn shorthand_rejects_missing_hash() {
        assert!(PrRef::parse("owner/repo").is_err());
    }

    #[test]
    fn shorthand_rejects_zero_number() {
        assert!(PrRef::parse("o/r#0").is_err());
    }

    #[test]
    fn shorthand_rejects_empty_segments() {
        assert!(PrRef::parse("/r#1").is_err());
        assert!(PrRef::parse("o/#1").is_err());
        assert!(PrRef::parse("#1").is_err());
    }

    // ── GitHub.com ──────────────────────────────────────────────

    #[test]
    fn github_com_url_round_trips() {
        let r = PrRef::parse("https://github.com/o/r/pull/42").unwrap();
        assert_eq!(r.host_id, RepoHostId::github_com());
        assert_eq!(r.owner, "o");
        assert_eq!(r.repo, "r");
        assert_eq!(r.number, 42);
    }

    #[test]
    fn github_com_url_with_trailing_path_extracts_pr() {
        let r = PrRef::parse("https://github.com/o/r/pull/42/files").unwrap();
        assert_eq!(r.host_id, RepoHostId::github_com());
        assert_eq!(r.number, 42);
    }

    #[test]
    fn github_com_url_rejects_non_pull_path() {
        assert!(PrRef::parse("https://github.com/o/r/issues/42").is_err());
        assert!(PrRef::parse("https://github.com/o/r/tree/main").is_err());
    }

    // ── GHE ─────────────────────────────────────────────────────

    #[test]
    fn ghe_url_resolves_to_ghe_host_id() {
        let r = PrRef::parse("https://ghe.example.com/team/proj/pull/7").unwrap();
        assert_eq!(r.host_id, RepoHostId::ghe("ghe.example.com"));
        assert_eq!(r.host_id.api_base, "https://ghe.example.com/api/v3");
        assert_eq!(r.owner, "team");
        assert_eq!(r.repo, "proj");
        assert_eq!(r.number, 7);
    }

    #[test]
    fn ghe_url_with_github_prefix_classifies_as_github() {
        // `github.acme.io` heuristic — see `RepoHostId::classify_host`.
        let r = PrRef::parse("https://github.acme.io/o/r/pull/1").unwrap();
        assert_eq!(r.host_id.kind, RepoHostKind::GitHub);
        assert_eq!(r.host_id.host, "github.acme.io");
    }

    // ── Azure DevOps ───────────────────────────────────────────

    #[test]
    fn ado_modern_url_collapses_org_project_into_owner() {
        let r =
            PrRef::parse("https://dev.azure.com/contoso/widgets/_git/api/pullrequest/99").unwrap();
        assert_eq!(r.host_id, RepoHostId::azure_devops());
        assert_eq!(r.owner, "contoso/widgets");
        assert_eq!(r.repo, "api");
        assert_eq!(r.number, 99);
    }

    #[test]
    fn ado_modern_url_rejects_wrong_segments() {
        // Missing `_git`.
        assert!(PrRef::parse("https://dev.azure.com/c/w/api/pullrequest/1").is_err());
        // Wrong literal in pullrequest slot.
        assert!(PrRef::parse("https://dev.azure.com/c/w/_git/api/pulls/1").is_err());
    }

    #[test]
    fn ado_legacy_visualstudio_url_pulls_org_from_host() {
        let r = PrRef::parse(
            "https://contoso.visualstudio.com/widgets/_git/api/pullrequest/77",
        )
        .unwrap();
        assert_eq!(r.host_id, RepoHostId::azure_devops());
        assert_eq!(r.owner, "contoso/widgets");
        assert_eq!(r.repo, "api");
        assert_eq!(r.number, 77);
    }

    #[test]
    fn ado_legacy_url_rejects_missing_git_segment() {
        assert!(
            PrRef::parse("https://contoso.visualstudio.com/widgets/api/pullrequest/1").is_err()
        );
    }

    // ── Unknown hosts ───────────────────────────────────────────

    #[test]
    fn unknown_host_is_rejected() {
        assert!(PrRef::parse("https://gitlab.com/o/r/-/merge_requests/1").is_err());
        assert!(PrRef::parse("https://bitbucket.org/o/r/pull-requests/1").is_err());
    }

    // ── Display ────────────────────────────────────────────────

    #[test]
    fn display_omits_host_for_log_stability() {
        let r = PrRef::parse("https://ghe.example.com/o/r/pull/1").unwrap();
        assert_eq!(format!("{r}"), "o/r#1");
    }
}
