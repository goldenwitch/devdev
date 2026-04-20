//! Public `ShellSession` API — what the ACP hooks (capability 12) drive.
//!
//! Owns mutable [`ShellState`] and holds shared references to the VFS, the
//! tool engine, and the virtual git backend. Each [`ShellSession::execute`]
//! call parses, expands, dispatches, and updates state in one shot.
//!
//! # Lock order
//!
//! When a single call path needs more than one of the shared mutexes, the
//! order is always:
//!
//! 1. `vfs` (`Arc<Mutex<MemFs>>`)
//! 2. `git` (`Arc<Mutex<dyn VirtualGit>>`)
//!
//! Never take `git` before `vfs`. Nothing outside this crate is allowed
//! to call back into `ShellSession::execute` while either lock is held —
//! `execute` is not reentrant. Spawning background work that touches the
//! VFS or git must release both locks first. The ACP hooks layer
//! (capability 12) drives `execute` one request at a time per session,
//! which keeps this property trivially true at the agent boundary.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use devdev_git::VirtualGit;
use devdev_vfs::MemFs;
use devdev_wasm::ToolEngine;

use crate::executor::{ShellResult, execute};
use crate::parser::parse;
use crate::state::ShellState;

/// A long-lived shell session. Holds cwd / env / `$?` between calls and
/// dispatches against shared backends.
///
/// The VFS is wrapped in `Arc<Mutex<_>>` so redirects and `cd` can mutate
/// the same tree the tool engine reads from. Git is behind `Arc<Mutex<_>>`
/// because `VirtualGit` is intentionally not `Sync` (libgit2 raw pointers).
pub struct ShellSession {
    state: ShellState,
    vfs: Arc<Mutex<MemFs>>,
    tools: Arc<dyn ToolEngine>,
    git: Arc<Mutex<dyn VirtualGit>>,
}

impl ShellSession {
    pub fn new(
        vfs: Arc<Mutex<MemFs>>,
        tools: Arc<dyn ToolEngine>,
        git: Arc<Mutex<dyn VirtualGit>>,
    ) -> Self {
        Self {
            state: ShellState::new(),
            vfs,
            tools,
            git,
        }
    }

    /// Execute one command string. Parse errors are returned inline with
    /// exit code 2 (matching bash).
    pub fn execute(&mut self, command: &str) -> ShellResult {
        let list = match parse(command) {
            Ok(l) => l,
            Err(e) => {
                let result = ShellResult {
                    stdout: Vec::new(),
                    stderr: format!("devdev-shell: parse error: {e}\n").into_bytes(),
                    exit_code: 2,
                    session_ended: false,
                };
                self.state.last_exit_code = result.exit_code;
                return result;
            }
        };

        let mut vfs = self.vfs.lock().expect("vfs mutex poisoned");
        let git = self.git.lock().expect("git mutex poisoned");
        execute(
            &list,
            &mut self.state,
            &mut vfs,
            self.tools.as_ref(),
            &*git,
        )
    }

    pub fn cwd(&self) -> &Path {
        &self.state.cwd
    }

    pub fn env(&self) -> &HashMap<String, String> {
        &self.state.env
    }

    pub fn last_exit_code(&self) -> i32 {
        self.state.last_exit_code
    }

    /// Direct access for tests / hook wiring that needs to pre-seed state.
    pub fn state_mut(&mut self) -> &mut ShellState {
        &mut self.state
    }
}
