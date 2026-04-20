//! `VirtualGit` implementations for the evaluator.
//!
//! [`OwnedVirtualGit`] owns a [`VirtualRepo`] so it can live behind an
//! `Arc<Mutex<dyn VirtualGit + 'static>>` (cap 13 requirement). The
//! upstream [`VirtualGitRepo<'a>`] is lifetime-parameterised and
//! therefore not directly storable; we rebuild one per call.
//!
//! [`StubGit`] is the non-repo fallback: every git command returns
//! `"not a git repository"` with exit code 1. Used when the host path
//! has no `.git` directory or when the caller sets
//! `EvalConfig::include_git = false`.

use devdev_git::{GitResult, VirtualGit, VirtualGitRepo, VirtualRepo};

/// Owns a [`VirtualRepo`] and answers `execute` by building a fresh
/// borrowing [`VirtualGitRepo`] per call. Construct inside the shell
/// worker's `FnOnce` closure; libgit2 handles are `!Send`.
pub struct OwnedVirtualGit {
    repo: VirtualRepo,
}

impl OwnedVirtualGit {
    pub fn new(repo: VirtualRepo) -> Self {
        Self { repo }
    }
}

impl VirtualGit for OwnedVirtualGit {
    fn execute(&self, args: &[String], cwd: &str) -> GitResult {
        VirtualGitRepo::new(&self.repo).execute(args, cwd)
    }
}

/// `VirtualGit` that always fails with `"not a git repository"`.
///
/// Matches git's own wording on stderr so agents see familiar output.
pub struct StubGit;

impl VirtualGit for StubGit {
    fn execute(&self, _args: &[String], _cwd: &str) -> GitResult {
        GitResult::err(
            "fatal: not a git repository (or any of the parent directories)",
            1,
        )
    }
}
