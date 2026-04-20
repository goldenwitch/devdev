//! Variable + glob expansion.
//!
//! Turns parser AST [`Word`]s into the fully-resolved argv strings the
//! dispatcher hands to builtins / git / the tool engine. Expansion happens
//! after parsing and before dispatch — see `capabilities/09-shell-executor.md`.

use std::path::Path;

use devdev_vfs::MemFs;

use crate::ast::{Word, WordPart};
use crate::state::ShellState;

/// Expand a single [`Word`] into zero or more argv strings.
///
/// Rules:
/// - `$VAR` / `${VAR}` → `state.env[VAR]` (empty if unset).
/// - `$?` → `state.last_exit_code` as a string.
/// - Literal parts pass through.
/// - An unquoted `GlobPattern` expands against the VFS; matches replace the
///   pattern and the word may fan out into multiple results. If the glob
///   yields no matches we keep the literal pattern (bash with `nullglob` off).
/// - A word containing a glob with matches is concatenated with its
///   surrounding literal/variable parts *before* fan-out. In practice we
///   only support one glob part per word; composite globs (`src/$DIR/*.rs`
///   where `$DIR=foo`) work because the variable expands first and we
///   rebuild one combined pattern from all parts before matching.
pub fn expand_word(word: &Word, state: &ShellState, vfs: &MemFs) -> Vec<String> {
    // First pass: render each part to a string, remembering which of the
    // resulting fragments is the glob pattern (if any).
    let mut prefix = String::new();
    let mut glob: Option<String> = None;
    let mut suffix = String::new();

    for part in &word.parts {
        match part {
            WordPart::Literal(s) => {
                if glob.is_none() {
                    prefix.push_str(s);
                } else {
                    suffix.push_str(s);
                }
            }
            WordPart::Variable(name) => {
                let v = state.env.get(name).map(String::as_str).unwrap_or("");
                if glob.is_none() {
                    prefix.push_str(v);
                } else {
                    suffix.push_str(v);
                }
            }
            WordPart::LastExitCode => {
                let v = state.last_exit_code.to_string();
                if glob.is_none() {
                    prefix.push_str(&v);
                } else {
                    suffix.push_str(&v);
                }
            }
            WordPart::GlobPattern(g) => {
                if word.quoted || glob.is_some() {
                    // Quoted or second glob — treat as literal.
                    if glob.is_none() {
                        prefix.push_str(g);
                    } else {
                        suffix.push_str(g);
                    }
                } else {
                    glob = Some(g.clone());
                }
            }
        }
    }

    let Some(g) = glob else {
        return vec![prefix + &suffix];
    };

    let pattern = format!("{prefix}{g}{suffix}");
    let matches = devdev_vfs::glob::expand(&pattern, Path::new(&state.cwd), vfs.tree())
        .unwrap_or_default();

    if matches.is_empty() {
        return vec![pattern];
    }

    matches
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Expand every word of a command's argv, flattening the results.
pub fn expand_words(words: &[Word], state: &ShellState, vfs: &MemFs) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    for w in words {
        out.extend(expand_word(w, state, vfs));
    }
    out
}
