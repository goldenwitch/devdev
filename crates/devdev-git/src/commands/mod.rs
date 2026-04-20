//! `git` subcommand dispatch.
//!
//! Each module under `commands::` implements one subcommand. The
//! [`VirtualGit`] trait is the public face — the shell executor holds one
//! and calls [`VirtualGit::execute`] with the argv after the leading `git`.
//!
//! Contract mirrors the `ToolEngine` trait from capability 04: return
//! `(stdout, stderr, exit_code)`. No panics on bad input — emit a stderr
//! message and exit non-zero instead.

use crate::VirtualRepo;

pub mod blame;
pub mod branch;
pub mod diff;
pub mod log;
pub mod ls_files;
pub mod rev_parse;
pub mod show;
pub mod status;
pub mod tag;

/// Result of running one `git <sub>` invocation.
#[derive(Debug, Clone)]
pub struct GitResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

impl GitResult {
    pub fn ok(stdout: Vec<u8>) -> Self {
        Self {
            stdout,
            stderr: Vec::new(),
            exit_code: 0,
        }
    }

    pub fn err(msg: impl Into<String>, exit_code: i32) -> Self {
        let mut s = msg.into();
        if !s.ends_with('\n') {
            s.push('\n');
        }
        Self {
            stdout: Vec::new(),
            stderr: s.into_bytes(),
            exit_code,
        }
    }
}

/// Mutating / network subcommands we refuse to run in the sandbox.
///
/// Per `capabilities/06-virtual-git-commands.md` the agent must see a
/// stable error message so it knows to work around rather than retry.
pub const BLOCKED: &[&str] = &[
    "push",
    "pull",
    "fetch",
    "merge",
    "rebase",
    "cherry-pick",
    "commit",
    "add",
    "reset",
    "stash",
    "checkout",
    "switch",
    "clone",
    "init",
];

/// Read-only subset of git implemented against an in-memory repo.
///
/// Implementors hold a reference to a [`VirtualRepo`] (or equivalent) and
/// route `args[0]` to the matching handler module. Intentionally **not**
/// `Send`/`Sync` — `git2::Repository` wraps a raw libgit2 pointer which
/// is neither; the shell executor holds one per session.
pub trait VirtualGit {
    /// `args` is the tokens after `git` (e.g. `["log", "--oneline", "-5"]`).
    /// `cwd` is the VFS-relative working directory string — unused by most
    /// commands but threaded through for forward compatibility with
    /// pathspec resolution.
    fn execute(&self, args: &[String], cwd: &str) -> GitResult;
}

/// Default implementation backed by a single [`VirtualRepo`].
pub struct VirtualGitRepo<'a> {
    repo: &'a VirtualRepo,
}

impl<'a> VirtualGitRepo<'a> {
    pub fn new(repo: &'a VirtualRepo) -> Self {
        Self { repo }
    }
}

impl<'a> VirtualGit for VirtualGitRepo<'a> {
    fn execute(&self, args: &[String], cwd: &str) -> GitResult {
        let Some((sub, rest)) = args.split_first() else {
            return GitResult::err(
                "devdev: git requires a subcommand (try `git log`, `git status`, …)",
                1,
            );
        };

        if BLOCKED.contains(&sub.as_str()) {
            return GitResult::err(
                format!("devdev: git {sub} is not available in the virtual workspace."),
                1,
            );
        }

        let repo = self.repo.repo();
        match sub.as_str() {
            "log" => log::run(repo, rest),
            "show" => show::run(repo, rest),
            "diff" => diff::run(repo, rest),
            "status" => status::run(repo, rest, cwd),
            "blame" => blame::run(repo, rest),
            "rev-parse" => rev_parse::run(repo, rest),
            "branch" => branch::run(repo, rest),
            "tag" => tag::run(repo, rest),
            "ls-files" => ls_files::run(repo, rest),
            other => GitResult::err(format!("git: '{other}' is not a devdev git command."), 1),
        }
    }
}
