//! `git branch` — list local branches with the current one marked `*`.

use git2::{BranchType, Repository};

use super::GitResult;

pub fn run(repo: &Repository, _args: &[String]) -> GitResult {
    let current = repo.head().ok().and_then(|h| h.shorthand().map(|s| s.to_owned()));
    let branches = match repo.branches(Some(BranchType::Local)) {
        Ok(b) => b,
        Err(e) => return GitResult::err(format!("git branch: {e}"), 128),
    };
    let mut names: Vec<String> = Vec::new();
    for b in branches {
        let (branch, _) = match b {
            Ok(pair) => pair,
            Err(e) => return GitResult::err(format!("git branch: {e}"), 128),
        };
        if let Ok(Some(name)) = branch.name() {
            names.push(name.to_owned());
        }
    }
    names.sort();
    let mut out = Vec::new();
    for name in &names {
        let marker = if Some(name.as_str()) == current.as_deref() {
            "* "
        } else {
            "  "
        };
        out.extend_from_slice(format!("{marker}{name}\n").as_bytes());
    }
    GitResult::ok(out)
}
