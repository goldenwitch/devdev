//! Session router: maps tasks to agent sessions.
//!
//! Each task gets its own logical session. The router manages the mapping
//! and provides `SessionHandle` for tasks to send prompts.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

/// Error type for router operations.
#[derive(thiserror::Error, Debug)]
pub enum RouterError {
    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session already exists for task: {0}")]
    SessionExists(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("subprocess crashed, recovery in progress")]
    SubprocessCrashed,
}

/// Response from the agent.
#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub text: String,
    pub stop_reason: String,
}

/// A streaming response chunk.
#[derive(Debug, Clone)]
pub enum ResponseChunk {
    Text(String),
    Done { stop_reason: String },
}

/// Context injected into a session.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub system_prompt: String,
    pub repo_paths: Vec<String>,
    pub prior_observations: Vec<String>,
}

impl SessionContext {
    /// Build the initial prompt that injects context.
    pub fn initial_prompt(&self) -> String {
        let mut parts = Vec::new();
        if !self.system_prompt.is_empty() {
            parts.push(self.system_prompt.clone());
        }
        if !self.repo_paths.is_empty() {
            parts.push(format!("Repos: {}", self.repo_paths.join(", ")));
        }
        for obs in &self.prior_observations {
            parts.push(format!("Prior: {obs}"));
        }
        parts.join("\n")
    }
}

/// Internal state for a session.
struct SessionState {
    session_id: String,
    task_id: String,
    context: SessionContext,
}

/// Backend trait for agent communication (mockable for tests).
#[async_trait::async_trait]
pub trait SessionBackend: Send + Sync {
    /// Create a new agent session, returns session_id.
    async fn create_session(&self, cwd: &str) -> Result<String, RouterError>;

    /// Send a prompt to an existing session.
    async fn send_prompt(&self, session_id: &str, text: &str)
    -> Result<AgentResponse, RouterError>;

    /// Send a prompt and stream response chunks.
    async fn send_prompt_streaming(
        &self,
        session_id: &str,
        text: &str,
        tx: mpsc::Sender<ResponseChunk>,
    ) -> Result<(), RouterError>;

    /// Cancel/destroy a session.
    async fn destroy_session(&self, session_id: &str) -> Result<(), RouterError>;
}

/// Manages task → agent session mapping.
pub struct SessionRouter {
    backend: Arc<dyn SessionBackend>,
    sessions: Mutex<HashMap<String, SessionState>>,
    interactive_session: Mutex<Option<String>>,
}

impl SessionRouter {
    pub fn new(backend: Arc<dyn SessionBackend>) -> Self {
        Self {
            backend,
            sessions: Mutex::new(HashMap::new()),
            interactive_session: Mutex::new(None),
        }
    }

    /// Create a new session for a task.
    pub async fn create_session(
        &self,
        task_id: &str,
        context: SessionContext,
    ) -> Result<SessionHandle, RouterError> {
        let mut sessions = self.sessions.lock().await;
        if sessions.contains_key(task_id) {
            return Err(RouterError::SessionExists(task_id.to_string()));
        }

        let session_id = self.backend.create_session("/").await?;

        // Inject context if any.
        let initial = context.initial_prompt();
        if !initial.is_empty() {
            self.backend.send_prompt(&session_id, &initial).await?;
        }

        sessions.insert(
            task_id.to_string(),
            SessionState {
                session_id: session_id.clone(),
                task_id: task_id.to_string(),
                context,
            },
        );

        Ok(SessionHandle {
            task_id: task_id.to_string(),
            session_id,
            backend: Arc::clone(&self.backend),
        })
    }

    /// Destroy a session (task completed/cancelled).
    pub async fn destroy_session(&self, task_id: &str) -> Result<(), RouterError> {
        let mut sessions = self.sessions.lock().await;
        let state = sessions
            .remove(task_id)
            .ok_or_else(|| RouterError::SessionNotFound(task_id.to_string()))?;

        self.backend.destroy_session(&state.session_id).await
    }

    /// Create the interactive session (for TUI/headless chat).
    pub async fn create_interactive_session(&self) -> Result<SessionHandle, RouterError> {
        let session_id = self.backend.create_session("/").await?;

        *self.interactive_session.lock().await = Some(session_id.clone());

        Ok(SessionHandle {
            task_id: "__interactive__".to_string(),
            session_id,
            backend: Arc::clone(&self.backend),
        })
    }

    /// Get session info for a task.
    pub async fn get_session_id(&self, task_id: &str) -> Option<String> {
        let sessions = self.sessions.lock().await;
        sessions.get(task_id).map(|s| s.session_id.clone())
    }

    /// List all active task sessions.
    pub async fn active_sessions(&self) -> Vec<(String, String)> {
        let sessions = self.sessions.lock().await;
        sessions
            .iter()
            .map(|(tid, state)| (tid.clone(), state.session_id.clone()))
            .collect()
    }

    /// Recover from a subprocess crash: recreate all sessions.
    pub async fn recover(&self) -> Result<(), RouterError> {
        let mut sessions = self.sessions.lock().await;
        let mut new_sessions = HashMap::new();

        for (task_id, state) in sessions.drain() {
            let new_session_id = self.backend.create_session("/").await?;

            // Re-inject context.
            let initial = state.context.initial_prompt();
            if !initial.is_empty() {
                self.backend.send_prompt(&new_session_id, &initial).await?;
            }

            new_sessions.insert(
                task_id,
                SessionState {
                    session_id: new_session_id,
                    task_id: state.task_id,
                    context: state.context,
                },
            );
        }

        *sessions = new_sessions;
        Ok(())
    }
}

/// A lightweight handle a task uses to communicate with its agent session.
pub struct SessionHandle {
    pub task_id: String,
    session_id: String,
    backend: Arc<dyn SessionBackend>,
}

impl SessionHandle {
    /// Send a prompt and collect the full response.
    pub async fn send_prompt(&self, text: &str) -> Result<AgentResponse, RouterError> {
        self.backend.send_prompt(&self.session_id, text).await
    }

    /// Send a prompt and stream response chunks.
    pub async fn send_prompt_streaming(
        &self,
        text: &str,
    ) -> Result<mpsc::Receiver<ResponseChunk>, RouterError> {
        let (tx, rx) = mpsc::channel(64);
        let backend = Arc::clone(&self.backend);
        let session_id = self.session_id.clone();
        let text = text.to_string();

        tokio::spawn(async move {
            let _ = backend.send_prompt_streaming(&session_id, &text, tx).await;
        });

        Ok(rx)
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}
