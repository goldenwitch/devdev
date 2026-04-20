//! Pipeline execution + operator sequencing + redirect handling.
//!
//! Given a parsed [`CommandList`], run each [`Pipeline`] sequentially and
//! honour `&&`, `||`, `;` between them. Within a pipeline, run stages
//! sequentially (buffer-and-pass): stage N's stdout becomes stage N+1's stdin.
//! Redirects are applied per-stage against the VFS.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use devdev_git::VirtualGit;
use devdev_vfs::MemFs;
use devdev_wasm::ToolEngine;

use crate::ast::{Command, CommandList, Operator, Pipeline, Redirect, RedirectKind};
use crate::dispatch::{DispatchCtx, DispatchOutput, dispatch};
use crate::expand::{expand_word, expand_words};
use crate::state::ShellState;

/// Hard cap on any single intermediate buffer in the pipeline — stdin
/// into a stage, the final stage's stdout, or the aggregate stderr. Once
/// a stage would push us past this, we truncate and set exit code 141
/// (POSIX SIGPIPE convention) so the overall pipeline fails loudly
/// instead of silently exhausting host memory.
pub const MAX_PIPE_BUFFER: usize = 64 * 1024 * 1024;

/// Exit code used when a pipeline buffer would exceed [`MAX_PIPE_BUFFER`].
/// Matches POSIX `128 + SIGPIPE`.
pub const EXIT_PIPE_OVERFLOW: i32 = 141;

/// Append `src` to `dst`, truncating to keep `dst.len() <= MAX_PIPE_BUFFER`.
/// Returns `true` when truncation occurred.
fn append_capped(dst: &mut Vec<u8>, src: &[u8]) -> bool {
    let room = MAX_PIPE_BUFFER.saturating_sub(dst.len());
    if src.len() <= room {
        dst.extend_from_slice(src);
        false
    } else {
        dst.extend_from_slice(&src[..room]);
        true
    }
}

/// Result of executing a full command string.
#[derive(Debug, Clone, Default)]
pub struct ShellResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    /// True if the session should end (e.g. `exit` was called).
    pub session_ended: bool,
}

/// Execute a parsed command list against the given session state.
pub fn execute(
    list: &CommandList,
    state: &mut ShellState,
    vfs: &mut MemFs,
    tools: &dyn ToolEngine,
    git: &dyn VirtualGit,
) -> ShellResult {
    let mut last = run_pipeline(&list.first, state, vfs, tools, git);
    state.last_exit_code = last.exit_code;
    if last.session_ended {
        return last;
    }

    for (op, pipe) in &list.rest {
        let skip = match op {
            Operator::And => last.exit_code != 0,
            Operator::Or => last.exit_code == 0,
            Operator::Semi => false,
        };
        if skip {
            continue;
        }
        let next = run_pipeline(pipe, state, vfs, tools, git);
        state.last_exit_code = next.exit_code;
        let overflow_out = append_capped(&mut last.stdout, &next.stdout);
        let overflow_err = append_capped(&mut last.stderr, &next.stderr);
        last.exit_code = if overflow_out || overflow_err {
            EXIT_PIPE_OVERFLOW
        } else {
            next.exit_code
        };
        last.session_ended = next.session_ended;
        if last.session_ended {
            break;
        }
    }

    last
}

/// Run one pipeline: pipe stdout of stage N into stdin of stage N+1.
fn run_pipeline(
    pipe: &Pipeline,
    state: &mut ShellState,
    vfs: &mut MemFs,
    tools: &dyn ToolEngine,
    git: &dyn VirtualGit,
) -> ShellResult {
    if pipe.stages.is_empty() {
        return ShellResult::default();
    }

    let mut stdin: Vec<u8> = Vec::new();
    let mut aggregate_stderr: Vec<u8> = Vec::new();
    let mut last_stdout: Vec<u8> = Vec::new();
    let mut last_exit = 0;
    let mut overflowed = false;

    let n = pipe.stages.len();
    for (i, cmd) in pipe.stages.iter().enumerate() {
        let stage = run_stage(cmd, &stdin, state, vfs, tools, git);
        last_exit = stage.exit_code;
        if append_capped(&mut aggregate_stderr, &stage.stderr) {
            overflowed = true;
        }

        if stage.session_ended {
            return ShellResult {
                stdout: Vec::new(),
                stderr: aggregate_stderr,
                exit_code: last_exit,
                session_ended: true,
            };
        }

        if i + 1 == n {
            // Final stage — copy into last_stdout with the cap.
            last_stdout.clear();
            if append_capped(&mut last_stdout, &stage.stdout) {
                overflowed = true;
            }
        } else {
            // Intermediate stage — its stdout becomes the next stdin.
            // Cap here too so a mid-pipeline explosion is stopped early.
            stdin.clear();
            if append_capped(&mut stdin, &stage.stdout) {
                overflowed = true;
            }
        }
    }

    ShellResult {
        stdout: last_stdout,
        stderr: aggregate_stderr,
        exit_code: if overflowed { EXIT_PIPE_OVERFLOW } else { last_exit },
        session_ended: false,
    }
}

/// Run one stage: expand argv, apply `< file` redirect to stdin, dispatch,
/// then apply output redirects. Returns the (possibly empty if redirected)
/// stdout plus stderr and exit code.
fn run_stage(
    cmd: &Command,
    piped_stdin: &[u8],
    state: &mut ShellState,
    vfs: &mut MemFs,
    tools: &dyn ToolEngine,
    git: &dyn VirtualGit,
) -> DispatchOutput {
    // Expand the command name.
    let name_parts = expand_word(&cmd.name, state, vfs);
    if name_parts.is_empty() {
        return DispatchOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
            session_ended: false,
        };
    }
    let name = name_parts[0].clone();
    // Words in `name_parts[1..]` are the fan-out of a glob on the command
    // name itself (rare); prepend them as the first args.
    let mut args: Vec<String> = name_parts.into_iter().skip(1).collect();
    args.extend(expand_words(&cmd.args, state, vfs));

    // Per-command env assignments: expand values, collect into overlay.
    let mut env_overlay: HashMap<String, String> = HashMap::new();
    for (k, val) in &cmd.env_assignments {
        let v = expand_word(val, state, vfs).join(" ");
        env_overlay.insert(k.clone(), v.clone());
    }

    // Apply input redirect (`< file`) — reads VFS file into stdin.
    let mut stdin: Vec<u8> = piped_stdin.to_vec();
    let mut early_err: Option<DispatchOutput> = None;
    for r in &cmd.redirects {
        if matches!(r.kind, RedirectKind::In) {
            let target = expand_redirect_target(r, state, vfs);
            let abs = resolve_path(&target, &state.cwd);
            match vfs.read(&abs) {
                Ok(data) => stdin = data,
                Err(e) => {
                    early_err = Some(DispatchOutput {
                        stdout: Vec::new(),
                        stderr: format!("{name}: {target}: {e}\n").into_bytes(),
                        exit_code: 1,
                        session_ended: false,
                    });
                    break;
                }
            }
        }
    }
    if let Some(err) = early_err {
        return err;
    }

    // Dispatch.
    let mut ctx = DispatchCtx {
        state,
        vfs,
        tools,
        git,
    };
    let mut out = dispatch(&name, &args, &stdin, &env_overlay, &mut ctx);

    // Apply output redirects in order.
    for r in &cmd.redirects {
        match r.kind {
            RedirectKind::In => {} // handled above
            RedirectKind::Out | RedirectKind::Append => {
                let target = expand_redirect_target(r, state, vfs);
                let abs = resolve_path(&target, &state.cwd);
                let data = std::mem::take(&mut out.stdout);
                let result = if matches!(r.kind, RedirectKind::Out) {
                    vfs.write(&abs, &data)
                } else if vfs.exists(&abs) {
                    vfs.append(&abs, &data)
                } else {
                    vfs.write(&abs, &data)
                };
                if let Err(e) = result {
                    out.stderr
                        .extend_from_slice(format!("{name}: {target}: {e}\n").as_bytes());
                    out.exit_code = 1;
                }
            }
            RedirectKind::ErrOut | RedirectKind::ErrAppend => {
                let target = expand_redirect_target(r, state, vfs);
                let abs = resolve_path(&target, &state.cwd);
                let data = std::mem::take(&mut out.stderr);
                let result = if matches!(r.kind, RedirectKind::ErrOut) {
                    vfs.write(&abs, &data)
                } else if vfs.exists(&abs) {
                    vfs.append(&abs, &data)
                } else {
                    vfs.write(&abs, &data)
                };
                if let Err(e) = result {
                    out.stderr
                        .extend_from_slice(format!("{name}: {target}: {e}\n").as_bytes());
                    out.exit_code = 1;
                }
            }
            RedirectKind::ErrToStdout => {
                let drained = std::mem::take(&mut out.stderr);
                out.stdout.extend_from_slice(&drained);
            }
        }
    }

    out
}

fn expand_redirect_target(r: &Redirect, state: &ShellState, vfs: &MemFs) -> String {
    expand_word(&r.target, state, vfs).join(" ")
}

fn resolve_path(s: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(s);
    let resolved = devdev_vfs::path::resolve(p, cwd);
    devdev_vfs::path::normalize(&resolved)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_capped_no_truncation_below_limit() {
        let mut dst = Vec::new();
        let src = vec![0u8; 1024];
        assert!(!append_capped(&mut dst, &src));
        assert_eq!(dst.len(), 1024);
    }

    #[test]
    fn append_capped_truncates_at_limit() {
        let mut dst = vec![0u8; MAX_PIPE_BUFFER - 16];
        let src = vec![0u8; 64];
        // Can only fit 16 more bytes.
        assert!(append_capped(&mut dst, &src));
        assert_eq!(dst.len(), MAX_PIPE_BUFFER);
    }

    #[test]
    fn append_capped_idempotent_when_already_full() {
        let mut dst = vec![0u8; MAX_PIPE_BUFFER];
        let src = vec![0u8; 1];
        assert!(append_capped(&mut dst, &src));
        assert_eq!(dst.len(), MAX_PIPE_BUFFER);
    }
}