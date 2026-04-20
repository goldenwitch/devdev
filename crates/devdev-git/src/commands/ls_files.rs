//! `git ls-files` — list tracked file paths from the index.

use git2::Repository;

use super::GitResult;

pub fn run(repo: &Repository, _args: &[String]) -> GitResult {
    let index = match repo.index() {
        Ok(i) => i,
        Err(e) => return GitResult::err(format!("git ls-files: {e}"), 128),
    };
    let mut names: Vec<String> = index
        .iter()
        .filter_map(|e| String::from_utf8(e.path).ok())
        .collect();
    names.sort();
    let mut out = Vec::new();
    for n in &names {
        out.extend_from_slice(n.as_bytes());
        out.push(b'\n');
    }
    GitResult::ok(out)
}
