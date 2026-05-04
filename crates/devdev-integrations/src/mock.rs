//! In-memory test double for [`crate::RepoHostAdapter`].
//!
//! Host-agnostic: the same mock serves GitHub and Azure DevOps tests.
//! Construct with [`MockAdapter::new`] for a default github.com host
//! id, or [`MockAdapter::with_host`] to simulate any forge instance.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::RepoHostAdapter;
use crate::host::RepoHostId;
use crate::types::*;

type PrKey = (String, String, u64);
type PostedReview = (String, String, u64, Review);
type PostedComment = (String, String, u64, String);

/// In-memory double that returns canned responses and records
/// outgoing calls. Default host id is [`RepoHostId::github_com`].
pub struct MockAdapter {
    host_id: RepoHostId,
    prs: HashMap<PrKey, PullRequest>,
    diffs: HashMap<PrKey, String>,
    comments: HashMap<PrKey, Vec<Comment>>,
    statuses: HashMap<PrKey, PrStatus>,
    posted_reviews: Arc<Mutex<Vec<PostedReview>>>,
    posted_comments: Arc<Mutex<Vec<PostedComment>>>,
    /// SHA overrides applied via `update_head_sha` (simulates new pushes).
    sha_overrides: Arc<Mutex<HashMap<PrKey, String>>>,
}

impl MockAdapter {
    pub fn new() -> Self {
        Self::with_host(RepoHostId::github_com())
    }

    pub fn with_host(host_id: RepoHostId) -> Self {
        Self {
            host_id,
            prs: HashMap::new(),
            diffs: HashMap::new(),
            comments: HashMap::new(),
            statuses: HashMap::new(),
            posted_reviews: Arc::new(Mutex::new(Vec::new())),
            posted_comments: Arc::new(Mutex::new(Vec::new())),
            sha_overrides: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Add a canned PR response.
    pub fn with_pr(mut self, owner: &str, repo: &str, pr: PullRequest) -> Self {
        let number = pr.number;
        self.prs.insert((owner.into(), repo.into(), number), pr);
        self
    }

    /// Add a canned diff response.
    pub fn with_diff(mut self, owner: &str, repo: &str, number: u64, diff: &str) -> Self {
        self.diffs
            .insert((owner.into(), repo.into(), number), diff.into());
        self
    }

    /// Add canned comments.
    pub fn with_comments(
        mut self,
        owner: &str,
        repo: &str,
        number: u64,
        comments: Vec<Comment>,
    ) -> Self {
        self.comments
            .insert((owner.into(), repo.into(), number), comments);
        self
    }

    /// Add a canned PR status.
    pub fn with_status(mut self, owner: &str, repo: &str, number: u64, status: PrStatus) -> Self {
        self.statuses
            .insert((owner.into(), repo.into(), number), status);
        self
    }

    /// Get all reviews that were posted.
    pub fn posted_reviews(&self) -> Vec<PostedReview> {
        self.posted_reviews.lock().unwrap().clone()
    }

    /// Get all comments that were posted.
    pub fn posted_comments(&self) -> Vec<PostedComment> {
        self.posted_comments.lock().unwrap().clone()
    }

    /// Simulate a new push by changing the head SHA for a PR.
    pub fn update_head_sha(&self, owner: &str, repo: &str, number: u64, new_sha: &str) {
        self.sha_overrides
            .lock()
            .unwrap()
            .insert(key(owner, repo, number), new_sha.to_string());
    }
}

impl Default for MockAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn key(owner: &str, repo: &str, number: u64) -> PrKey {
    (owner.into(), repo.into(), number)
}

#[async_trait]
impl RepoHostAdapter for MockAdapter {
    fn host_id(&self) -> &RepoHostId {
        &self.host_id
    }

    async fn get_pr(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, RepoHostError> {
        let mut pr = self
            .prs
            .get(&key(owner, repo, number))
            .cloned()
            .ok_or_else(|| RepoHostError::NotFound(format!("{owner}/{repo}#{number}")))?;

        if let Some(sha) = self
            .sha_overrides
            .lock()
            .unwrap()
            .get(&key(owner, repo, number))
        {
            pr.head_sha = sha.clone();
        }

        Ok(pr)
    }

    async fn get_pr_diff(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, RepoHostError> {
        self.diffs
            .get(&key(owner, repo, number))
            .cloned()
            .ok_or_else(|| RepoHostError::NotFound(format!("{owner}/{repo}#{number}")))
    }

    async fn list_pr_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Vec<Comment>, RepoHostError> {
        Ok(self
            .comments
            .get(&key(owner, repo, number))
            .cloned()
            .unwrap_or_default())
    }

    async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        review: Review,
    ) -> Result<(), RepoHostError> {
        self.posted_reviews
            .lock()
            .unwrap()
            .push((owner.into(), repo.into(), number, review));
        Ok(())
    }

    async fn post_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), RepoHostError> {
        self.posted_comments
            .lock()
            .unwrap()
            .push((owner.into(), repo.into(), number, body.into()));
        Ok(())
    }

    async fn get_pr_status(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PrStatus, RepoHostError> {
        self.statuses
            .get(&key(owner, repo, number))
            .cloned()
            .ok_or_else(|| RepoHostError::NotFound(format!("{owner}/{repo}#{number}")))
    }

    async fn get_pr_head_sha(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, RepoHostError> {
        if let Some(sha) = self
            .sha_overrides
            .lock()
            .unwrap()
            .get(&key(owner, repo, number))
        {
            return Ok(sha.clone());
        }
        self.prs
            .get(&key(owner, repo, number))
            .map(|pr| pr.head_sha.clone())
            .ok_or_else(|| RepoHostError::NotFound(format!("{owner}/{repo}#{number}")))
    }

    async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<PullRequest>, RepoHostError> {
        let overrides = self.sha_overrides.lock().unwrap().clone();
        let mut out = Vec::new();
        for ((o, r, n), pr) in &self.prs {
            if o == owner && r == repo && matches!(pr.state, PrState::Open) {
                let mut pr = pr.clone();
                if let Some(sha) = overrides.get(&(o.clone(), r.clone(), *n)) {
                    pr.head_sha = sha.clone();
                }
                out.push(pr);
            }
        }
        Ok(out)
    }
}
