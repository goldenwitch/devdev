//! Catalog integrity: every `spirit/scenarios/S*.md` pairs 1:1 with
//! a `#[tokio::test]` of the matching name in `scenarios.rs`.
//!
//! This guards against the two silent drift modes: orphan markdown
//! files that no test exercises, and tests that claim to implement
//! a scenario that doesn't exist on disk.
//!
//! The test reads both files as text (no extra deps) and compares
//! the ID sets.

use std::collections::BTreeSet;
use std::fs;

use devdev_scenarios::workspace_root;

/// IDs declared by markdown catalog files.
fn markdown_ids() -> BTreeSet<String> {
    let dir = workspace_root().join("spirit").join("scenarios");
    let mut ids = BTreeSet::new();
    for entry in fs::read_dir(&dir).expect("read scenarios dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('S') || !name.ends_with(".md") {
            continue;
        }
        // File name is `S<id>-<slug>.md` — the ID is the
        // `S<digits>` prefix.
        let id: String = name
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        ids.insert(id);
    }
    ids
}

/// IDs declared by test functions in `scenarios.rs`. The naming
/// convention is `s<id>_<slug>` (lowercase); we reverse it to
/// `S<id>` for comparison.
fn test_ids() -> BTreeSet<String> {
    let path = workspace_root()
        .join("crates")
        .join("devdev-scenarios")
        .join("tests")
        .join("scenarios.rs");
    let text = fs::read_to_string(&path).expect("read scenarios.rs");

    let mut ids = BTreeSet::new();
    for line in text.lines() {
        // Match `async fn s<digits>_...(` — catalog entries only.
        // Sub-scenarios (e.g., `s06_checkpoint_missing_is_fresh_start`)
        // are deliberately excluded: they share an ID with a catalog
        // entry and don't need their own markdown.
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("async fn s") {
            let mut digits = String::new();
            for ch in rest.chars() {
                if ch.is_ascii_digit() {
                    digits.push(ch);
                } else {
                    break;
                }
            }
            if digits.is_empty() {
                continue;
            }
            // Only count the *primary* scenario fn per ID — the one
            // whose name immediately ends with `_<slug>` referencing
            // the same id prefix. This works because `s06_checkpoint_round_trip`
            // is lexically before `s06_checkpoint_missing...` in this
            // file; we simply dedupe by ID.
            ids.insert(format!("S{digits}"));
        }
    }
    ids
}

#[test]
fn catalog_and_tests_match() {
    let md = markdown_ids();
    let tests = test_ids();
    assert_eq!(
        md, tests,
        "scenario catalog drift:\n  markdown: {md:?}\n  tests:    {tests:?}\n\
         Every S*.md needs a matching `async fn s<id>_<slug>` and vice versa."
    );
}
