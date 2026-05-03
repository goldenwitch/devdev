//! Concrete [`McpToolProvider`] backed by the daemon's live state.
//!
//! This is the production bridge between DevDev's long-lived daemon
//! structures and the MCP tools exposed over loopback HTTP. Separate
//! from `tools.rs` so tests in that module can continue to exercise
//! the server skeleton with just a `StaticProvider`.

use std::sync::Arc;

use async_trait::async_trait;
use devdev_integrations::host::RepoHostId;
use devdev_tasks::approval::{ApprovalError, ApprovalGate};
use devdev_tasks::registry::TaskRegistry;
use devdev_workspace::Fs;
use tokio::sync::Mutex;

use crate::credentials::CredentialStore;
use crate::mcp::{AskKind, AskRequest, AskResponse, McpProviderError, McpToolProvider, TaskInfo};

/// Wraps the daemon's shared `Arc<Mutex<TaskRegistry>>` and
/// `Arc<Mutex<Fs>>` so the MCP server can both surface task state and
/// mutate the workspace filesystem on the agent's behalf.
///
/// Additional providers (ledger, prefs) will be folded into this
/// struct as capabilities 27 and workspace prefs land — keeping a
/// single concrete type simplifies the boot wiring in `run_up`.
#[derive(Clone)]
pub struct DaemonToolProvider {
    tasks: Arc<Mutex<TaskRegistry>>,
    fs: Arc<Mutex<Fs>>,
    approval_gate: Option<Arc<Mutex<ApprovalGate>>>,
    credentials: Option<Arc<CredentialStore>>,
}

impl DaemonToolProvider {
    pub fn new(tasks: Arc<Mutex<TaskRegistry>>, fs: Arc<Mutex<Fs>>) -> Self {
        Self {
            tasks,
            fs,
            approval_gate: None,
            credentials: None,
        }
    }

    /// Wire the approval gate + credential snapshot so `devdev_ask`
    /// is live. Constructed without these, the tool returns a
    /// `not configured` error — keeping pre-Phase-C tests unaffected.
    pub fn with_ask(
        mut self,
        gate: Arc<Mutex<ApprovalGate>>,
        credentials: Arc<CredentialStore>,
    ) -> Self {
        self.approval_gate = Some(gate);
        self.credentials = Some(credentials);
        self
    }
}

#[async_trait]
impl McpToolProvider for DaemonToolProvider {
    async fn tasks_list(&self) -> Result<Vec<TaskInfo>, McpProviderError> {
        let registry = self.tasks.lock().await;
        let out = registry
            .list()
            .into_iter()
            .map(|t| TaskInfo {
                id: t.id().to_string(),
                kind: t.task_type().to_string(),
                name: t.describe(),
                status: t.status().to_string(),
            })
            .collect();
        Ok(out)
    }

    async fn fs_write(&self, path: String, content: String) -> Result<(), McpProviderError> {
        if !path.starts_with('/') {
            return Err(McpProviderError::Other(format!(
                "path must be absolute (start with '/'): {path}"
            )));
        }
        let mut fs = self.fs.lock().await;
        // Create parent dirs so the agent doesn't have to mkdir first.
        if let Some(parent_end) = path.rfind('/') {
            let parent = &path[..parent_end];
            if !parent.is_empty() {
                fs.mkdir_p(parent.as_bytes(), 0o755)
                    .map_err(|e| McpProviderError::Other(format!("mkdir_p {parent}: {e:?}")))?;
            }
        }
        fs.write_path(path.as_bytes(), content.as_bytes())
            .map_err(|e| McpProviderError::Other(format!("write_path {path}: {e:?}")))?;
        Ok(())
    }

    async fn ask(&self, req: AskRequest) -> Result<AskResponse, McpProviderError> {
        let gate = self
            .approval_gate
            .as_ref()
            .ok_or_else(|| McpProviderError::Other("ask: approval gate not configured".into()))?;
        let credentials = self
            .credentials
            .as_ref()
            .ok_or_else(|| McpProviderError::Other("ask: credential store not configured".into()))?;

        let action = match req.kind {
            AskKind::PostReview => "post_review",
            AskKind::PostComment => "post_comment",
            AskKind::RequestToken => "request_token",
            AskKind::Question => "question",
        };
        let details = serde_json::json!({
            "summary": req.summary,
            "payload": req.payload,
        });

        let outcome = {
            let mut g = gate.lock().await;
            g.request_approval(action, details).await
        };

        match outcome {
            Ok(()) => {
                let needs_token = matches!(
                    req.kind,
                    AskKind::PostReview | AskKind::PostComment | AskKind::RequestToken
                );
                let (token, expires_at) = if needs_token {
                    // Resolve the target host: explicit `host` field
                    // wins, otherwise default to github.com so old
                    // clients keep working. Unknown hosts surface as
                    // a hard rejection \u2014 silently swapping in the
                    // wrong token would be a security footgun.
                    let host_id = match req.host.as_deref() {
                        Some(h) => match RepoHostId::from_browse_host(h) {
                            Some(id) => id,
                            None => {
                                return Ok(AskResponse::Rejected {
                                    reason: format!("unknown ask host: {h}"),
                                });
                            }
                        },
                        None => RepoHostId::github_com(),
                    };
                    match credentials.get(&host_id) {
                        Some(c) => (
                            Some(c.token().expose().to_string()),
                            c.expires_at_hint(),
                        ),
                        None => (None, None),
                    }
                } else {
                    (None, None)
                };
                Ok(AskResponse::Approved {
                    token,
                    expires_at,
                    payload: req.payload,
                })
            }
            Err(ApprovalError::Rejected) => Ok(AskResponse::Rejected {
                reason: "user rejected".into(),
            }),
            Err(ApprovalError::Timeout) => Ok(AskResponse::Timeout),
            Err(ApprovalError::DryRun { action, details }) => Ok(AskResponse::Rejected {
                reason: format!("dry-run: {action} {details}"),
            }),
            Err(ApprovalError::ChannelClosed) => Err(McpProviderError::Other(
                "ask: approval channel closed".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devdev_tasks::task::{Task, TaskError, TaskMessage, TaskStatus};
    use std::time::Duration;

    /// Minimal `Task` for testing — no real poll behaviour, just
    /// exposes the four accessors the provider reads.
    struct FakeTask {
        id: String,
        kind: &'static str,
        desc: String,
        status: TaskStatus,
    }

    #[async_trait]
    impl Task for FakeTask {
        fn id(&self) -> &str {
            &self.id
        }
        fn describe(&self) -> String {
            self.desc.clone()
        }
        fn status(&self) -> &TaskStatus {
            &self.status
        }
        fn set_status(&mut self, status: TaskStatus) {
            self.status = status;
        }
        async fn poll(&mut self) -> Result<Vec<TaskMessage>, TaskError> {
            Ok(vec![])
        }
        fn serialize(&self) -> Result<serde_json::Value, TaskError> {
            Ok(serde_json::json!({}))
        }
        fn task_type(&self) -> &str {
            self.kind
        }
        fn poll_interval(&self) -> Duration {
            Duration::from_secs(60)
        }
    }

    #[tokio::test]
    async fn tasks_list_reflects_registry_snapshot() {
        let mut reg = TaskRegistry::new();
        reg.add(Box::new(FakeTask {
            id: "t-1".into(),
            kind: "monitor-pr",
            desc: "monitor owner/repo#42".into(),
            status: TaskStatus::Polling,
        }));
        reg.add(Box::new(FakeTask {
            id: "t-2".into(),
            kind: "vibe-check",
            desc: "vibe check".into(),
            status: TaskStatus::Idle,
        }));

        let provider = DaemonToolProvider::new(
            Arc::new(Mutex::new(reg)),
            Arc::new(Mutex::new(devdev_workspace::Fs::new())),
        );
        let mut tasks = provider.tasks_list().await.expect("list");
        tasks.sort_by(|a, b| a.id.cmp(&b.id));

        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "t-1");
        assert_eq!(tasks[0].kind, "monitor-pr");
        assert_eq!(tasks[0].status, "polling");
        assert_eq!(tasks[1].id, "t-2");
        assert_eq!(tasks[1].kind, "vibe-check");
        assert_eq!(tasks[1].status, "idle");
    }

    #[tokio::test]
    async fn tasks_list_empty_registry_returns_empty_vec() {
        let provider = DaemonToolProvider::new(
            Arc::new(Mutex::new(TaskRegistry::new())),
            Arc::new(Mutex::new(devdev_workspace::Fs::new())),
        );
        let tasks = provider.tasks_list().await.expect("list");
        assert!(tasks.is_empty());
    }

    // ── ask: Phase C1 coverage ────────────────────────────────────

    use devdev_tasks::approval::{ApprovalPolicy, ApprovalResponse, approval_channel};

    fn build_provider_with_ask(
        policy: ApprovalPolicy,
        timeout: Duration,
        token: Option<&str>,
    ) -> (
        DaemonToolProvider,
        Arc<Mutex<devdev_tasks::approval::ApprovalHandle>>,
        Arc<CredentialStore>,
    ) {
        let (gate, handle) = approval_channel(policy, timeout);
        let gate = Arc::new(Mutex::new(gate));
        let handle = Arc::new(Mutex::new(handle));
        let store = Arc::new(match token {
            Some(t) => CredentialStore::with_entry(RepoHostId::github_com(), t),
            None => CredentialStore::empty(),
        });
        let provider = DaemonToolProvider::new(
            Arc::new(Mutex::new(TaskRegistry::new())),
            Arc::new(Mutex::new(devdev_workspace::Fs::new())),
        )
        .with_ask(gate, Arc::clone(&store));
        (provider, handle, store)
    }

    #[tokio::test]
    async fn ask_post_review_auto_approve_returns_token() {
        let (provider, _handle, _secrets) = build_provider_with_ask(
            ApprovalPolicy::AutoApprove,
            Duration::from_secs(1),
            Some("ghp_live_token"),
        );
        let resp = provider
            .ask(AskRequest {
                kind: AskKind::PostReview,
                summary: "post review on PR #42".into(),
                payload: serde_json::json!({ "comment": "looks good" }),
            host: None,
            })
            .await
            .expect("ask succeeds");
        match resp {
            AskResponse::Approved {
                token,
                expires_at,
                payload,
            } => {
                assert_eq!(token.as_deref(), Some("ghp_live_token"));
                assert!(expires_at.is_some());
                assert_eq!(payload["comment"], "looks good");
            }
            other => panic!("expected approved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ask_question_does_not_surface_token() {
        let (provider, _h, _s) = build_provider_with_ask(
            ApprovalPolicy::AutoApprove,
            Duration::from_secs(1),
            Some("ghp_live_token"),
        );
        let resp = provider
            .ask(AskRequest {
                kind: AskKind::Question,
                summary: "what color?".into(),
                payload: serde_json::json!({}),
            host: None,
            })
            .await
            .unwrap();
        match resp {
            AskResponse::Approved {
                token, expires_at, ..
            } => {
                assert!(token.is_none(), "question should not return token");
                assert!(expires_at.is_none());
            }
            other => panic!("expected approved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ask_rejected_returns_reason() {
        let (provider, handle, _s) =
            build_provider_with_ask(ApprovalPolicy::Ask, Duration::from_secs(2), Some("tok"));
        let req = AskRequest {
            kind: AskKind::PostReview,
            summary: "x".into(),
            payload: serde_json::json!({}),
            host: None,
        };
        let provider_clone = provider.clone();
        let ask_task = tokio::spawn(async move { provider_clone.ask(req).await });
        // Pump the handle: receive the request, then deny it.
        let pending = {
            let mut h = handle.lock().await;
            h.request_rx.recv().await.expect("request arrives")
        };
        {
            let h = handle.lock().await;
            h.response_tx
                .send(ApprovalResponse {
                    id: pending.id.clone(),
                    approve: false,
                })
                .await
                .unwrap();
        }
        let resp = ask_task.await.unwrap().unwrap();
        assert!(matches!(resp, AskResponse::Rejected { .. }));
    }

    #[tokio::test]
    async fn ask_timeout_returns_timeout_status() {
        let (provider, _handle, _s) =
            build_provider_with_ask(ApprovalPolicy::Ask, Duration::from_millis(50), None);
        let resp = provider
            .ask(AskRequest {
                kind: AskKind::Question,
                summary: "stalls".into(),
                payload: serde_json::json!({}),
            host: None,
            })
            .await
            .unwrap();
        assert!(matches!(resp, AskResponse::Timeout));
    }

    #[tokio::test]
    async fn ask_dry_run_policy_reports_dry_run() {
        let (provider, _h, _s) =
            build_provider_with_ask(ApprovalPolicy::DryRun, Duration::from_secs(1), None);
        let resp = provider
            .ask(AskRequest {
                kind: AskKind::PostComment,
                summary: "drop".into(),
                payload: serde_json::json!({"comment": "x"}),
            host: None,
            })
            .await
            .unwrap();
        match resp {
            AskResponse::Rejected { reason } => {
                assert!(reason.contains("dry-run"), "reason was {reason}");
            }
            other => panic!("expected rejected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ask_without_configuration_errors() {
        // Provider built without `with_ask`.
        let provider = DaemonToolProvider::new(
            Arc::new(Mutex::new(TaskRegistry::new())),
            Arc::new(Mutex::new(devdev_workspace::Fs::new())),
        );
        let err = provider
            .ask(AskRequest {
                kind: AskKind::Question,
                summary: "nope".into(),
                payload: serde_json::json!({}),
            host: None,
            })
            .await
            .expect_err("must error");
        assert!(format!("{err}").contains("not configured"));
    }

    #[tokio::test]
    async fn ask_routes_token_by_host_selector() {
        // Two host entries in the credential store; the ask's
        // `host` field picks which token comes back.
        let (gate, _handle) =
            approval_channel(ApprovalPolicy::AutoApprove, Duration::from_secs(1));
        let store = Arc::new(CredentialStore::with_entries([
            crate::credentials::Credential::new(
                RepoHostId::github_com(),
                "ghp_main",
                crate::credentials::TokenSource::Injected,
            ),
            crate::credentials::Credential::new(
                RepoHostId::ghe("ghe.acme.io"),
                "ghe_secret",
                crate::credentials::TokenSource::Injected,
            ),
        ]));
        let provider = DaemonToolProvider::new(
            Arc::new(Mutex::new(TaskRegistry::new())),
            Arc::new(Mutex::new(devdev_workspace::Fs::new())),
        )
        .with_ask(Arc::new(Mutex::new(gate)), store);

        let resp = provider
            .ask(AskRequest {
                kind: AskKind::PostReview,
                summary: "ghe review".into(),
                payload: serde_json::json!({}),
                host: Some("ghe.acme.io".into()),
            })
            .await
            .unwrap();
        match resp {
            AskResponse::Approved { token, .. } => {
                assert_eq!(token.as_deref(), Some("ghe_secret"));
            }
            other => panic!("expected approved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ask_unknown_host_is_rejected() {
        let (provider, _h, _s) = build_provider_with_ask(
            ApprovalPolicy::AutoApprove,
            Duration::from_secs(1),
            Some("ghp_main"),
        );
        let resp = provider
            .ask(AskRequest {
                kind: AskKind::PostReview,
                summary: "bogus".into(),
                payload: serde_json::json!({}),
                host: Some("gitlab.example.com".into()),
            })
            .await
            .unwrap();
        match resp {
            AskResponse::Rejected { reason } => {
                assert!(reason.contains("unknown ask host"), "reason was {reason}");
            }
            other => panic!("expected rejected, got {other:?}"),
        }
    }
}
