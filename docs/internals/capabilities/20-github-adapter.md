---
id: github-adapter
title: "GitHub Integration Adapter"
status: done
type: leaf
phase: 2
crate: devdev-integrations
priority: P0
depends-on: []
effort: L
---

# P2-05 — GitHub Integration Adapter

**New crate: `devdev-integrations`.** Adapter pattern over external services, starting with GitHub. The adapter fetches PRs, diffs, comments, posts reviews, and checks PR status — everything the MonitorPR task (P2-07) needs.

## Scope

**In:**
- `GitHubAdapter` trait: abstract interface for GitHub API operations.
- `LiveGitHubAdapter`: real implementation using GitHub REST API via `reqwest`.
- `MockGitHubAdapter`: test double that returns canned responses and records calls.
- Authentication: `GH_TOKEN` environment variable (same token as Copilot auth).
- Rate limiting: read `X-RateLimit-Remaining` headers, back off on 429, log remaining quota.
- Retry: exponential backoff on 5xx, 1 retry on network timeout.
- Data types: `PullRequest`, `PrDiff`, `Comment`, `Review`, `PrStatus`, `CheckRun`.

**Out:**
- Webhook receivers (Phase 3 — we poll for now).
- GraphQL API (REST is sufficient for PR operations).
- GitLab / Bitbucket adapters (future — the trait is the seam).
- Repository creation, issue management, or anything beyond PR operations.

## PoC Requirement (Spec Rule 2)

Before implementing review posting:

1. Test that `GH_TOKEN` (Copilot-scoped) has permission to call `POST /repos/{owner}/{repo}/pulls/{pull_number}/reviews`.
2. If not, document the required scopes.
3. Test rate limit headers are present and parseable.

**PoC Result:** _Not yet run._

## Interface

```rust
use async_trait::async_trait;

#[async_trait]
pub trait GitHubAdapter: Send + Sync {
    /// Fetch PR metadata (title, author, state, head SHA, base SHA).
    async fn get_pr(&self, owner: &str, repo: &str, number: u64) -> Result<PullRequest, GitHubError>;

    /// Fetch the unified diff for a PR.
    async fn get_pr_diff(&self, owner: &str, repo: &str, number: u64) -> Result<String, GitHubError>;

    /// List all review comments on a PR.
    async fn list_pr_comments(&self, owner: &str, repo: &str, number: u64) -> Result<Vec<Comment>, GitHubError>;

    /// Post a full review (approve, request changes, or comment).
    async fn post_review(&self, owner: &str, repo: &str, number: u64, review: Review) -> Result<(), GitHubError>;

    /// Post a single comment on a PR.
    async fn post_comment(&self, owner: &str, repo: &str, number: u64, body: &str) -> Result<(), GitHubError>;

    /// Get PR merge status and CI check runs.
    async fn get_pr_status(&self, owner: &str, repo: &str, number: u64) -> Result<PrStatus, GitHubError>;

    /// Get the head SHA of the PR (for detecting new pushes).
    async fn get_pr_head_sha(&self, owner: &str, repo: &str, number: u64) -> Result<String, GitHubError>;
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: PrState,       // Open, Closed, Merged
    pub head_sha: String,
    pub base_sha: String,
    pub head_ref: String,     // branch name
    pub base_ref: String,
    pub body: Option<String>,
    pub created_at: String,   // ISO 8601
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub path: Option<String>,     // file path for review comments
    pub line: Option<u64>,        // line number for review comments
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct Review {
    pub event: ReviewEvent,       // Approve, RequestChanges, Comment
    pub body: String,             // overall review body
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, Copy)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone)]
pub struct ReviewComment {
    pub path: String,
    pub line: u64,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct PrStatus {
    pub mergeable: Option<bool>,
    pub checks: Vec<CheckRun>,
}

#[derive(Debug, Clone)]
pub struct CheckRun {
    pub name: String,
    pub status: String,         // queued, in_progress, completed
    pub conclusion: Option<String>,  // success, failure, neutral, ...
}

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
    Network(#[from] reqwest::Error),
    #[error("token not set: GH_TOKEN environment variable is required")]
    TokenNotSet,
}
```

### Mock Adapter

```rust
pub struct MockGitHubAdapter {
    prs: HashMap<(String, String, u64), PullRequest>,
    diffs: HashMap<(String, String, u64), String>,
    comments: HashMap<(String, String, u64), Vec<Comment>>,
    posted_reviews: Arc<Mutex<Vec<(String, String, u64, Review)>>>,
    posted_comments: Arc<Mutex<Vec<(String, String, u64, String)>>>,
}

impl MockGitHubAdapter {
    pub fn new() -> Self;
    pub fn with_pr(self, owner: &str, repo: &str, pr: PullRequest) -> Self;
    pub fn with_diff(self, owner: &str, repo: &str, number: u64, diff: &str) -> Self;
    pub fn posted_reviews(&self) -> Vec<(String, String, u64, Review)>;
    pub fn posted_comments(&self) -> Vec<(String, String, u64, String)>;
}
```

## Implementation Notes

- **HTTP client:** `reqwest::Client` with `GH_TOKEN` in `Authorization: Bearer <token>` header. Accept `application/vnd.github.v3+json` for JSON, `application/vnd.github.v3.diff` for diff.
- **Rate limiting:** After every response, check `X-RateLimit-Remaining`. If below 10, log a warning. On 429 response, read `Retry-After` header, sleep that duration, retry once.
- **Pagination:** PR comments can be paginated. Follow `Link: <url>; rel="next"` headers. Cap at 10 pages (guard against runaway).
- **Review posting endpoint:** `POST /repos/{owner}/{repo}/pulls/{pull_number}/reviews` with `{"event": "COMMENT", "body": "...", "comments": [...]}`.
- **Security:** Never log the token. Sanitize token from error messages. Use HTTPS only.

## Files

```
crates/devdev-integrations/Cargo.toml
crates/devdev-integrations/src/lib.rs           — re-exports, GitHubAdapter trait
crates/devdev-integrations/src/github.rs        — LiveGitHubAdapter
crates/devdev-integrations/src/github_mock.rs   — MockGitHubAdapter
crates/devdev-integrations/src/types.rs         — PullRequest, Comment, Review, etc.
crates/devdev-integrations/src/rate_limit.rs    — Rate limit tracking, backoff
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-05-1 | §3.4 | GitHubAdapter trait: get PR, get diff, list comments, post review, post comment, get status |
| SR-05-2 | §3.4 | Authentication via GH_TOKEN |
| SR-05-3 | §3.4 | Rate limiting: respect 429, back off, log remaining quota |
| SR-05-4 | §3.4 | No webhooks — Phase 2 polls |
| SR-05-5 | §4 (GitHub Adapter row) | Integration tests gated behind DEVDEV_E2E |
| SR-05-6 | §4 (GitHub Adapter row) | Unit tests with recorded HTTP responses (wiremock) |
| SR-05-7 | Open Question #4 | PoC: validate GH_TOKEN scope covers PR API |

## Acceptance Tests

### Unit (with wiremock)

- [ ] `get_pr_returns_metadata` — mock endpoint returns JSON → adapter parses PullRequest correctly
- [ ] `get_pr_not_found` — mock 404 → `GitHubError::NotFound`
- [ ] `get_pr_diff_returns_unified` — mock diff endpoint → adapter returns diff string
- [ ] `list_comments_paginates` — mock two pages → adapter follows Link header, returns all comments
- [ ] `post_review_sends_correct_body` — call post_review → verify request body matches expected JSON
- [ ] `post_comment_sends_correct_body` — same for single comment
- [ ] `rate_limit_429_retries` — mock 429 with Retry-After → adapter waits and retries
- [ ] `rate_limit_low_remaining_logs_warning` — mock low remaining → verify log output
- [ ] `token_not_set_errors` — unset GH_TOKEN → `GitHubError::TokenNotSet`
- [ ] `unauthorized_401_errors` — mock 401 → `GitHubError::Unauthorized`
- [ ] `server_error_retries_once` — mock 500, then 200 → adapter retries and succeeds
- [ ] `get_pr_head_sha_returns_sha` — verify head SHA extraction from PR response

### Mock Adapter

- [ ] `mock_records_posted_reviews` — call post_review on mock → `posted_reviews()` contains it
- [ ] `mock_records_posted_comments` — same for comments
- [ ] `mock_returns_configured_pr` — configure with_pr → get_pr returns it

### E2E (gated behind DEVDEV_E2E)

- [ ] `e2e_fetch_real_pr` — fetch a known public PR → verify fields populated
- [ ] `e2e_fetch_real_diff` — fetch diff → non-empty string
- [ ] `e2e_post_and_delete_comment` — post a test comment, verify it exists, delete it (cleanup)

## Spec Compliance Checklist

- [ ] SR-05-1 through SR-05-7: all requirements covered
- [ ] PoC result recorded for GH_TOKEN scope
- [ ] All acceptance tests passing
