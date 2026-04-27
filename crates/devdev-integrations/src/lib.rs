//! GitHub integration adapter for DevDev.
//!
//! Provides the `GitHubAdapter` trait and a `MockGitHubAdapter` for testing.
//! The `LiveGitHubAdapter` performs real HTTP calls to the GitHub REST API.

pub mod github;
pub mod github_mock;
pub mod rate_limit;
pub mod types;

pub use github::LiveGitHubAdapter;
pub use github_mock::MockGitHubAdapter;
pub use types::*;

use async_trait::async_trait;

/// Abstract interface for GitHub API operations.
#[async_trait]
pub trait GitHubAdapter: Send + Sync {
    /// Fetch PR metadata.
    async fn get_pr(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, GitHubError>;

    /// Fetch the unified diff for a PR.
    async fn get_pr_diff(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, GitHubError>;

    /// List all review comments on a PR.
    async fn list_pr_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Vec<Comment>, GitHubError>;

    /// Post a full review (approve, request changes, or comment).
    async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        review: Review,
    ) -> Result<(), GitHubError>;

    /// Post a single comment on a PR.
    async fn post_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), GitHubError>;

    /// Get PR merge status and CI check runs.
    async fn get_pr_status(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PrStatus, GitHubError>;

    /// Get the head SHA of the PR (for detecting new pushes).
    async fn get_pr_head_sha(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, GitHubError>;

    /// List open PRs in a repo. Adapters paginate internally; callers
    /// receive the flat union. Used by `RepoWatchTask` to discover
    /// new PRs without webhooks.
    async fn list_open_prs(&self, owner: &str, repo: &str)
    -> Result<Vec<PullRequest>, GitHubError>;
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
