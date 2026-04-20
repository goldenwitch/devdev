//! `git tag` — list tag names.

use git2::Repository;

use super::GitResult;

pub fn run(repo: &Repository, _args: &[String]) -> GitResult {
    let names = match repo.tag_names(None) {
        Ok(n) => n,
        Err(e) => return GitResult::err(format!("git tag: {e}"), 128),
    };
    let mut sorted: Vec<String> = names.iter().flatten().map(|s| s.to_owned()).collect();
    sorted.sort();
    let mut out = Vec::new();
    for n in sorted {
        out.extend_from_slice(n.as_bytes());
        out.push(b'\n');
    }
    GitResult::ok(out)
}
