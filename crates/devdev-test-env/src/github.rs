//! GitHub fixture provisioner.
//!
//! Implements the idempotent `apply` / `verify` / `reset-comments`
//! ops for the GitHub side of the manifest. Talks to the github.com
//! REST API directly (the `octocrab` crate would pull a non-trivial
//! dep tree for not much benefit at this scope).
//!
//! Authentication: a GitHub fine-grained PAT with `Contents: write`,
//! `Pull requests: write`, `Issues: write`, and `Administration:
//! write` (last only needed if the repo doesn't yet exist) on the
//! fixture org.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use reqwest::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::manifest::{CanonicalPr, GithubFixture, GithubLock};

const API_BASE: &str = "https://api.github.com";
const UA: &str = "devdev-test-env/0.1";

/// Authenticated client for the GitHub REST API. Each method is
/// idempotent: it reads first, only writes if state diverges.
pub struct GithubClient {
    http: Client,
    token: String,
}

impl GithubClient {
    pub fn new(token: String) -> anyhow::Result<Self> {
        let http = Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http, token })
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github+json")
            .header(USER_AGENT, UA)
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    /// Apply the manifest's GitHub side. Returns the resolved
    /// [`GithubLock`] (PR number etc.).
    pub async fn apply(&self, fixture: &GithubFixture) -> anyhow::Result<GithubLock> {
        let repo = self.ensure_repo(&fixture.org, &fixture.repo).await?;
        let default_sha = self
            .branch_head_sha(&fixture.org, &fixture.repo, &fixture.default_branch)
            .await?;
        self.ensure_fixture_branch(
            &fixture.org,
            &fixture.repo,
            &fixture.fixture_branch,
            &default_sha,
        )
        .await?;
        self.ensure_fixture_files(
            &fixture.org,
            &fixture.repo,
            &fixture.fixture_branch,
            &fixture.canonical_pr,
        )
        .await?;
        let (pr_number, pr_node_id) = self
            .ensure_canonical_pr(
                &fixture.org,
                &fixture.repo,
                &fixture.fixture_branch,
                &fixture.canonical_pr,
            )
            .await?;

        Ok(GithubLock {
            repo_id: repo.id,
            canonical_pr_number: pr_number,
            canonical_pr_node_id: pr_node_id,
        })
    }

    /// Read-only: returns Ok if the manifest matches reality byte-
    /// for-byte. Non-zero exit otherwise.
    pub async fn verify(
        &self,
        fixture: &GithubFixture,
        lock: &GithubLock,
    ) -> anyhow::Result<()> {
        let pr: PrResponse = self
            .auth(self.http.get(format!(
                "{API_BASE}/repos/{}/{}/pulls/{}",
                fixture.org, fixture.repo, lock.canonical_pr_number
            )))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if pr.title != fixture.canonical_pr.title {
            anyhow::bail!(
                "github canonical PR title drifted: manifest={:?}, live={:?}",
                fixture.canonical_pr.title,
                pr.title
            );
        }
        if pr.body.as_deref().unwrap_or("") != fixture.canonical_pr.body {
            anyhow::bail!("github canonical PR body drifted from manifest");
        }
        if pr.state != "open" {
            anyhow::bail!("github canonical PR is not open: state={}", pr.state);
        }
        Ok(())
    }

    async fn ensure_repo(&self, org: &str, repo: &str) -> anyhow::Result<RepoResponse> {
        let url = format!("{API_BASE}/repos/{org}/{repo}");
        let resp = self.auth(self.http.get(&url)).send().await?;
        if resp.status().is_success() {
            return Ok(resp.json().await?);
        }
        if resp.status().as_u16() != 404 {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("GET {url} returned {status}: {body}");
        }
        // Create.
        let create = self
            .auth(self.http.post(format!("{API_BASE}/orgs/{org}/repos")))
            .json(&json!({
                "name": repo,
                "private": false,
                "auto_init": true,
                "description": "DevDev live-test fixture; managed by devdev-test-env",
            }))
            .send()
            .await?
            .error_for_status()?
            .json::<RepoResponse>()
            .await?;
        Ok(create)
    }

    async fn branch_head_sha(
        &self,
        org: &str,
        repo: &str,
        branch: &str,
    ) -> anyhow::Result<String> {
        let url = format!("{API_BASE}/repos/{org}/{repo}/git/ref/heads/{branch}");
        let v: Value = self
            .auth(self.http.get(&url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        v["object"]["sha"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("missing object.sha in {url}"))
    }

    async fn ensure_fixture_branch(
        &self,
        org: &str,
        repo: &str,
        branch: &str,
        from_sha: &str,
    ) -> anyhow::Result<()> {
        let url = format!("{API_BASE}/repos/{org}/{repo}/git/ref/heads/{branch}");
        let resp = self.auth(self.http.get(&url)).send().await?;
        if resp.status().is_success() {
            return Ok(());
        }
        if resp.status().as_u16() != 404 {
            anyhow::bail!("GET {url} returned {}", resp.status());
        }
        self.auth(self.http.post(format!("{API_BASE}/repos/{org}/{repo}/git/refs")))
            .json(&json!({ "ref": format!("refs/heads/{branch}"), "sha": from_sha }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn ensure_fixture_files(
        &self,
        org: &str,
        repo: &str,
        branch: &str,
        canonical: &CanonicalPr,
    ) -> anyhow::Result<()> {
        for file in &canonical.fixture_files {
            let path = &file.path;
            let url = format!("{API_BASE}/repos/{org}/{repo}/contents/{path}?ref={branch}");
            let head = self.auth(self.http.get(&url)).send().await?;
            let (existing_sha, existing_b64) = if head.status().is_success() {
                let v: ContentsResponse = head.json().await?;
                (Some(v.sha), v.content)
            } else if head.status().as_u16() == 404 {
                (None, String::new())
            } else {
                anyhow::bail!("GET {url} returned {}", head.status());
            };

            let want_b64 = B64.encode(file.contents.as_bytes());
            // GitHub returns content with newlines every 60 chars; normalise before compare.
            if existing_b64.replace('\n', "") == want_b64 && existing_sha.is_some() {
                continue;
            }

            let mut body = json!({
                "message": format!("devdev-test-env: ensure fixture file {path}"),
                "content": want_b64,
                "branch": branch,
            });
            if let Some(sha) = existing_sha {
                body["sha"] = Value::String(sha);
            }
            self.auth(
                self.http
                    .put(format!("{API_BASE}/repos/{org}/{repo}/contents/{path}")),
            )
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        }
        Ok(())
    }

    async fn ensure_canonical_pr(
        &self,
        org: &str,
        repo: &str,
        head: &str,
        canonical: &CanonicalPr,
    ) -> anyhow::Result<(u64, String)> {
        // List open PRs with matching head ref.
        let url = format!(
            "{API_BASE}/repos/{org}/{repo}/pulls?state=open&head={org}:{head}&per_page=10"
        );
        let prs: Vec<PrResponse> = self
            .auth(self.http.get(&url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if let Some(pr) = prs.into_iter().find(|p| p.title == canonical.title) {
            // Body drift is fixed up here, not a verify failure.
            if pr.body.as_deref().unwrap_or("") != canonical.body {
                self.auth(self.http.patch(format!(
                    "{API_BASE}/repos/{org}/{repo}/pulls/{}",
                    pr.number
                )))
                .json(&json!({ "body": canonical.body }))
                .send()
                .await?
                .error_for_status()?;
            }
            return Ok((pr.number, pr.node_id));
        }

        // Create.
        let pr: PrResponse = self
            .auth(
                self.http
                    .post(format!("{API_BASE}/repos/{org}/{repo}/pulls")),
            )
            .json(&json!({
                "title": canonical.title,
                "body": canonical.body,
                "head": head,
                "base": canonical.base,
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok((pr.number, pr.node_id))
    }

    /// List comment ids on the canonical PR. Used by `reset-comments`.
    pub async fn list_pr_comments(
        &self,
        org: &str,
        repo: &str,
        pr_number: u64,
    ) -> anyhow::Result<Vec<IssueComment>> {
        let url = format!("{API_BASE}/repos/{org}/{repo}/issues/{pr_number}/comments?per_page=100");
        let v: Vec<IssueComment> = self
            .auth(self.http.get(&url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(v)
    }

    pub async fn delete_issue_comment(
        &self,
        org: &str,
        repo: &str,
        comment_id: u64,
    ) -> anyhow::Result<()> {
        self.auth(self.http.delete(format!(
            "{API_BASE}/repos/{org}/{repo}/issues/comments/{comment_id}"
        )))
        .send()
        .await?
        .error_for_status()?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RepoResponse {
    pub id: u64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PrResponse {
    number: u64,
    node_id: String,
    title: String,
    state: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentsResponse {
    sha: String,
    #[serde(default)]
    content: String,
}

/// Issue comment shape. The id and author login are all `reset` needs.
#[derive(Debug, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    pub user: CommentUser,
}

#[derive(Debug, Deserialize)]
pub struct CommentUser {
    pub login: String,
}
