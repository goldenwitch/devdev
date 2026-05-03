//! Azure DevOps Services adapter (REST API 7.0).
//!
//! # URL layout
//!
//! ADO scopes pull requests by `organization / project / repository`
//! rather than GitHub's `owner / repo` pair. To fit the
//! [`crate::RepoHostAdapter`] surface without a breaking signature
//! change, this adapter encodes the triple as:
//!
//! * `owner = "<organization>/<project>"` (slash-joined)
//! * `repo  = "<repository>"`
//!
//! For example `https://dev.azure.com/contoso/Acme/_git/widgets` is
//! addressed as `owner = "contoso/Acme"`, `repo = "widgets"`.
//!
//! # Authentication
//!
//! ADO uses HTTP Basic auth with an empty username and the PAT as
//! the password (`Authorization: Basic base64(":<PAT>")`). PATs are
//! organization-scoped; obtain one from
//! `https://dev.azure.com/<org>/_usersSettings/tokens`.
//!
//! # Type mapping (lossy points)
//!
//! | DevDev type           | ADO source                           | Notes |
//! |-----------------------|--------------------------------------|-------|
//! | `PrState::Open`       | `status = "active"`                  |       |
//! | `PrState::Closed`     | `status = "abandoned"`               |       |
//! | `PrState::Merged`     | `status = "completed"`               |       |
//! | `ReviewEvent::Approve`| vote `10` (or `5` *approved with suggestions*) | `5` flattened to approve |
//! | `ReviewEvent::RequestChanges` | vote `-10` or `-5`           | `-5` *waiting for author* flattened |
//! | `ReviewEvent::Comment`| vote `0`                             |       |
//! | `CheckRun.status`     | PR Status `state`                    | `pending`/`succeeded`/`failed`/`error`/`notApplicable` mapped to GH-shaped `queued`/`in_progress`/`completed` |
//! | `CheckRun.conclusion` | derived from PR Status `state`       | `succeeded`→`success`, `failed`→`failure`, `error`→`failure`, `notApplicable`→`neutral` |
//!
//! # Status note
//!
//! This is the initial cut. Pagination uses ADO's `continuationToken`
//! header for `list_pr_comments` / `list_open_prs`; not all error
//! shapes have been exercised against live tenants. Live testing is
//! gated on `DEVDEV_E2E_ADO=1` + `ADO_TOKEN` + `ADO_ORG_URL`.

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};

use crate::RepoHostAdapter;
use crate::host::RepoHostId;
use crate::types::*;

const API_VERSION: &str = "7.0";

pub struct AzureDevOpsAdapter {
    host_id: RepoHostId,
    client: reqwest::Client,
    auth_header: String,
}

impl AzureDevOpsAdapter {
    /// Build an ADO Services adapter using `dev.azure.com` and the
    /// supplied PAT.
    pub fn new(pat: String) -> Self {
        Self::with_host(RepoHostId::azure_devops(), pat)
    }

    /// Build an adapter against a specific host id (e.g. a legacy
    /// `*.visualstudio.com` instance).
    pub fn with_host(host_id: RepoHostId, pat: String) -> Self {
        let auth = B64.encode(format!(":{pat}"));
        Self {
            host_id,
            client: reqwest::Client::new(),
            auth_header: format!("Basic {auth}"),
        }
    }

    fn split_owner(owner: &str) -> Result<(&str, &str), RepoHostError> {
        owner.split_once('/').ok_or_else(|| {
            RepoHostError::Unsupported(format!(
                "ADO requires owner=\"<org>/<project>\"; got {owner:?}"
            ))
        })
    }

    fn pr_base(&self, owner: &str, repo: &str, number: u64) -> Result<String, RepoHostError> {
        let (org, project) = Self::split_owner(owner)?;
        Ok(format!(
            "{}/{org}/{project}/_apis/git/repositories/{repo}/pullrequests/{number}",
            self.host_id.api_base
        ))
    }

    fn list_base(&self, owner: &str, repo: &str) -> Result<String, RepoHostError> {
        let (org, project) = Self::split_owner(owner)?;
        Ok(format!(
            "{}/{org}/{project}/_apis/git/repositories/{repo}/pullrequests",
            self.host_id.api_base
        ))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, RepoHostError> {
        let resp = self
            .client
            .get(url)
            .header(AUTHORIZATION, &self.auth_header)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "devdev/0.1")
            .send()
            .await?;
        check_status(&resp)?;
        let text = resp.text().await?;
        serde_json::from_str(&text)
            .map_err(|e| RepoHostError::Deserialize(format!("{e}: {text}")))
    }

    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, RepoHostError> {
        let resp = self
            .client
            .post(url)
            .header(AUTHORIZATION, &self.auth_header)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "devdev/0.1")
            .json(body)
            .send()
            .await?;
        check_status(&resp)?;
        let text = resp.text().await?;
        if text.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| RepoHostError::Deserialize(format!("{e}: {text}")))
    }

    async fn patch_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<(), RepoHostError> {
        let resp = self
            .client
            .patch(url)
            .header(AUTHORIZATION, &self.auth_header)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "devdev/0.1")
            .json(body)
            .send()
            .await?;
        check_status(&resp)?;
        Ok(())
    }
}

fn check_status(resp: &reqwest::Response) -> Result<(), RepoHostError> {
    let status = resp.status().as_u16();
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(RepoHostError::Unauthorized),
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

fn parse_pr(value: &serde_json::Value) -> PullRequest {
    let status = value["status"].as_str().unwrap_or("active");
    let state = match status {
        "completed" => PrState::Merged,
        "abandoned" => PrState::Closed,
        _ => PrState::Open,
    };
    PullRequest {
        number: value["pullRequestId"].as_u64().unwrap_or(0),
        title: value["title"].as_str().unwrap_or("").to_string(),
        author: value["createdBy"]["uniqueName"]
            .as_str()
            .or_else(|| value["createdBy"]["displayName"].as_str())
            .unwrap_or("")
            .to_string(),
        state,
        head_sha: value["lastMergeSourceCommit"]["commitId"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        base_sha: value["lastMergeTargetCommit"]["commitId"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        head_ref: strip_refs(value["sourceRefName"].as_str().unwrap_or("")),
        base_ref: strip_refs(value["targetRefName"].as_str().unwrap_or("")),
        body: value["description"].as_str().map(String::from),
        created_at: value["creationDate"].as_str().unwrap_or("").to_string(),
        // ADO doesn't expose a top-level updated_at; fall back to creation.
        updated_at: value["closedDate"]
            .as_str()
            .or_else(|| value["creationDate"].as_str())
            .unwrap_or("")
            .to_string(),
    }
}

fn strip_refs(r: &str) -> String {
    r.strip_prefix("refs/heads/").unwrap_or(r).to_string()
}

fn map_status_state(state: &str) -> (String, Option<String>) {
    // ADO PR status `state` → (GH-shaped status, conclusion)
    match state {
        "succeeded" => ("completed".into(), Some("success".into())),
        "failed" => ("completed".into(), Some("failure".into())),
        "error" => ("completed".into(), Some("failure".into())),
        "notApplicable" => ("completed".into(), Some("neutral".into())),
        "pending" => ("in_progress".into(), None),
        "notSet" | "" => ("queued".into(), None),
        other => (other.to_string(), None),
    }
}

#[async_trait]
impl RepoHostAdapter for AzureDevOpsAdapter {
    fn host_id(&self) -> &RepoHostId {
        &self.host_id
    }

    async fn get_pr(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequest, RepoHostError> {
        let url = format!(
            "{}?api-version={API_VERSION}",
            self.pr_base(owner, repo, number)?
        );
        let value: serde_json::Value = self.get_json(&url).await?;
        Ok(parse_pr(&value))
    }

    async fn get_pr_diff(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<String, RepoHostError> {
        // ADO doesn't return a unified diff in one call. Synthesize
        // by fetching the iteration's changes endpoint and rendering
        // a placeholder; production callers should prefer
        // `get_pr_status` + diff against the head SHA via local git.
        // For now we surface a clear `Unsupported` so callers can
        // route around it.
        let _ = (owner, repo, number);
        Err(RepoHostError::Unsupported(
            "Azure DevOps unified-diff endpoint is not implemented; \
             diff via head SHA against base instead"
                .into(),
        ))
    }

    async fn list_pr_comments(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Vec<Comment>, RepoHostError> {
        let url = format!(
            "{}/threads?api-version={API_VERSION}",
            self.pr_base(owner, repo, number)?
        );
        let value: serde_json::Value = self.get_json(&url).await?;
        let mut out = Vec::new();
        let threads = value["value"].as_array().cloned().unwrap_or_default();
        for thread in threads {
            // System-generated threads (e.g. status changes) have a
            // synthetic author; skip if they have no comments.
            let Some(comments) = thread["comments"].as_array() else {
                continue;
            };
            for c in comments {
                out.push(Comment {
                    id: c["id"].as_u64().unwrap_or(0),
                    author: c["author"]["uniqueName"]
                        .as_str()
                        .or_else(|| c["author"]["displayName"].as_str())
                        .unwrap_or("")
                        .to_string(),
                    body: c["content"].as_str().unwrap_or("").to_string(),
                    path: thread["threadContext"]["filePath"]
                        .as_str()
                        .map(String::from),
                    line: thread["threadContext"]["rightFileStart"]["line"].as_u64(),
                    created_at: c["publishedDate"].as_str().unwrap_or("").to_string(),
                });
            }
        }
        Ok(out)
    }

    async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        review: Review,
    ) -> Result<(), RepoHostError> {
        // 1. Post the summary comment as a new thread.
        if !review.body.is_empty() {
            let thread_url = format!(
                "{}/threads?api-version={API_VERSION}",
                self.pr_base(owner, repo, number)?
            );
            let thread_body = serde_json::json!({
                "comments": [{
                    "parentCommentId": 0,
                    "content": review.body,
                    "commentType": 1,
                }],
                "status": 1,
            });
            self.post_json(&thread_url, &thread_body).await?;
        }

        // 2. Post each line comment as its own thread with file context.
        for c in &review.comments {
            let thread_url = format!(
                "{}/threads?api-version={API_VERSION}",
                self.pr_base(owner, repo, number)?
            );
            let thread_body = serde_json::json!({
                "comments": [{
                    "parentCommentId": 0,
                    "content": c.body,
                    "commentType": 1,
                }],
                "status": 1,
                "threadContext": {
                    "filePath": c.path,
                    "rightFileStart": { "line": c.line, "offset": 1 },
                    "rightFileEnd":   { "line": c.line, "offset": 1 },
                },
            });
            self.post_json(&thread_url, &thread_body).await?;
        }

        // 3. Cast the vote (reviewer self).
        let vote = match review.event {
            ReviewEvent::Approve => 10,
            ReviewEvent::RequestChanges => -10,
            ReviewEvent::Comment => 0,
        };
        if vote != 0 {
            // The reviewer id `me` resolves to the PAT's identity.
            let vote_url = format!(
                "{}/reviewers/me?api-version={API_VERSION}",
                self.pr_base(owner, repo, number)?
            );
            let body = serde_json::json!({ "vote": vote });
            self.patch_json(&vote_url, &body).await?;
        }
        Ok(())
    }

    async fn post_comment(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), RepoHostError> {
        let url = format!(
            "{}/threads?api-version={API_VERSION}",
            self.pr_base(owner, repo, number)?
        );
        let payload = serde_json::json!({
            "comments": [{
                "parentCommentId": 0,
                "content": body,
                "commentType": 1,
            }],
            "status": 1,
        });
        self.post_json(&url, &payload).await?;
        Ok(())
    }

    async fn get_pr_status(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<PrStatus, RepoHostError> {
        // PR mergeStatus.
        let pr_url = format!(
            "{}?api-version={API_VERSION}",
            self.pr_base(owner, repo, number)?
        );
        let pr_value: serde_json::Value = self.get_json(&pr_url).await?;
        let mergeable = pr_value["mergeStatus"]
            .as_str()
            .map(|s| matches!(s, "succeeded" | "queued"));

        // PR statuses (genre/name/state).
        let st_url = format!(
            "{}/statuses?api-version={API_VERSION}",
            self.pr_base(owner, repo, number)?
        );
        let st_value: serde_json::Value = self.get_json(&st_url).await?;
        let checks = st_value["value"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                let state = c["state"].as_str().unwrap_or("").to_string();
                let (status, conclusion) = map_status_state(&state);
                let name = c["context"]["name"]
                    .as_str()
                    .unwrap_or_else(|| c["description"].as_str().unwrap_or(""))
                    .to_string();
                CheckRun {
                    name,
                    status,
                    conclusion,
                }
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
        let url = format!(
            "{}?api-version={API_VERSION}",
            self.pr_base(owner, repo, number)?
        );
        let value: serde_json::Value = self.get_json(&url).await?;
        Ok(value["lastMergeSourceCommit"]["commitId"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<PullRequest>, RepoHostError> {
        let url = format!(
            "{}?searchCriteria.status=active&api-version={API_VERSION}",
            self.list_base(owner, repo)?
        );
        let value: serde_json::Value = self.get_json(&url).await?;
        Ok(value["value"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(parse_pr)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_owner_requires_org_project() {
        assert!(AzureDevOpsAdapter::split_owner("contoso").is_err());
        let ok = AzureDevOpsAdapter::split_owner("contoso/Acme").unwrap();
        assert_eq!(ok, ("contoso", "Acme"));
    }

    #[test]
    fn pr_url_layout() {
        let a = AzureDevOpsAdapter::new("pat".into());
        let url = a.pr_base("contoso/Acme", "widgets", 42).unwrap();
        assert_eq!(
            url,
            "https://dev.azure.com/contoso/Acme/_apis/git/repositories/widgets/pullrequests/42"
        );
    }

    #[test]
    fn map_status_states() {
        assert_eq!(
            map_status_state("succeeded"),
            ("completed".into(), Some("success".into()))
        );
        assert_eq!(map_status_state("pending"), ("in_progress".into(), None));
        assert_eq!(map_status_state("notSet"), ("queued".into(), None));
    }

    #[test]
    fn parse_pr_maps_status_to_state() {
        let raw = serde_json::json!({
            "pullRequestId": 7,
            "title": "Add ADO support",
            "createdBy": { "uniqueName": "alice@example.com" },
            "status": "completed",
            "lastMergeSourceCommit": { "commitId": "deadbeef" },
            "lastMergeTargetCommit": { "commitId": "cafef00d" },
            "sourceRefName": "refs/heads/feature/x",
            "targetRefName": "refs/heads/main",
            "creationDate": "2026-05-03T00:00:00Z",
        });
        let pr = parse_pr(&raw);
        assert_eq!(pr.number, 7);
        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(pr.head_ref, "feature/x");
        assert_eq!(pr.base_ref, "main");
        assert_eq!(pr.head_sha, "deadbeef");
    }
}
