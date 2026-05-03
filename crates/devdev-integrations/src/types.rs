//! Host-agnostic data types for repository forge interactions.
//!
//! These types intentionally avoid GitHub-specific vocabulary
//! (`check_runs`, `merge_commit_sha`, etc.) so the same shapes can
//! describe pull requests on GitHub.com, GitHub Enterprise, and
//! Azure DevOps. Adapter implementations are responsible for the
//! lossy mappings.

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

/// A unifying status-check record.
///
/// On GitHub this maps to a Checks API entry (`status`,
/// `conclusion`). On Azure DevOps it maps to a PR status policy
/// entry (`state`, `genre/name`). The `status` field follows the
/// GitHub vocabulary (`queued`, `in_progress`, `completed`) for
/// historical compatibility; ADO mappings are documented in the
/// `azure_devops` adapter module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

/// Errors from any [`crate::RepoHostAdapter`] implementation.
///
/// Adapter-specific status codes are mapped to the closest abstract
/// variant; the `body` field on [`RepoHostError::ServerError`]
/// preserves the wire-level detail for diagnostics.
#[derive(thiserror::Error, Debug)]
pub enum RepoHostError {
    #[error("authentication failed: token missing or invalid")]
    Unauthorized,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("rate limited: retry after {retry_after}s")]
    RateLimited { retry_after: u64 },

    #[error("server error: {status} {body}")]
    ServerError { status: u16, body: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("token not set: a credential is required for this host")]
    TokenNotSet,

    #[error("deserialization error: {0}")]
    Deserialize(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),
}

impl From<reqwest::Error> for RepoHostError {
    fn from(e: reqwest::Error) -> Self {
        RepoHostError::Network(e.to_string())
    }
}
