//! Live GitHub REST adapter.
//!
//! Covers both **GitHub.com** (`https://api.github.com`) and any
//! **GitHub Enterprise Server** instance (`https://<ghe-host>/api/v3`).
//! The two speak the same wire protocol; only the API base URL
//! differs. Construct via [`GitHubAdapter::new`] with an explicit
//! [`RepoHostId`], or via [`GitHubAdapter::github_com`] for the
//! common github.com case.

use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use std::env;

use crate::RepoHostAdapter;
use crate::host::RepoHostId;
use crate::rate_limit::RateLimitTracker;
use crate::types::*;

/// Real GitHub REST API client.
///
/// Holds an [`RepoHostId`] (so the daemon registry can key on it),
/// the API base URL, the bearer token, and a rate-limit tracker.
pub struct GitHubAdapter {
    host_id: RepoHostId,
    client: reqwest::Client,
    token: String,
    rate_limit: RateLimitTracker,
}

impl GitHubAdapter {
    /// Build a github.com adapter, reading the token from `GH_TOKEN`.
    ///
    /// Equivalent to `Self::new(RepoHostId::github_com(), token)` with
    /// the env-var lookup folded in. Provided for migration ease;
    /// production daemons should resolve credentials through the
    /// `devdev-daemon` `CredentialStore` and call [`Self::new`].
    pub fn from_env() -> Result<Self, RepoHostError> {
        let token = env::var("GH_TOKEN").map_err(|_| RepoHostError::TokenNotSet)?;
        Ok(Self::new(RepoHostId::github_com(), token))
    }

    /// Build a github.com adapter with an explicit token.
    pub fn github_com(token: String) -> Self {
        Self::new(RepoHostId::github_com(), token)
    }

    /// Build an adapter for a GitHub Enterprise Server install at
    /// `host` (e.g. `ghe.example.com`).
    pub fn ghe(host: impl Into<String>, token: String) -> Self {
        Self::new(RepoHostId::ghe(host), token)
    }

    /// Construct directly from a host id and token.
    pub fn new(host_id: RepoHostId, token: String) -> Self {
        Self {
            host_id,
            client: reqwest::Client::new(),
            token,
            rate_limit: RateLimitTracker::new(),
        }
    }

    /// Current rate-limit tracker.
    pub fn rate_limit(&self) -> &RateLimitTracker {
        &self.rate_limit
    }

    fn api_base(&self) -> &str {
        &self.host_id.api_base
    }

    /// Send a GET request and handle common errors.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, RepoHostError> {
        let resp = self
            .client
            .get(url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github.v3+json")
            .header(USER_AGENT, "devdev/0.1")
            .send()
            .await?;

        self.update_rate_limit(&resp);
        self.check_status(&resp)?;

        let text = resp.text().await?;
        serde_json::from_str(&text)
            .map_err(|e| RepoHostError::Deserialize(format!("{e}: {text}")))
    }

    /// Send a GET and return raw text (for diffs).
    async fn get_text(&self, url: &str, accept: &str) -> Result<String, RepoHostError> {
        let resp = self
            .client
            .get(url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, accept)
            .header(USER_AGENT, "devdev/0.1")
            .send()
            .await?;

        self.update_rate_limit(&resp);
        self.check_status(&resp)?;

        Ok(resp.text().await?)
    }

    /// Send a POST with JSON body.
    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<(), RepoHostError> {
        let resp = self
            .client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(ACCEPT, "application/vnd.github.v3+json")
            .header(USER_AGENT, "devdev/0.1")
            .json(body)
            .send()
            .await?;

        self.update_rate_limit(&resp);
        self.check_status(&resp)?;

        Ok(())
    }

    fn update_rate_limit(&self, resp: &reqwest::Response) {
        let remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(u64::MAX);
        let reset = resp
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        self.rate_limit.update(remaining, reset);
    }

    fn check_status(&self, resp: &reqwest::Response) -> Result<(), RepoHostError> {
        let status = resp.status().as_u16();
        match status {
            200..=299 => Ok(()),
            401 => Err(RepoHostError::Unauthorized),
            404 => Err(RepoHostError::NotFound(resp.url().to_string())),
            429 => {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60);
                Err(RepoHostError::RateLimited { retry_after })
            }
            _ => Err(RepoHostError::ServerError {
                status,
                body: String::new(),
            }),
        }
    }
}

/// Parse a PR JSON response into [`PullRequest`].
fn parse_pr(value: serde_json::Value) -> Result<PullRequest, RepoHostError> {
    let merged = value
        .get("merged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let state_str = value
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("open");
    let state = if merged {
        PrState::Merged
    } else if state_str == "closed" {
        PrState::Closed
    } else {
        PrState::Open
    };

    Ok(PullRequest {
        number: value["number"].as_u64().unwrap_or(0),
        title: value["title"].as_str().unwrap_or("").to_string(),
        author: value["user"]["login"].as_str().unwrap_or("").to_string(),
        state,
        head_sha: value["head"]["sha"].as_str().unwrap_or("").to_string(),
        base_sha: value["base"]["sha"].as_str().unwrap_or("").to_string(),
        head_ref: value["head"]["ref"].as_str().unwrap_or("").to_string(),
        base_ref: value["base"]["ref"].as_str().unwrap_or("").to_string(),
        body: value["body"].as_str().map(String::from),
        created_at: value["created_at"].as_str().unwrap_or("").to_string(),
        updated_at: value["updated_at"].as_str().unwrap_or("").to_string(),
    })
}

fn parse_comment(value: &serde_json::Value) -> Comment {
    Comment {
        id: value["id"].as_u64().unwrap_or(0),
        author: value["user"]["login"].as_str().unwrap_or("").to_string(),
        body: value["body"].as_str().unwrap_or("").to_string(),
        path: value["path"].as_str().map(String::from),
        line: value["line"].as_u64(),
        created_at: value["created_at"].as_str().unwrap_or("").to_string(),
    }
}

#[async_trait]
impl RepoHostAdapter for GitHubAdapter {
    fn host_id(&self) -> &RepoHostId {
        &self.host_id
    }

    async fn get_pr(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, RepoHostError> {
        let url = format!("{}/repos/{owner}/{repo}/pulls/{number}", self.api_base());
        let value: serde_json::Value = self.get_json(&url).await?;
        parse_pr(value)
    }

    async fn get_pr_diff(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, RepoHostError> {
        let url = format!("{}/repos/{owner}/{repo}/pulls/{number}", self.api_base());
        self.get_text(&url, "application/vnd.github.v3.diff").await
    }

    async fn list_pr_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Vec<Comment>, RepoHostError> {
        let mut all_comments = Vec::new();
        let mut page = 1u32;
        let max_pages = 10;

        loop {
            let url = format!(
                "{}/repos/{owner}/{repo}/pulls/{number}/comments?per_page=100&page={page}",
                self.api_base()
            );
            let value: serde_json::Value = self.get_json(&url).await?;

            let arr = value
                .as_array()
                .ok_or_else(|| RepoHostError::Deserialize("expected array".into()))?;

            if arr.is_empty() {
                break;
            }

            for item in arr {
                all_comments.push(parse_comment(item));
            }

            page += 1;
            if page > max_pages as u32 {
                break;
            }
        }

        Ok(all_comments)
    }

    async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        review: Review,
    ) -> Result<(), RepoHostError> {
        let url = format!(
            "{}/repos/{owner}/{repo}/pulls/{number}/reviews",
            self.api_base()
        );

        let event = match review.event {
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
            ReviewEvent::Comment => "COMMENT",
        };

        let comments: Vec<serde_json::Value> = review
            .comments
            .iter()
            .map(|c| {
                serde_json::json!({
                    "path": c.path,
                    "line": c.line,
                    "body": c.body,
                })
            })
            .collect();

        let body = serde_json::json!({
            "event": event,
            "body": review.body,
            "comments": comments,
        });

        self.post_json(&url, &body).await
    }

    async fn post_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), RepoHostError> {
        let url = format!(
            "{}/repos/{owner}/{repo}/issues/{number}/comments",
            self.api_base()
        );
        let payload = serde_json::json!({ "body": body });
        self.post_json(&url, &payload).await
    }

    async fn get_pr_status(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PrStatus, RepoHostError> {
        // Get PR for mergeable status
        let pr_url = format!("{}/repos/{owner}/{repo}/pulls/{number}", self.api_base());
        let pr_value: serde_json::Value = self.get_json(&pr_url).await?;
        let mergeable = pr_value["mergeable"].as_bool();

        // Get check runs for the head SHA
        let head_sha = pr_value["head"]["sha"].as_str().unwrap_or("");
        let checks_url = format!(
            "{}/repos/{owner}/{repo}/commits/{head_sha}/check-runs",
            self.api_base()
        );
        let checks_value: serde_json::Value = self.get_json(&checks_url).await?;

        let checks = checks_value["check_runs"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|c| CheckRun {
                name: c["name"].as_str().unwrap_or("").to_string(),
                status: c["status"].as_str().unwrap_or("").to_string(),
                conclusion: c["conclusion"].as_str().map(String::from),
            })
            .collect();

        Ok(PrStatus { mergeable, checks })
    }

    async fn get_pr_head_sha(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, RepoHostError> {
        let url = format!("{}/repos/{owner}/{repo}/pulls/{number}", self.api_base());
        let value: serde_json::Value = self.get_json(&url).await?;
        Ok(value["head"]["sha"].as_str().unwrap_or("").to_string())
    }

    async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<PullRequest>, RepoHostError> {
        let mut all = Vec::new();
        let mut page = 1u32;
        let max_pages = 10u32;
        loop {
            let url = format!(
                "{}/repos/{owner}/{repo}/pulls?state=open&per_page=100&page={page}",
                self.api_base()
            );
            let value: serde_json::Value = self.get_json(&url).await?;
            let arr = value
                .as_array()
                .ok_or_else(|| RepoHostError::Deserialize("expected array".into()))?;
            if arr.is_empty() {
                break;
            }
            for item in arr {
                all.push(parse_pr(item.clone())?);
            }
            page += 1;
            if page > max_pages {
                break;
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_com_constructor_uses_dotcom_base() {
        let a = GitHubAdapter::github_com("tok".into());
        assert_eq!(a.host_id().host, "github.com");
        assert_eq!(a.api_base(), "https://api.github.com");
    }

    #[test]
    fn ghe_constructor_uses_v3_path() {
        let a = GitHubAdapter::ghe("ghe.example.com", "tok".into());
        assert_eq!(a.host_id().host, "ghe.example.com");
        assert_eq!(a.api_base(), "https://ghe.example.com/api/v3");
    }
}
