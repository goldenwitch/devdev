//! Authentication cascade for the Copilot CLI.
//!
//! The CLI reads `GH_TOKEN` / `GITHUB_TOKEN` / `COPILOT_GITHUB_TOKEN` from
//! its own environment at spawn time — if any of those is set, no in-band
//! auth RPC is required. This module just reports which signal we saw so
//! the client can decide whether to call `authenticate`.

/// Env var names the Copilot CLI recognises, in priority order.
pub const TOKEN_ENV_VARS: &[&str] = &["GH_TOKEN", "GITHUB_TOKEN", "COPILOT_GITHUB_TOKEN"];

/// Returns the first set env var's `(name, value)` if any, else `None`.
pub fn find_env_token() -> Option<(&'static str, String)> {
    for name in TOKEN_ENV_VARS {
        if let Ok(val) = std::env::var(name)
            && !val.is_empty()
        {
            return Some((name, val));
        }
    }
    None
}

/// Auth strategy decided up front from env + initialize result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStrategy {
    /// Env var picked up by CLI — no RPC needed.
    EnvToken(&'static str),
    /// Call `authenticate` with the named method.
    Method(String),
    /// No usable strategy — client should surface an error.
    None,
}

/// Pick a strategy from the auth methods the agent advertised.
/// Prefers env tokens; falls back to the first advertised method (the CLI
/// typically lists `oauth` or `api_key` in priority order already).
pub fn choose_strategy(advertised: &[String]) -> AuthStrategy {
    if let Some((name, _)) = find_env_token() {
        return AuthStrategy::EnvToken(name);
    }
    match advertised.first() {
        Some(m) => AuthStrategy::Method(m.clone()),
        None => AuthStrategy::None,
    }
}
