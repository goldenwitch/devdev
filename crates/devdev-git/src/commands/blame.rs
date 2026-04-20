//! `git blame <file>` — line-by-line commit attribution.

use std::path::Path;

use git2::Repository;

use super::GitResult;

pub fn run(repo: &Repository, args: &[String]) -> GitResult {
    let Some(file) = args.iter().find(|a| !a.starts_with('-')).cloned() else {
        return GitResult::err("git blame: file argument required", 128);
    };

    let blame = match repo.blame_file(Path::new(&file), None) {
        Ok(b) => b,
        Err(e) => return GitResult::err(format!("git blame: {e}"), 128),
    };

    // Read the file contents from the HEAD tree so blame can display the
    // line text alongside the attribution.
    let head_tree = match repo.head().and_then(|h| h.peel_to_tree()) {
        Ok(t) => t,
        Err(e) => return GitResult::err(format!("git blame: {e}"), 128),
    };
    let entry = match head_tree.get_path(Path::new(&file)) {
        Ok(e) => e,
        Err(e) => return GitResult::err(format!("git blame: {file}: {e}"), 128),
    };
    let blob = match repo.find_blob(entry.id()) {
        Ok(b) => b,
        Err(e) => return GitResult::err(format!("git blame: {e}"), 128),
    };
    let content = String::from_utf8_lossy(blob.content()).into_owned();

    let mut out = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let Some(hunk) = blame.get_line(line_no) else {
            continue;
        };
        let sig = hunk.final_signature();
        let author = sig.name().unwrap_or("");
        let sha = hunk.final_commit_id();
        let sha_short: String = sha.to_string().chars().take(8).collect();
        out.extend_from_slice(
            format!(
                "{} ({} {:>4}) {}\n",
                sha_short,
                author,
                line_no,
                line
            )
            .as_bytes(),
        );
    }

    GitResult::ok(out)
}
