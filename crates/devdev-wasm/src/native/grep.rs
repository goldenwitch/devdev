//! Native `grep` implementation backed by the `regex` crate + VFS walk.
//!
//! Flag surface (per `capabilities/04-tool-registry.md`):
//!   -r / -R   recurse into directories
//!   -n        prefix matches with `line:`
//!   -i        case-insensitive
//!   -l        list filenames only
//!   -v        invert match
//!   -F        fixed string (not regex)
//!   -w        whole-word match
//!   -c        count matches per file
//!
//! Exit codes follow GNU grep:
//!   0 — matches found
//!   1 — no matches
//!   2 — error

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use devdev_vfs::{FileType, MemFs};
use regex::{Regex, RegexBuilder};

use crate::native::NativeTool;
use crate::registry::ToolResult;

pub(crate) struct Grep;

#[derive(Default)]
struct Options {
    recursive: bool,
    line_number: bool,
    ignore_case: bool,
    files_with_matches: bool,
    invert: bool,
    fixed_string: bool,
    whole_word: bool,
    count: bool,
}

fn parse_args(args: &[String]) -> Result<(Options, String, Vec<String>), String> {
    let mut opts = Options::default();
    let mut positional: Vec<String> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            positional.extend(iter.by_ref().cloned());
            break;
        }
        if let Some(rest) = arg.strip_prefix("--") {
            match rest {
                "recursive" => opts.recursive = true,
                "line-number" => opts.line_number = true,
                "ignore-case" => opts.ignore_case = true,
                "files-with-matches" => opts.files_with_matches = true,
                "invert-match" => opts.invert = true,
                "fixed-strings" => opts.fixed_string = true,
                "word-regexp" => opts.whole_word = true,
                "count" => opts.count = true,
                _ => return Err(format!("grep: unrecognized option '--{rest}'")),
            }
            continue;
        }
        if let Some(short) = arg.strip_prefix('-') {
            if short.is_empty() {
                // bare `-` = stdin path marker
                positional.push(arg.clone());
                continue;
            }
            for ch in short.chars() {
                match ch {
                    'r' | 'R' => opts.recursive = true,
                    'n' => opts.line_number = true,
                    'i' => opts.ignore_case = true,
                    'l' => opts.files_with_matches = true,
                    'v' => opts.invert = true,
                    'F' => opts.fixed_string = true,
                    'w' => opts.whole_word = true,
                    'c' => opts.count = true,
                    _ => return Err(format!("grep: invalid option -- '{ch}'")),
                }
            }
            continue;
        }
        positional.push(arg.clone());
    }
    if positional.is_empty() {
        return Err("grep: missing PATTERN".into());
    }
    let pattern = positional.remove(0);
    Ok((opts, pattern, positional))
}

fn build_regex(pattern: &str, opts: &Options) -> Result<Regex, String> {
    let mut source = if opts.fixed_string {
        regex::escape(pattern)
    } else {
        pattern.to_owned()
    };
    if opts.whole_word {
        source = format!(r"\b(?:{source})\b");
    }
    RegexBuilder::new(&source)
        .case_insensitive(opts.ignore_case)
        .build()
        .map_err(|e| format!("grep: {e}"))
}

impl NativeTool for Grep {
    fn execute(
        &self,
        args: &[String],
        stdin: &[u8],
        _env: &HashMap<String, String>,
        cwd: &str,
        fs: &MemFs,
    ) -> ToolResult {
        let (opts, pattern, paths) = match parse_args(args) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult {
                    stdout: Vec::new(),
                    stderr: format!("{e}\n").into_bytes(),
                    exit_code: 2,
                };
            }
        };
        let re = match build_regex(&pattern, &opts) {
            Ok(re) => re,
            Err(e) => {
                return ToolResult {
                    stdout: Vec::new(),
                    stderr: format!("{e}\n").into_bytes(),
                    exit_code: 2,
                };
            }
        };

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut any_match = false;
        let mut had_error = false;

        // If no paths given, read stdin.
        if paths.is_empty() {
            let text = String::from_utf8_lossy(stdin);
            let matched = grep_text(&re, &opts, &text, None, &mut stdout);
            any_match |= matched;
        } else {
            // Determine whether to print filename prefix (any path argument implies yes
            // when multiple paths or recursive; GNU grep prints filename when >1 source).
            let multiple_sources = paths.len() > 1 || opts.recursive;
            for raw in &paths {
                let full = resolve(cwd, raw);
                let ft = match fs.stat(&full) {
                    Ok(s) => s.file_type,
                    Err(e) => {
                        stderr.extend_from_slice(
                            format!("grep: {}: {e}\n", full.display()).as_bytes(),
                        );
                        had_error = true;
                        continue;
                    }
                };
                match ft {
                    FileType::File => {
                        let name = display_path(raw);
                        let content = match fs.read(&full) {
                            Ok(b) => b,
                            Err(e) => {
                                stderr.extend_from_slice(
                                    format!("grep: {raw}: {e}\n").as_bytes(),
                                );
                                had_error = true;
                                continue;
                            }
                        };
                        let text = String::from_utf8_lossy(&content);
                        let label = if multiple_sources { Some(name.as_str()) } else { None };
                        any_match |= grep_text(&re, &opts, &text, label, &mut stdout);
                    }
                    FileType::Directory => {
                        if !opts.recursive {
                            stderr.extend_from_slice(
                                format!("grep: {raw}: Is a directory\n").as_bytes(),
                            );
                            had_error = true;
                            continue;
                        }
                        let files = walk_files(fs, &full);
                        for path in files {
                            let rel = relative_label(raw, &full, &path);
                            let content = match fs.read(&path) {
                                Ok(b) => b,
                                Err(e) => {
                                    stderr.extend_from_slice(
                                        format!("grep: {rel}: {e}\n").as_bytes(),
                                    );
                                    had_error = true;
                                    continue;
                                }
                            };
                            let text = String::from_utf8_lossy(&content);
                            any_match |= grep_text(&re, &opts, &text, Some(&rel), &mut stdout);
                        }
                    }
                    FileType::Symlink => {
                        // Spec punts symlink handling — skip quietly.
                    }
                }
            }
        }

        let exit_code = if had_error {
            2
        } else if any_match {
            0
        } else {
            1
        };
        ToolResult {
            stdout,
            stderr,
            exit_code,
        }
    }
}

/// Apply the regex to `text`, writing formatted matches to `out`.
/// Returns true if any line matched (after inversion is applied).
fn grep_text(
    re: &Regex,
    opts: &Options,
    text: &str,
    label: Option<&str>,
    out: &mut Vec<u8>,
) -> bool {
    let mut matches = 0usize;
    let mut line_hits: Vec<(usize, &str)> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let hit = re.is_match(line);
        let keep = if opts.invert { !hit } else { hit };
        if keep {
            matches += 1;
            line_hits.push((idx + 1, line));
        }
    }

    if matches == 0 {
        return false;
    }

    if opts.files_with_matches {
        if let Some(l) = label {
            out.extend_from_slice(l.as_bytes());
            out.push(b'\n');
        }
        return true;
    }
    if opts.count {
        if let Some(l) = label {
            out.extend_from_slice(format!("{l}:{matches}\n").as_bytes());
        } else {
            out.extend_from_slice(format!("{matches}\n").as_bytes());
        }
        return true;
    }

    for (lineno, line) in line_hits {
        if let Some(l) = label {
            out.extend_from_slice(l.as_bytes());
            out.push(b':');
        }
        if opts.line_number {
            out.extend_from_slice(format!("{lineno}:").as_bytes());
        }
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    true
}

/// Produce an absolute VFS path by resolving `raw` against `cwd`.
fn resolve(cwd: &str, raw: &str) -> PathBuf {
    if raw.starts_with('/') {
        PathBuf::from(raw)
    } else if cwd == "/" {
        PathBuf::from(format!("/{raw}"))
    } else {
        PathBuf::from(format!("{cwd}/{raw}"))
    }
}

fn display_path(raw: &str) -> String {
    raw.to_owned()
}

/// Build the label for a file discovered during recursion, preserving the
/// user-supplied prefix `raw` so output reads e.g. `src/foo/bar.rs`.
fn relative_label(raw: &str, root_abs: &Path, file_abs: &Path) -> String {
    let root = root_abs.to_string_lossy();
    let file = file_abs.to_string_lossy();
    let suffix = file.strip_prefix(root.as_ref()).unwrap_or(&file);
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() {
        raw.to_owned()
    } else if raw.ends_with('/') {
        format!("{raw}{suffix}")
    } else {
        format!("{raw}/{suffix}")
    }
}

/// Depth-first walk of `root`, collecting absolute paths of every regular file.
fn walk_files(fs: &MemFs, root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_rec(fs, root, &mut out);
    out
}

fn walk_rec(fs: &MemFs, dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs.list(dir) {
        Ok(v) => v,
        Err(_) => return,
    };
    for entry in entries {
        match entry.file_type {
            FileType::File => out.push(entry.path),
            FileType::Directory => walk_rec(fs, &entry.path, out),
            FileType::Symlink => {}
        }
    }
}
