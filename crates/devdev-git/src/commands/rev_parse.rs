//! `git rev-parse <ref>` — resolve a ref to a full 40-char SHA.

use git2::Repository;

use super::GitResult;

pub fn run(repo: &Repository, args: &[String]) -> GitResult {
    if args.is_empty() {
        return GitResult::err("git rev-parse: ref argument required", 128);
    }
    let mut out = Vec::new();
    for spec in args {
        match repo.revparse_single(spec) {
            Ok(obj) => {
                out.extend_from_slice(obj.id().to_string().as_bytes());
                out.push(b'\n');
            }
            Err(e) => return GitResult::err(format!("git rev-parse: {spec}: {e}"), 128),
        }
    }
    GitResult::ok(out)
}
