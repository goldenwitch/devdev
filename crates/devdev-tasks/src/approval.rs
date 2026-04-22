//! Approval gate: intercepts external actions and applies policy.

use std::time::Duration;

use tokio::sync::mpsc;

/// Policy for handling external action approvals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    /// Queue the action, emit request, wait for user response.
    Ask,
    /// Execute immediately, log the action.
    AutoApprove,
    /// Log what would happen, never execute.
    DryRun,
}

/// A request for approval of an external action.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub id: String,
    pub action: String,
    pub details: serde_json::Value,
}

/// A response to an approval request.
#[derive(Debug, Clone)]
pub struct ApprovalResponse {
    pub id: String,
    pub approve: bool,
}

/// Error returned by the approval gate.
#[derive(thiserror::Error, Debug)]
pub enum ApprovalError {
    #[error("action rejected by user")]
    Rejected,

    #[error("approval timed out")]
    Timeout,

    #[error("dry run: {action}")]
    DryRun {
        action: String,
        details: serde_json::Value,
    },

    #[error("channel closed")]
    ChannelClosed,
}

/// Intercepts external actions and applies approval policy.
pub struct ApprovalGate {
    policy: ApprovalPolicy,
    timeout: Duration,
    request_tx: mpsc::Sender<ApprovalRequest>,
    response_rx: mpsc::Receiver<ApprovalResponse>,
    next_id: u64,
}

/// The other end of the approval channels (held by TUI/headless).
pub struct ApprovalHandle {
    pub request_rx: mpsc::Receiver<ApprovalRequest>,
    pub response_tx: mpsc::Sender<ApprovalResponse>,
}

/// Create an approval gate and its corresponding handle.
pub fn approval_channel(
    policy: ApprovalPolicy,
    timeout: Duration,
) -> (ApprovalGate, ApprovalHandle) {
    let (req_tx, req_rx) = mpsc::channel(32);
    let (resp_tx, resp_rx) = mpsc::channel(32);

    let gate = ApprovalGate {
        policy,
        timeout,
        request_tx: req_tx,
        response_rx: resp_rx,
        next_id: 1,
    };

    let handle = ApprovalHandle {
        request_rx: req_rx,
        response_tx: resp_tx,
    };

    (gate, handle)
}

impl ApprovalGate {
    /// Request approval for an external action.
    pub async fn request_approval(
        &mut self,
        action: &str,
        details: serde_json::Value,
    ) -> Result<(), ApprovalError> {
        match self.policy {
            ApprovalPolicy::AutoApprove => Ok(()),

            ApprovalPolicy::DryRun => Err(ApprovalError::DryRun {
                action: action.to_string(),
                details,
            }),

            ApprovalPolicy::Ask => {
                let id = format!("a-{}", self.next_id);
                self.next_id += 1;

                let request = ApprovalRequest {
                    id: id.clone(),
                    action: action.to_string(),
                    details,
                };

                self.request_tx
                    .send(request)
                    .await
                    .map_err(|_| ApprovalError::ChannelClosed)?;

                // Wait for response with timeout.
                match tokio::time::timeout(self.timeout, self.response_rx.recv()).await {
                    Ok(Some(response)) => {
                        if response.approve {
                            Ok(())
                        } else {
                            Err(ApprovalError::Rejected)
                        }
                    }
                    Ok(None) => Err(ApprovalError::ChannelClosed),
                    Err(_) => Err(ApprovalError::Timeout),
                }
            }
        }
    }

    pub fn policy(&self) -> ApprovalPolicy {
        self.policy
    }
}
