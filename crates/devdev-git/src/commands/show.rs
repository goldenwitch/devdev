//! `git show <ref>` — commit metadata + diff against its first parent.

use git2::{DiffFormat, Repository};

use super::{GitResult, log::format_time};

pub fn run(repo: &Repository, args: &[String]) -> GitResult {
    let target = if args.is_empty() { "HEAD" } else { &args[0] };
    let obj = match repo.revparse_single(target) {
        Ok(o) => o,
        Err(e) => return GitResult::err(format!("git show: {e}"), 128),
    };
    let commit = match obj.peel_to_commit() {
        Ok(c) => c,
        Err(e) => return GitResult::err(format!("git show: {e}"), 128),
    };

    let mut out = Vec::new();
    out.extend_from_slice(format!("commit {}\n", commit.id()).as_bytes());
    let author = commit.author();
    out.extend_from_slice(
        format!(
            "Author: {} <{}>\n",
            author.name().unwrap_or(""),
            author.email().unwrap_or("")
        )
        .as_bytes(),
    );
    out.extend_from_slice(format!("Date:   {}\n\n", format_time(&author.when())).as_bytes());
    for line in commit.message().unwrap_or("").lines() {
        out.extend_from_slice(b"    ");
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    out.push(b'\n');

    let new_tree = match commit.tree() {
        Ok(t) => t,
        Err(e) => return GitResult::err(format!("git show: {e}"), 128),
    };
    let old_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let diff = match repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None) {
        Ok(d) => d,
        Err(e) => return GitResult::err(format!("git show: {e}"), 128),
    };
    if let Err(e) = diff.print(DiffFormat::Patch, |_, _, line| {
        match line.origin() {
            '+' | '-' | ' ' => out.push(line.origin() as u8),
            _ => {}
        }
        out.extend_from_slice(line.content());
        true
    }) {
        return GitResult::err(format!("git show: {e}"), 128);
    }

    GitResult::ok(out)
}
