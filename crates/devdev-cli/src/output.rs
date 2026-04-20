//! Output formatters for the `devdev` binary.
//!
//! Two modes:
//!
//! * [`render_human`] — the default, human-readable form documented in
//!   `capabilities/14-test-harness.md`. Deliberately unstable: used
//!   only for interactive runs.
//! * [`render_json`] — the machine-readable form pinned by
//!   `json_output_matches_snapshot`. Every field that exists on
//!   [`EvalResult`] is emitted; nothing else is added silently.
//!
//! Neither formatter writes anywhere on its own — callers decide where
//! the bytes go (stdout for happy paths, stderr for diagnostics).

use std::fmt::Write as _;

use serde::Serialize;

use crate::config::{EvalResult, RepoStats, ToolCallLog};

/// Render [`EvalResult`] in the human-readable form shown to a user at
/// the terminal. Always ends with a single trailing newline.
pub fn render_human(result: &EvalResult) -> String {
    let mut out = String::with_capacity(512);

    out.push_str("Agent is evaluating...\n");
    for tc in &result.tool_calls {
        let _ = writeln!(
            out,
            "  [tool] {}  (exit {}, {})",
            tc.command,
            tc.exit_code,
            fmt_duration(tc.duration.as_secs_f64()),
        );
    }

    out.push_str("\n─── Verdict ───\n");
    out.push_str(&result.verdict);
    if !result.verdict.ends_with('\n') {
        out.push('\n');
    }

    let _ = writeln!(
        out,
        "\nEvaluation complete ({} tool {}, {})",
        result.tool_calls.len(),
        if result.tool_calls.len() == 1 { "call" } else { "calls" },
        fmt_duration(result.duration.as_secs_f64()),
    );

    out
}

/// Render [`EvalResult`] as a single-line JSON object with a trailing
/// newline. The shape is contractual — see
/// `tests/acceptance_cli.rs::json_output_matches_snapshot`.
pub fn render_json(result: &EvalResult) -> String {
    let view = JsonView::from(result);
    let mut s = serde_json::to_string(&view).expect("EvalResult → JSON is total");
    s.push('\n');
    s
}

/// Lightweight serialisable mirror of [`EvalResult`]. Keeping it
/// separate from the public struct means the JSON contract cannot
/// drift accidentally when new fields are added to [`EvalResult`].
#[derive(Serialize)]
struct JsonView<'a> {
    verdict: &'a str,
    stop_reason: &'a str,
    tool_calls: Vec<JsonToolCall<'a>>,
    duration_ms: u128,
    is_git_repo: bool,
    repo_stats: JsonRepoStats,
}

#[derive(Serialize)]
struct JsonToolCall<'a> {
    command: &'a str,
    exit_code: i32,
    duration_ms: u128,
}

#[derive(Serialize)]
struct JsonRepoStats {
    files: u64,
    bytes: u64,
}

impl<'a> From<&'a EvalResult> for JsonView<'a> {
    fn from(r: &'a EvalResult) -> Self {
        Self {
            verdict: &r.verdict,
            stop_reason: &r.stop_reason,
            tool_calls: r.tool_calls.iter().map(JsonToolCall::from).collect(),
            duration_ms: r.duration.as_millis(),
            is_git_repo: r.is_git_repo,
            repo_stats: JsonRepoStats::from(&r.repo_stats),
        }
    }
}

impl<'a> From<&'a ToolCallLog> for JsonToolCall<'a> {
    fn from(tc: &'a ToolCallLog) -> Self {
        Self {
            command: &tc.command,
            exit_code: tc.exit_code,
            duration_ms: tc.duration.as_millis(),
        }
    }
}

impl From<&RepoStats> for JsonRepoStats {
    fn from(s: &RepoStats) -> Self {
        Self { files: s.files, bytes: s.bytes }
    }
}

/// `0.8s`, `34.2s`, `1.0s`. Matches the shape in
/// `capabilities/14-test-harness.md`.
fn fmt_duration(secs: f64) -> String {
    format!("{secs:.1}s")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn sample_result() -> EvalResult {
        EvalResult {
            verdict: "looks ok".into(),
            stop_reason: "end_turn".into(),
            tool_calls: vec![
                ToolCallLog {
                    command: "grep -n foo src/".into(),
                    exit_code: 0,
                    duration: Duration::from_millis(823),
                },
                ToolCallLog {
                    command: "git log -1".into(),
                    exit_code: 0,
                    duration: Duration::from_millis(312),
                },
            ],
            duration: Duration::from_millis(1234),
            is_git_repo: true,
            repo_stats: RepoStats { files: 3, bytes: 42 },
        }
    }

    #[test]
    fn human_lists_every_tool_call_in_order() {
        let s = render_human(&sample_result());
        let grep_at = s.find("grep -n foo src/").expect("grep line");
        let log_at = s.find("git log -1").expect("git log line");
        assert!(grep_at < log_at, "tool calls out of order");
        assert!(s.contains("Evaluation complete (2 tool calls"));
    }

    #[test]
    fn human_pluralises_single_tool_call() {
        let mut r = sample_result();
        r.tool_calls.truncate(1);
        let s = render_human(&r);
        assert!(s.contains("Evaluation complete (1 tool call,"));
    }

    #[test]
    fn json_roundtrip_preserves_every_documented_field() {
        let r = sample_result();
        let s = render_json(&r);
        let v: serde_json::Value = serde_json::from_str(&s).expect("parses");
        assert_eq!(v["verdict"], "looks ok");
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(v["tool_calls"][0]["command"], "grep -n foo src/");
        assert_eq!(v["tool_calls"][0]["exit_code"], 0);
        assert_eq!(v["tool_calls"][0]["duration_ms"], 823);
        assert_eq!(v["duration_ms"], 1234);
        assert_eq!(v["is_git_repo"], true);
        assert_eq!(v["repo_stats"]["files"], 3);
        assert_eq!(v["repo_stats"]["bytes"], 42);
    }
}
