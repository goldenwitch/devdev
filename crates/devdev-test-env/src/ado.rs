//! Azure DevOps fixture provisioner.
//!
//! Surface mirrors `github.rs`: idempotent `apply` / `verify` and a
//! `list_pr_threads` / `delete_thread_comment` pair for
//! `reset-comments`. ADO uses HTTP Basic auth with `base64(":<PAT>")`
//! exactly the way our `RepoHostAdapter` does.
//!
//! ADO REST is more verbose than GitHub's: branches don't exist as
//! a first-class resource (you push commits to refs); PR comments
//! live inside threads. Both quirks are abstracted away here so the
//! main binary sees a uniform interface.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use reqwest::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::manifest::{AdoFixture, AdoLock, CanonicalPr};

const API_VERSION: &str = "7.1";
const UA: &str = "devdev-test-env/0.1";

pub struct AdoClient {
    http: Client,
    basic: String,
}

impl AdoClient {
    pub fn new(pat: &str) -> anyhow::Result<Self> {
        let basic = format!("Basic {}", B64.encode(format!(":{pat}").as_bytes()));
        let http = Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http, basic })
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header(AUTHORIZATION, &self.basic)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, UA)
    }

    fn org_root(&self, fixture: &AdoFixture) -> String {
        format!(
            "https://dev.azure.com/{}/{}/_apis",
            fixture.org, fixture.project
        )
    }

    /// Apply the manifest's ADO side. Project + org are asserted to
    /// already exist; repo is created if missing; canonical PR is
    /// opened if not already open.
    pub async fn apply(&self, fixture: &AdoFixture) -> anyhow::Result<AdoLock> {
        let repo_id = self.ensure_repo(fixture).await?;
        // Seeding the fixture branch via REST `pushes` is significantly
        // hairier than github's `contents` API: ADO requires building
        // a tree object and pushing a commit. For the first cut we
        // require the fixture branch to exist (manual one-time push)
        // and limit `apply` to PR-level idempotency. Verified by
        // attempting `branch_head_sha`; if missing, `apply` fails with
        // a directive pointing at the bootstrap doc.
        let head_sha = self
            .branch_head_sha(fixture, &repo_id, &fixture.fixture_branch)
            .await?;
        let _ = self
            .branch_head_sha(fixture, &repo_id, &fixture.default_branch)
            .await?;
        let pr_id = self
            .ensure_canonical_pr(fixture, &repo_id, &head_sha)
            .await?;
        Ok(AdoLock {
            repo_id,
            canonical_pr_id: pr_id,
        })
    }

    pub async fn verify(
        &self,
        fixture: &AdoFixture,
        lock: &AdoLock,
    ) -> anyhow::Result<()> {
        let url = format!(
            "{}/git/repositories/{}/pullrequests/{}?api-version={API_VERSION}",
            self.org_root(fixture),
            lock.repo_id,
            lock.canonical_pr_id
        );
        let pr: PrResponse = self
            .auth(self.http.get(&url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if pr.title != fixture.canonical_pr.title {
            anyhow::bail!(
                "ado canonical PR title drifted: manifest={:?}, live={:?}",
                fixture.canonical_pr.title,
                pr.title
            );
        }
        if pr.description.as_deref().unwrap_or("") != fixture.canonical_pr.body {
            anyhow::bail!("ado canonical PR description drifted from manifest");
        }
        if pr.status != "active" {
            anyhow::bail!("ado canonical PR is not active: status={}", pr.status);
        }
        Ok(())
    }

    async fn ensure_repo(&self, fixture: &AdoFixture) -> anyhow::Result<String> {
        let list_url = format!(
            "{}/git/repositories?api-version={API_VERSION}",
            self.org_root(fixture)
        );
        let v: ListRepos = self
            .auth(self.http.get(&list_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(r) = v.value.iter().find(|r| r.name == fixture.repo) {
            return Ok(r.id.clone());
        }
        let create: RepoResponse = self
            .auth(self.http.post(&list_url))
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({ "name": fixture.repo }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(create.id)
    }

    async fn branch_head_sha(
        &self,
        fixture: &AdoFixture,
        repo_id: &str,
        branch: &str,
    ) -> anyhow::Result<String> {
        let url = format!(
            "{}/git/repositories/{}/refs?filter=heads/{}&api-version={API_VERSION}",
            self.org_root(fixture),
            repo_id,
            branch
        );
        let v: ListRefs = self
            .auth(self.http.get(&url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        v.value
            .into_iter()
            .find(|r| r.name == format!("refs/heads/{branch}"))
            .map(|r| r.object_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "ado branch heads/{branch} not found in repo {repo_id}; \
                     bootstrap by pushing the fixture branch manually \
                     (see docs/internals/live-test-fixtures.md)"
                )
            })
    }

    async fn ensure_canonical_pr(
        &self,
        fixture: &AdoFixture,
        repo_id: &str,
        _head_sha: &str,
    ) -> anyhow::Result<u64> {
        let list_url = format!(
            "{}/git/repositories/{}/pullrequests?searchCriteria.status=active&searchCriteria.sourceRefName=refs/heads/{}&api-version={API_VERSION}",
            self.org_root(fixture),
            repo_id,
            fixture.fixture_branch,
        );
        let list: ListPrs = self
            .auth(self.http.get(&list_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(pr) = list
            .value
            .into_iter()
            .find(|p| p.title == fixture.canonical_pr.title)
        {
            // Description drift fixed here.
            if pr.description.as_deref().unwrap_or("") != fixture.canonical_pr.body {
                self.auth(
                    self.http.patch(format!(
                        "{}/git/repositories/{}/pullrequests/{}?api-version={API_VERSION}",
                        self.org_root(fixture),
                        repo_id,
                        pr.pull_request_id,
                    )),
                )
                .header(CONTENT_TYPE, "application/json")
                .json(&json!({ "description": fixture.canonical_pr.body }))
                .send()
                .await?
                .error_for_status()?;
            }
            return Ok(pr.pull_request_id);
        }
        let body = pr_create_body(fixture, &fixture.canonical_pr);
        let create_url = format!(
            "{}/git/repositories/{}/pullrequests?api-version={API_VERSION}",
            self.org_root(fixture),
            repo_id
        );
        let pr: PrResponse = self
            .auth(self.http.post(&create_url))
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(pr.pull_request_id)
    }

    pub async fn list_pr_threads(
        &self,
        fixture: &AdoFixture,
        repo_id: &str,
        pr_id: u64,
    ) -> anyhow::Result<Vec<PrThread>> {
        let url = format!(
            "{}/git/repositories/{}/pullrequests/{}/threads?api-version={API_VERSION}",
            self.org_root(fixture),
            repo_id,
            pr_id
        );
        let v: ListThreads = self
            .auth(self.http.get(&url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(v.value)
    }

    pub async fn delete_thread_comment(
        &self,
        fixture: &AdoFixture,
        repo_id: &str,
        pr_id: u64,
        thread_id: u64,
        comment_id: u64,
    ) -> anyhow::Result<()> {
        let url = format!(
            "{}/git/repositories/{}/pullrequests/{}/threads/{}/comments/{}?api-version={API_VERSION}",
            self.org_root(fixture),
            repo_id,
            pr_id,
            thread_id,
            comment_id,
        );
        self.auth(self.http.delete(&url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

fn pr_create_body(fixture: &AdoFixture, canonical: &CanonicalPr) -> Value {
    json!({
        "sourceRefName": format!("refs/heads/{}", fixture.fixture_branch),
        "targetRefName": format!("refs/heads/{}", canonical.base),
        "title": canonical.title,
        "description": canonical.body,
    })
}

#[derive(Debug, Deserialize)]
struct ListRepos {
    value: Vec<RepoResponse>,
}

#[derive(Debug, Deserialize)]
struct RepoResponse {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ListRefs {
    value: Vec<RefResponse>,
}

#[derive(Debug, Deserialize)]
struct RefResponse {
    name: String,
    #[serde(rename = "objectId")]
    object_id: String,
}

#[derive(Debug, Deserialize)]
struct ListPrs {
    value: Vec<PrResponse>,
}

#[derive(Debug, Deserialize)]
struct PrResponse {
    #[serde(rename = "pullRequestId")]
    pull_request_id: u64,
    title: String,
    status: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListThreads {
    value: Vec<PrThread>,
}

#[derive(Debug, Deserialize)]
pub struct PrThread {
    pub id: u64,
    #[serde(default)]
    pub comments: Vec<ThreadComment>,
    /// `active`, `fixed`, `closed`, etc. We delete comments inside
    /// active threads only — closed threads are usually historical.
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ThreadComment {
    pub id: u64,
    #[serde(default)]
    pub content: String,
    pub author: ThreadCommentAuthor,
}

#[derive(Debug, Deserialize)]
pub struct ThreadCommentAuthor {
    #[serde(default, rename = "uniqueName")]
    pub unique_name: String,
}
