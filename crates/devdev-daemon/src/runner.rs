//! `RouterRunner` — bridge from [`AgentRunner`] to [`SessionRouter`].
//!
//! Each PR-level task gets its own `RouterRunner` so it owns its own
//! agent session keyed by task id. The session is created lazily on
//! the first `run_prompt` call.

use std::sync::Arc;

use async_trait::async_trait;
use devdev_tasks::agent::AgentRunner;
use tokio::sync::Mutex;

use crate::router::{SessionContext, SessionHandle, SessionRouter};

pub struct RouterRunner {
    router: Arc<SessionRouter>,
    task_id: String,
    handle: Mutex<Option<SessionHandle>>,
}

impl RouterRunner {
    pub fn new(router: Arc<SessionRouter>, task_id: impl Into<String>) -> Self {
        Self {
            router,
            task_id: task_id.into(),
            handle: Mutex::new(None),
        }
    }
}

#[async_trait]
impl AgentRunner for RouterRunner {
    async fn run_prompt(&self, prompt: String) -> Result<String, String> {
        let mut slot = self.handle.lock().await;
        if slot.is_none() {
            let h = self
                .router
                .create_session(&self.task_id, SessionContext::default())
                .await
                .map_err(|e| e.to_string())?;
            *slot = Some(h);
        }
        let h = slot.as_ref().expect("just inserted");
        let resp = h.send_prompt(&prompt).await.map_err(|e| e.to_string())?;
        Ok(resp.text)
    }
}
