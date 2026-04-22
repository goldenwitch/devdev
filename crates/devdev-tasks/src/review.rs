//! Review parsing: extract structured comments from agent text.
//!
//! The agent is prompted to format comments as `[file:line] comment text`.
//! This module parses those out. Unparseable text becomes the review body.

/// A structured review extracted from agent text.
#[derive(Debug, Clone)]
pub struct ParsedReview {
    /// General review body (non-structured text).
    pub body: String,
    /// Inline comments extracted from structured markers.
    pub comments: Vec<InlineComment>,
}

/// A single inline review comment.
#[derive(Debug, Clone)]
pub struct InlineComment {
    pub path: String,
    pub line: u64,
    pub body: String,
}

/// Parse agent text into a structured review.
///
/// Looks for lines matching `[path:line] comment text`.
/// Everything else becomes the review body.
pub fn parse_review(text: &str) -> ParsedReview {
    let mut body_lines = Vec::new();
    let mut comments = Vec::new();

    for line in text.lines() {
        if let Some(comment) = try_parse_inline(line) {
            comments.push(comment);
        } else {
            body_lines.push(line);
        }
    }

    // Trim leading/trailing empty lines from body.
    let body = body_lines.join("\n").trim().to_string();

    ParsedReview { body, comments }
}

fn try_parse_inline(line: &str) -> Option<InlineComment> {
    let line = line.trim();

    // Must start with `[`
    if !line.starts_with('[') {
        return None;
    }

    let close_bracket = line.find(']')?;
    let marker = &line[1..close_bracket];

    // Must contain exactly one ':'
    let (path, line_str) = marker.split_once(':')?;

    let path = path.trim();
    let line_num: u64 = line_str.trim().parse().ok()?;

    if path.is_empty() || line_num == 0 {
        return None;
    }

    // Comment body is everything after `] `.
    let body = line[close_bracket + 1..].trim().to_string();

    if body.is_empty() {
        return None;
    }

    Some(InlineComment {
        path: path.to_string(),
        line: line_num,
        body,
    })
}
