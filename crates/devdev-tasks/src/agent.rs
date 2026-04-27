//! `AgentRunner` — the seam between a task and the agent.
//!
//! Tasks need to ask the agent something (review this PR, summarize
//! this diff) without taking a hard dependency on the daemon's
//! [`SessionRouter`]. This trait is the seam: implementors run a
//! prompt to completion and return the assistant's reply text.

use async_trait::async_trait;

#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// Run a single prompt to completion. Returns the agent's reply
    /// text, or a human-readable error string.
    async fn run_prompt(&self, prompt: String) -> Result<String, String>;
}
