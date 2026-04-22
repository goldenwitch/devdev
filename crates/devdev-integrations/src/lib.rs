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
    async fn get_pr(&self, owner: &str, repo: &str, number: u64)
        -> Result<PullRequest, GitHubError>;

    /// Fetch the unified diff for a PR.
    async fn get_pr_diff(&self, owner: &str, repo: &str, number: u64)
        -> Result<String, GitHubError>;

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
}
