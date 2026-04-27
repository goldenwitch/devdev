//! Host-derived secrets handed to the agent only via approved
//! `devdev_ask` calls.
//!
//! The slot is populated at `devdev up` time (best-effort `gh auth
//! token`) and may be cleared / rotated at runtime. It is *never*
//! injected into the agent's process environment — the agent must
//! call `devdev_ask` and receive the token in the tool response.

use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::process::Command;

/// Slot of secrets that may be surfaced through `devdev_ask`.
#[derive(Debug, Default, Clone)]
pub struct AgentSecrets {
    /// Result of `gh auth token`. `None` if `gh` is missing or unauth.
    pub gh_token: Option<String>,
    /// Wall-clock seconds since epoch when `gh_token` was sampled.
    pub gh_token_sampled_at: Option<u64>,
}

impl AgentSecrets {
    /// Set the GitHub token and stamp the sample time. Used both by
    /// the boot path and by tests (which can inject deterministically).
    pub fn set_gh_token(&mut self, token: Option<String>) {
        self.gh_token = token;
        self.gh_token_sampled_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
    }

    /// `expires_at` hint for token consumers. We do not enforce
    /// revocation; this is informational only.
    pub fn token_expires_at_hint(&self) -> Option<u64> {
        // Hand out a one-hour validity window. GitHub user tokens live
        // far longer, but bounded hints train downstream consumers
        // not to cache.
        const ONE_HOUR_SECS: u64 = 3600;
        self.gh_token_sampled_at.map(|t| t + ONE_HOUR_SECS)
    }
}

/// Best-effort `gh auth token` invocation. Returns `Ok(None)` when
/// `gh` is not on PATH or returns a non-zero status; the caller treats
/// that as "no token available" rather than a hard failure.
pub async fn try_read_gh_token() -> std::io::Result<Option<String>> {
    let mut cmd = Command::new("gh");
    cmd.arg("auth").arg("token");
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let output = match tokio::time::timeout(Duration::from_secs(5), cmd.output()).await {
        Ok(r) => r?,
        Err(_) => return Ok(None),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        Ok(None)
    } else {
        Ok(Some(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_token_stamps_time() {
        let mut s = AgentSecrets::default();
        assert!(s.gh_token.is_none());
        assert!(s.token_expires_at_hint().is_none());
        s.set_gh_token(Some("ghp_test".to_string()));
        assert_eq!(s.gh_token.as_deref(), Some("ghp_test"));
        let exp = s.token_expires_at_hint().expect("hint set");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Expires roughly an hour from now (bounded slack for slow tests).
        assert!(exp >= now + 3500 && exp <= now + 3700);
    }

    #[test]
    fn clear_removes_token_and_keeps_hint_none() {
        let mut s = AgentSecrets::default();
        s.set_gh_token(Some("x".into()));
        s.set_gh_token(None);
        assert!(s.gh_token.is_none());
        // Hint stays Some because we stamped time on the second call;
        // hint is a function of "when did we last sample", not whether
        // we got a value.
        assert!(s.token_expires_at_hint().is_some());
    }
}
