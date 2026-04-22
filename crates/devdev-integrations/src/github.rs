//! Live GitHub adapter using reqwest.

use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use std::env;

use crate::rate_limit::RateLimitTracker;
use crate::types::*;
use crate::GitHubAdapter;

const API_BASE: &str = "https://api.github.com";

/// Real GitHub API client.
pub struct LiveGitHubAdapter {
    client: reqwest::Client,
    token: String,
    rate_limit: RateLimitTracker,
}

impl LiveGitHubAdapter {
    /// Create a new adapter, reading the token from `GH_TOKEN`.
    pub fn from_env() -> Result<Self, GitHubError> {
        let token = env::var("GH_TOKEN").map_err(|_| GitHubError::TokenNotSet)?;
        Ok(Self {
            client: reqwest::Client::new(),
            token,
            rate_limit: RateLimitTracker::new(),
        })
    }

    /// Create a new adapter with an explicit token.
    pub fn with_token(token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            token,
            rate_limit: RateLimitTracker::new(),
        }
    }

    /// Current rate-limit tracker.
    pub fn rate_limit(&self) -> &RateLimitTracker {
        &self.rate_limit
    }

    /// Send a GET request and handle common errors.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, GitHubError> {
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
            .map_err(|e| GitHubError::Deserialize(format!("{e}: {text}")))
    }

    /// Send a GET and return raw text (for diffs).
    async fn get_text(&self, url: &str, accept: &str) -> Result<String, GitHubError> {
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
    ) -> Result<(), GitHubError> {
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

    fn check_status(&self, resp: &reqwest::Response) -> Result<(), GitHubError> {
        let status = resp.status().as_u16();
        match status {
            200..=299 => Ok(()),
            401 => Err(GitHubError::Unauthorized),
            404 => Err(GitHubError::NotFound(resp.url().to_string())),
            429 => {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(60);
                Err(GitHubError::RateLimited { retry_after })
            }
            _ => Err(GitHubError::ServerError {
                status,
                body: String::new(),
            }),
        }
    }
}

/// Parse the GitHub API PR JSON response into our `PullRequest` type.
fn parse_pr(value: serde_json::Value) -> Result<PullRequest, GitHubError> {
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
        title: value["title"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        author: value["user"]["login"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        state,
        head_sha: value["head"]["sha"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        base_sha: value["base"]["sha"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        head_ref: value["head"]["ref"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        base_ref: value["base"]["ref"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        body: value["body"].as_str().map(String::from),
        created_at: value["created_at"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        updated_at: value["updated_at"]
            .as_str()
            .unwrap_or("")
            .to_string(),
    })
}

fn parse_comment(value: &serde_json::Value) -> Comment {
    Comment {
        id: value["id"].as_u64().unwrap_or(0),
        author: value["user"]["login"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        body: value["body"].as_str().unwrap_or("").to_string(),
        path: value["path"].as_str().map(String::from),
        line: value["line"].as_u64(),
        created_at: value["created_at"]
            .as_str()
            .unwrap_or("")
            .to_string(),
    }
}

#[async_trait]
impl GitHubAdapter for LiveGitHubAdapter {
    async fn get_pr(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, GitHubError> {
        let url = format!("{API_BASE}/repos/{owner}/{repo}/pulls/{number}");
        let value: serde_json::Value = self.get_json(&url).await?;
        parse_pr(value)
    }

    async fn get_pr_diff(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, GitHubError> {
        let url = format!("{API_BASE}/repos/{owner}/{repo}/pulls/{number}");
        self.get_text(&url, "application/vnd.github.v3.diff")
            .await
    }

    async fn list_pr_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Vec<Comment>, GitHubError> {
        let mut all_comments = Vec::new();
        let mut page = 1u32;
        let max_pages = 10;

        loop {
            let url = format!(
                "{API_BASE}/repos/{owner}/{repo}/pulls/{number}/comments?per_page=100&page={page}"
            );
            let value: serde_json::Value = self.get_json(&url).await?;

            let arr = value
                .as_array()
                .ok_or_else(|| GitHubError::Deserialize("expected array".into()))?;

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
    ) -> Result<(), GitHubError> {
        let url =
            format!("{API_BASE}/repos/{owner}/{repo}/pulls/{number}/reviews");

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
    ) -> Result<(), GitHubError> {
        let url =
            format!("{API_BASE}/repos/{owner}/{repo}/issues/{number}/comments");
        let payload = serde_json::json!({ "body": body });
        self.post_json(&url, &payload).await
    }

    async fn get_pr_status(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PrStatus, GitHubError> {
        // Get PR for mergeable status
        let pr_url = format!("{API_BASE}/repos/{owner}/{repo}/pulls/{number}");
        let pr_value: serde_json::Value = self.get_json(&pr_url).await?;
        let mergeable = pr_value["mergeable"].as_bool();

        // Get check runs for the head SHA
        let head_sha = pr_value["head"]["sha"]
            .as_str()
            .unwrap_or("");
        let checks_url =
            format!("{API_BASE}/repos/{owner}/{repo}/commits/{head_sha}/check-runs");
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
    ) -> Result<String, GitHubError> {
        let url = format!("{API_BASE}/repos/{owner}/{repo}/pulls/{number}");
        let value: serde_json::Value = self.get_json(&url).await?;
        Ok(value["head"]["sha"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }
}
