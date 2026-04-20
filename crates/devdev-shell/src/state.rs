//! Shell session state — cwd, environment variables, exit tracking.

use std::collections::HashMap;
use std::path::PathBuf;

/// Mutable state for a shell session.
#[derive(Debug, Clone)]
pub struct ShellState {
    /// Current working directory (absolute, `/`-normalized).
    pub cwd: PathBuf,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Exit code of the last command.
    pub last_exit_code: i32,
    /// Previous working directory (for `cd -`).
    pub oldpwd: Option<PathBuf>,
}

impl ShellState {
    pub fn new() -> Self {
        Self {
            cwd: PathBuf::from("/"),
            env: HashMap::new(),
            last_exit_code: 0,
            oldpwd: None,
        }
    }
}

impl Default for ShellState {
    fn default() -> Self {
        Self::new()
    }
}
