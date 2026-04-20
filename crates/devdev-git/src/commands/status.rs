//! `git status` (read-only).
//!
//! We only inspect the on-disk (temp) checkout the loader created — the
//! VFS isn't a true working tree here, so "working tree" = "the extracted
//! temp repo at load time". That suffices for the P0 acceptance bullet
//! (agent sees modified files if they were modified before load).

use git2::{Repository, Status, StatusOptions};

use super::GitResult;

pub fn run(repo: &Repository, args: &[String], _cwd: &str) -> GitResult {
    let short = args.iter().any(|a| a == "--short" || a == "-s");

    let mut opts = StatusOptions::new();
    opts.include_untracked(true).renames_head_to_index(true);
    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        Err(e) => return GitResult::err(format!("git status: {e}"), 128),
    };

    let mut out = Vec::new();
    if short {
        for entry in statuses.iter() {
            let s = entry.status();
            let (x, y) = short_codes(s);
            let path = entry.path().unwrap_or("");
            out.extend_from_slice(format!("{x}{y} {path}\n").as_bytes());
        }
        return GitResult::ok(out);
    }

    // Long format (abbreviated to the sections the agent needs).
    let branch = current_branch(repo).unwrap_or_else(|| "HEAD".into());
    out.extend_from_slice(format!("On branch {branch}\n").as_bytes());

    let (staged, unstaged, untracked) = partition(&statuses);

    if !staged.is_empty() {
        out.extend_from_slice(b"\nChanges to be committed:\n");
        for (label, path) in &staged {
            out.extend_from_slice(format!("\t{label:<12}{path}\n").as_bytes());
        }
    }
    if !unstaged.is_empty() {
        out.extend_from_slice(b"\nChanges not staged for commit:\n");
        for (label, path) in &unstaged {
            out.extend_from_slice(format!("\t{label:<12}{path}\n").as_bytes());
        }
    }
    if !untracked.is_empty() {
        out.extend_from_slice(b"\nUntracked files:\n");
        for path in &untracked {
            out.extend_from_slice(format!("\t{path}\n").as_bytes());
        }
    }
    if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() {
        out.extend_from_slice(b"\nnothing to commit, working tree clean\n");
    }

    GitResult::ok(out)
}

fn current_branch(repo: &Repository) -> Option<String> {
    let head = repo.head().ok()?;
    if head.is_branch() {
        head.shorthand().map(|s| s.to_owned())
    } else {
        None
    }
}

type StatusLine = (&'static str, String);
type StatusGroups = (Vec<StatusLine>, Vec<StatusLine>, Vec<String>);

fn partition(statuses: &git2::Statuses<'_>) -> StatusGroups {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();
    for entry in statuses.iter() {
        let s = entry.status();
        let Some(path) = entry.path() else { continue };
        if s.contains(Status::WT_NEW) {
            untracked.push(path.to_owned());
        }
        if s.contains(Status::INDEX_NEW) {
            staged.push(("new file:", path.to_owned()));
        }
        if s.contains(Status::INDEX_MODIFIED) {
            staged.push(("modified:", path.to_owned()));
        }
        if s.contains(Status::INDEX_DELETED) {
            staged.push(("deleted:", path.to_owned()));
        }
        if s.contains(Status::INDEX_RENAMED) {
            staged.push(("renamed:", path.to_owned()));
        }
        if s.contains(Status::WT_MODIFIED) {
            unstaged.push(("modified:", path.to_owned()));
        }
        if s.contains(Status::WT_DELETED) {
            unstaged.push(("deleted:", path.to_owned()));
        }
    }
    (staged, unstaged, untracked)
}

fn short_codes(s: Status) -> (char, char) {
    let x = if s.contains(Status::INDEX_NEW) {
        'A'
    } else if s.contains(Status::INDEX_MODIFIED) {
        'M'
    } else if s.contains(Status::INDEX_DELETED) {
        'D'
    } else if s.contains(Status::INDEX_RENAMED) {
        'R'
    } else {
        ' '
    };
    let y = if s.contains(Status::WT_NEW) {
        '?'
    } else if s.contains(Status::WT_MODIFIED) {
        'M'
    } else if s.contains(Status::WT_DELETED) {
        'D'
    } else {
        ' '
    };
    (x, y)
}
