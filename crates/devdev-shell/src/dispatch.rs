//! Command dispatch: builtin → git → tool engine.
//!
//! The executor calls [`dispatch`] with a command name and its already-expanded
//! argv. We try builtins first (they can mutate session state), then the
//! virtual git engine for `git ...` invocations, then the WASM/native tool
//! registry for everything else. Unknown commands bubble out of the tool
//! engine as exit 127.

use std::collections::HashMap;

use devdev_git::VirtualGit;
use devdev_vfs::MemFs;
use devdev_wasm::ToolEngine;

use crate::builtins::{BuiltinResult, try_builtin};
use crate::state::ShellState;

/// Outcome of dispatching one command stage.
#[derive(Debug, Clone)]
pub struct DispatchOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    /// True if a builtin signalled `exit` — executor should end the session.
    pub session_ended: bool,
}

/// Mutable session + backend references the dispatcher threads through.
pub struct DispatchCtx<'a> {
    pub state: &'a mut ShellState,
    pub vfs: &'a mut MemFs,
    pub tools: &'a dyn ToolEngine,
    pub git: &'a dyn VirtualGit,
}

/// Dispatch one command. `env_overlay` applies for the duration of this call
/// only (per-command `FOO=bar cmd` assignments).
pub fn dispatch(
    name: &str,
    args: &[String],
    stdin: &[u8],
    env_overlay: &HashMap<String, String>,
    ctx: &mut DispatchCtx<'_>,
) -> DispatchOutput {
    // 1. Builtin?
    match try_builtin(name, args, ctx.state, ctx.vfs) {
        BuiltinResult::Ok {
            stdout,
            stderr,
            exit_code,
        } => {
            return DispatchOutput {
                stdout,
                stderr,
                exit_code,
                session_ended: false,
            };
        }
        BuiltinResult::Exit(code) => {
            return DispatchOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: code,
                session_ended: true,
            };
        }
        BuiltinResult::NotBuiltin => {}
    }

    // 2. Virtual git?
    if name == "git" {
        let cwd = ctx.state.cwd.to_string_lossy().into_owned();
        let result = ctx.git.execute(args, &cwd);
        return DispatchOutput {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            session_ended: false,
        };
    }

    // 3. Tool engine (WASM / native / shim / 127).
    let mut env = ctx.state.env.clone();
    for (k, v) in env_overlay {
        env.insert(k.clone(), v.clone());
    }
    let cwd = ctx.state.cwd.to_string_lossy().into_owned();
    let result = ctx.tools.execute(name, args, stdin, &env, &cwd, ctx.vfs);
    DispatchOutput {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        session_ended: false,
    }
}
