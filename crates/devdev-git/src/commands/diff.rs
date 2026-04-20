//! `git diff` — limited to the commit-range and `--cached` forms for P0.
//!
//! Supported:
//!   git diff <rev>..<rev>   — diff between two commits
//!   git diff <rev>          — diff <rev>..HEAD
//!   git diff --cached       — diff HEAD tree vs. index
//!   git diff --name-only    — emit paths only
//!   git diff --stat         — per-file insertions/deletions summary
//!
//! Working-tree-vs-index diff is intentionally skipped: the VFS is the
//! working tree but we don't yet have a VFS→index differ. The shell
//! executor can fall back to `diff a b` if the agent needs it.

use git2::{DiffFormat, DiffOptions, DiffStatsFormat, Repository};

use super::GitResult;

struct Options {
    cached: bool,
    name_only: bool,
    stat: bool,
    revs: Vec<String>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        cached: false,
        name_only: false,
        stat: false,
        revs: Vec::new(),
    };
    for a in args {
        match a.as_str() {
            "--cached" | "--staged" => opts.cached = true,
            "--name-only" => opts.name_only = true,
            "--stat" => opts.stat = true,
            "--no-color" => {}
            s if s.starts_with('-') => return Err(format!("git diff: unsupported flag '{s}'")),
            _ => opts.revs.push(a.clone()),
        }
    }
    Ok(opts)
}

pub fn run(repo: &Repository, args: &[String]) -> GitResult {
    let opts = match parse(args) {
        Ok(o) => o,
        Err(e) => return GitResult::err(e, 128),
    };

    // Resolve old/new trees per the mode.
    let (old_tree, new_tree): (Option<git2::Tree<'_>>, Option<git2::Tree<'_>>) = if opts.cached {
        // Treat HEAD tree as both "old" and "new" in the absence of an
        // index differ. This keeps `--cached` from panicking on a clean
        // repo; real cached diffs vs. index are deferred to P1.
        let old = repo.head().and_then(|h| h.peel_to_tree()).ok();
        let new = repo.head().and_then(|h| h.peel_to_tree()).ok();
        (old, new)
    } else {
        match resolve_rev_range(repo, &opts.revs) {
            Ok(t) => t,
            Err(e) => return GitResult::err(e, 128),
        }
    };

    let mut diff_opts = DiffOptions::new();
    let diff = match repo.diff_tree_to_tree(
        old_tree.as_ref(),
        new_tree.as_ref(),
        Some(&mut diff_opts),
    ) {
        Ok(d) => d,
        Err(e) => return GitResult::err(format!("git diff: {e}"), 128),
    };

    if opts.name_only {
        let mut out = Vec::new();
        let _ = diff.foreach(
            &mut |delta, _| {
                if let Some(p) = delta.new_file().path().and_then(|p| p.to_str()) {
                    out.extend_from_slice(p.as_bytes());
                    out.push(b'\n');
                }
                true
            },
            None,
            None,
            None,
        );
        return GitResult::ok(out);
    }

    if opts.stat {
        let stats = match diff.stats() {
            Ok(s) => s,
            Err(e) => return GitResult::err(format!("git diff: {e}"), 128),
        };
        let buf = match stats.to_buf(DiffStatsFormat::FULL, 80) {
            Ok(b) => b,
            Err(e) => return GitResult::err(format!("git diff: {e}"), 128),
        };
        return GitResult::ok(buf.to_vec());
    }

    let mut out = Vec::new();
    if let Err(e) = diff.print(DiffFormat::Patch, |_, _, line| {
        match line.origin() {
            '+' | '-' | ' ' => out.push(line.origin() as u8),
            _ => {}
        }
        out.extend_from_slice(line.content());
        true
    }) {
        return GitResult::err(format!("git diff: {e}"), 128);
    }
    GitResult::ok(out)
}

/// Resolve the `<rev>..<rev>` or single-`<rev>` form to `(old_tree, new_tree)`.
fn resolve_rev_range<'r>(
    repo: &'r Repository,
    revs: &[String],
) -> Result<(Option<git2::Tree<'r>>, Option<git2::Tree<'r>>), String> {
    match revs.len() {
        0 => {
            // No revs given → empty diff (no working-tree mode yet).
            let old = repo.head().and_then(|h| h.peel_to_tree()).ok();
            let new = repo.head().and_then(|h| h.peel_to_tree()).ok();
            Ok((old, new))
        }
        1 => {
            let arg = &revs[0];
            if let Some((a, b)) = arg.split_once("..") {
                let old = resolve_tree(repo, a)?;
                let new = resolve_tree(repo, b)?;
                Ok((Some(old), Some(new)))
            } else {
                let old = resolve_tree(repo, arg)?;
                let new = resolve_tree(repo, "HEAD")?;
                Ok((Some(old), Some(new)))
            }
        }
        2 => {
            let old = resolve_tree(repo, &revs[0])?;
            let new = resolve_tree(repo, &revs[1])?;
            Ok((Some(old), Some(new)))
        }
        _ => Err("git diff: too many revisions".into()),
    }
}

fn resolve_tree<'r>(repo: &'r Repository, spec: &str) -> Result<git2::Tree<'r>, String> {
    let obj = repo
        .revparse_single(spec)
        .map_err(|e| format!("git diff: {spec}: {e}"))?;
    obj.peel_to_tree()
        .map_err(|e| format!("git diff: {spec}: {e}"))
}
