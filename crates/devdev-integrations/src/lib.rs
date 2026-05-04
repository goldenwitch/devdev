//! Repository-host integration adapters for DevDev.
//!
//! This crate exposes a host-agnostic [`RepoHostAdapter`] trait
//! covering the pull-request operations DevDev's tasks need:
//! fetching metadata, listing comments, posting reviews, reading
//! merge state, and discovering open PRs.
//!
//! Concrete implementations:
//! * [`GitHubAdapter`] — covers github.com **and** GitHub Enterprise
//!   Server (the wire protocol is identical; only the API base URL
//!   differs).
//! * [`AzureDevOpsAdapter`] — Azure DevOps Services REST 7.0.
//! * [`MockAdapter`] — in-memory test double, host-agnostic.
//!
//! Host routing is keyed by [`RepoHostId`] (see [`host`]). Callers
//! that already know the host construct an adapter directly; callers
//! that only have a URL (e.g. an MCP tool invocation from the agent)
//! classify the host first via [`RepoHostId::from_browse_host`] and
//! then look up the adapter in the daemon-side registry.

#![allow(clippy::result_large_err)]

pub mod azure_devops;
pub mod github;
pub mod host;
pub mod mock;
pub mod rate_limit;
pub mod types;

pub use azure_devops::AzureDevOpsAdapter;
pub use github::GitHubAdapter;
pub use host::{RepoHostId, RepoHostKind};
pub use mock::MockAdapter;
pub use types::*;

use async_trait::async_trait;

/// Abstract pull-request operations against any supported forge.
///
/// Methods take `(owner, repo, number)` for backwards compatibility
/// with the original GitHub-only surface. ADO's `org/project/repo`
/// triple is encoded as `owner = "<org>/<project>"`, `repo = "<repo>"`
/// (see [`AzureDevOpsAdapter`]). A future revision may introduce a
/// structured `RepoCoord` type.
#[async_trait]
pub trait RepoHostAdapter: Send + Sync {
    /// Identifier of the forge instance this adapter talks to. Used
    /// by the daemon registry as a routing/dedup key.
    fn host_id(&self) -> &RepoHostId;

    /// Fetch PR metadata.
    async fn get_pr(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, RepoHostError>;

    /// Fetch the unified diff for a PR.
    async fn get_pr_diff(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, RepoHostError>;

    /// List all review comments on a PR.
    async fn list_pr_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Vec<Comment>, RepoHostError>;

    /// Post a full review (approve, request changes, or comment).
    async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        review: Review,
    ) -> Result<(), RepoHostError>;

    /// Post a single comment on a PR.
    async fn post_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), RepoHostError>;

    /// Get PR merge status and CI check runs.
    async fn get_pr_status(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PrStatus, RepoHostError>;

    /// Get the head SHA of the PR (for detecting new pushes).
    async fn get_pr_head_sha(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, RepoHostError>;

    /// List open PRs in a repo. Adapters paginate internally; callers
    /// receive the flat union. Used by `RepoWatchTask` to discover
    /// new PRs without webhooks.
    async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<PullRequest>, RepoHostError>;
}

/// Stable fingerprint of a PR's reviewable state. Used as a ledger
/// `state_hash` to dedup re-reviews. We hash `head_sha + updated_at`
/// so a force-push *and* a metadata-only edit both bump the key.
pub fn pr_state_hash(pr: &PullRequest) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    pr.head_sha.hash(&mut h);
    pr.updated_at.hash(&mut h);
    format!("sha:{:x}", h.finish())
}
