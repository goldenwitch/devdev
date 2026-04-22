//! Stub [`SessionBackend`] that delegates nothing (yet).
//!
//! `devdev up` needs *some* backend to hand to `SessionRouter` so that
//! `status`/`shutdown` IPC calls work. A real ACP-backed implementation
//! — spawning the agent subprocess, managing `AcpClient` lifecycle,
//! mapping session IDs — is a multi-hundred-line change that reaches
//! deep into `devdev-acp` internals, so per the capability brief we
//! ship a stub that returns an error for every method.
//!
//! The stub stores `agent_program` + `agent_args` so the real
//! implementation (follow-up capability) only has to swap the method
//! bodies, not the wiring at the call site.

use async_trait::async_trait;
use devdev_daemon::router::{
    AgentResponse, ResponseChunk, RouterError, SessionBackend,
};
use tokio::sync::mpsc;

/// Stub session backend. Every method returns
/// [`RouterError::Backend`] with a stable message so higher layers
/// can distinguish "not wired" from genuine runtime failures.
pub struct AcpSessionBackend {
    #[allow(dead_code)] // used by the follow-up real implementation
    agent_program: String,
    #[allow(dead_code)]
    agent_args: Vec<String>,
}

impl AcpSessionBackend {
    pub fn new(agent_program: String, agent_args: Vec<String>) -> Self {
        Self {
            agent_program,
            agent_args,
        }
    }
}

const NOT_WIRED: &str = "session backend not yet wired (devdev up v1)";

#[async_trait]
impl SessionBackend for AcpSessionBackend {
    async fn create_session(&self, _cwd: &str) -> Result<String, RouterError> {
        Err(RouterError::Backend(NOT_WIRED.into()))
    }

    async fn send_prompt(
        &self,
        _session_id: &str,
        _text: &str,
    ) -> Result<AgentResponse, RouterError> {
        Err(RouterError::Backend(NOT_WIRED.into()))
    }

    async fn send_prompt_streaming(
        &self,
        _session_id: &str,
        _text: &str,
        _tx: mpsc::Sender<ResponseChunk>,
    ) -> Result<(), RouterError> {
        Err(RouterError::Backend(NOT_WIRED.into()))
    }

    async fn destroy_session(&self, _session_id: &str) -> Result<(), RouterError> {
        Err(RouterError::Backend(NOT_WIRED.into()))
    }
}
