//! Data types for GitHub API interactions.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: PrState,
    pub head_sha: String,
    pub base_sha: String,
    pub head_ref: String,
    pub base_ref: String,
    pub body: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub event: ReviewEvent,
    pub body: String,
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    pub path: String,
    pub line: u64,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrStatus {
    pub mergeable: Option<bool>,
    pub checks: Vec<CheckRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

/// Errors from GitHub API interactions.
#[derive(thiserror::Error, Debug)]
pub enum GitHubError {
    #[error("authentication failed: check GH_TOKEN")]
    Unauthorized,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("rate limited: retry after {retry_after}s")]
    RateLimited { retry_after: u64 },

    #[error("server error: {status} {body}")]
    ServerError { status: u16, body: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("token not set: GH_TOKEN environment variable is required")]
    TokenNotSet,

    #[error("deserialization error: {0}")]
    Deserialize(String),
}

impl From<reqwest::Error> for GitHubError {
    fn from(e: reqwest::Error) -> Self {
        GitHubError::Network(e.to_string())
    }
}
