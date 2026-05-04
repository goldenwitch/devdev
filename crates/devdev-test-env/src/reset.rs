//! `reset-comments` implementation.
//!
//! Strategy: every test-issued comment is required to begin with the
//! manifest's `comment_tag_prefix` (`[devdev-live-test...]`). On
//! cleanup we sweep:
//!
//! - Every comment authored by a non-admin user (definitionally
//!   test-originated; admin tokens never run from test code).
//! - Every comment authored by the admin identity that *also*
//!   starts with the tag prefix (so admin-pinned canonical notes
//!   without the tag survive).
//!
//! On ADO we additionally skip system-status comments
//! (`commentType == "system"`) — those represent vote/policy events
//! and are not deletable anyway.

use crate::ado::{AdoClient, ThreadComment};
use crate::github::{GithubClient, IssueComment};
use crate::manifest::{AdoFixture, AdoLock, GithubFixture, GithubLock};

/// Sweep stray comments off the canonical GitHub PR.
///
/// `admin_login` is the GitHub login of the admin identity whose
/// token authored the canonical PR. Comments NOT by that login are
/// always removed; admin-authored comments are removed only if
/// they begin with the configured tag prefix.
pub async fn reset_github_comments(
    client: &GithubClient,
    fixture: &GithubFixture,
    lock: &GithubLock,
    admin_login: &str,
) -> anyhow::Result<usize> {
    let comments = client
        .list_pr_comments(&fixture.org, &fixture.repo, lock.canonical_pr_number)
        .await?;
    let mut deleted = 0usize;
    for c in comments {
        if should_delete_github(&c, admin_login, &fixture.comment_tag_prefix) {
            client
                .delete_issue_comment(&fixture.org, &fixture.repo, c.id)
                .await?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// Sweep stray comments off the canonical ADO PR. Returns the
/// number of comments deleted.
pub async fn reset_ado_comments(
    client: &AdoClient,
    fixture: &AdoFixture,
    lock: &AdoLock,
    admin_unique_name: &str,
) -> anyhow::Result<usize> {
    let threads = client
        .list_pr_threads(fixture, &lock.repo_id, lock.canonical_pr_id)
        .await?;
    let mut deleted = 0usize;
    for thread in threads {
        if thread.status.as_deref() == Some("closed") {
            continue;
        }
        for c in &thread.comments {
            if should_delete_ado(c, admin_unique_name, &fixture.comment_tag_prefix) {
                client
                    .delete_thread_comment(
                        fixture,
                        &lock.repo_id,
                        lock.canonical_pr_id,
                        thread.id,
                        c.id,
                    )
                    .await?;
                deleted += 1;
            }
        }
    }
    Ok(deleted)
}

fn should_delete_github(c: &IssueComment, admin: &str, tag_prefix: &str) -> bool {
    if c.user.login.eq_ignore_ascii_case(admin) {
        body_carries_tag(&c.body, tag_prefix)
    } else {
        true
    }
}

fn should_delete_ado(c: &ThreadComment, admin: &str, tag_prefix: &str) -> bool {
    if c.author.unique_name.eq_ignore_ascii_case(admin) {
        body_carries_tag(&c.content, tag_prefix)
    } else {
        true
    }
}

/// A comment "carries the tag" if its trimmed body opens with the
/// configured prefix's leading-marker portion. The manifest stores
/// `[devdev-live-test]` as the canonical prefix, but real test
/// comments use `[devdev-live-test:<test>:<nonce>]`, so we compare
/// against the prefix sans the trailing `]`.
fn body_carries_tag(body: &str, tag_prefix: &str) -> bool {
    let opener = tag_prefix.strip_suffix(']').unwrap_or(tag_prefix);
    body.trim_start().starts_with(opener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::CommentUser;

    fn gh(body: &str, login: &str) -> IssueComment {
        IssueComment {
            id: 1,
            body: body.into(),
            user: CommentUser {
                login: login.into(),
            },
        }
    }

    #[test]
    fn github_keeps_admin_canonical_note() {
        let c = gh("Canonical reviewer note (do not delete).", "devdev-bot");
        assert!(!should_delete_github(&c, "devdev-bot", "[devdev-live-test]"));
    }

    #[test]
    fn github_deletes_admin_tagged_comment() {
        let c = gh("[devdev-live-test:foo:42] hello", "devdev-bot");
        assert!(should_delete_github(&c, "devdev-bot", "[devdev-live-test]"));
    }

    #[test]
    fn github_deletes_any_non_admin_comment() {
        let c = gh("hi from a contributor", "someone-else");
        assert!(should_delete_github(&c, "devdev-bot", "[devdev-live-test]"));
    }

    #[test]
    fn github_login_match_is_case_insensitive() {
        let c = gh("plain admin note", "DevDev-Bot");
        assert!(!should_delete_github(&c, "devdev-bot", "[devdev-live-test]"));
    }

    #[test]
    fn github_tag_must_be_at_start_after_trim() {
        // Leading whitespace is fine; tag in the middle is not.
        let leading = gh("   [devdev-live-test] yo", "devdev-bot");
        assert!(should_delete_github(&leading, "devdev-bot", "[devdev-live-test]"));
        let middle = gh("hi [devdev-live-test] yo", "devdev-bot");
        assert!(!should_delete_github(&middle, "devdev-bot", "[devdev-live-test]"));
    }
}
