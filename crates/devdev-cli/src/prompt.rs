//! Prompt-string formatting for [`crate::evaluate`].
//!
//! One pure function produces the `String` that the evaluator hands to
//! `AcpClient::prompt`. No template engine — a golden test pins the
//! exact shape. Preference files appear in declaration order; empty
//! optional sections are omitted entirely (no stray headers).

use crate::config::EvalContext;

/// Build the prompt text for one evaluation.
///
/// `repo_name` is the last path component of the host repo root,
/// already extracted by the caller so this function stays pure.
pub fn format_prompt(ctx: &EvalContext, repo_name: &str) -> String {
    let mut out = String::with_capacity(512);

    // Header.
    out.push_str("You are reviewing code in ");
    out.push_str(repo_name);
    out.push_str(". ");
    out.push_str(&ctx.task);
    out.push_str("\n\n");

    // Preferences section — omit the whole block when empty.
    if !ctx.preferences.is_empty() {
        out.push_str("## Preferences\n\n");
        for (idx, pref) in ctx.preferences.iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            out.push_str("### ");
            out.push_str(&pref.name);
            out.push('\n');
            out.push_str(&pref.content);
            if !pref.content.ends_with('\n') {
                out.push('\n');
            }
        }
        out.push('\n');
    }

    // Diff section — fenced with ```diff.
    if let Some(diff) = &ctx.diff
        && !diff.is_empty()
    {
        out.push_str("## Changes to Review\n\n");
        out.push_str("```diff\n");
        out.push_str(diff);
        if !diff.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    }

    // Focus paths.
    if !ctx.focus_paths.is_empty() {
        out.push_str("Focus on these files: ");
        out.push_str(&ctx.focus_paths.join(", "));
        out.push_str("\n\n");
    }

    out.push_str(
        "Evaluate the changes against the preferences. \
         Report any violations found. If no violations, \
         say \"No issues found.\"\n",
    );

    out
}
