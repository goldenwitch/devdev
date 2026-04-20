//! Native `diff` implementation backed by the `similar` crate.
//!
//! Flag surface (per `capabilities/04-tool-registry.md`):
//!   -u / --unified[=N]   unified format (default; N context lines, default 3)
//!   -r / --recursive     recurse; diff file-by-file (P0 accepts flag but P0 only needs two-file diff)
//!   -N / --new-file      treat missing files as empty
//!   -q / --brief         only report whether files differ
//!   --no-color           accepted and ignored
//!
//! Exit codes: 0 = identical, 1 = differ, 2 = error.

use std::collections::HashMap;
use std::path::PathBuf;

use devdev_vfs::MemFs;
use similar::{ChangeTag, TextDiff};

use crate::native::NativeTool;
use crate::registry::ToolResult;

pub(crate) struct Diff;

struct Options {
    unified_context: usize,
    brief: bool,
    new_file: bool,
    _recursive: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            unified_context: 3,
            brief: false,
            new_file: false,
            _recursive: false,
        }
    }
}

fn parse_args(args: &[String]) -> Result<(Options, Vec<String>), String> {
    let mut opts = Options::default();
    let mut positional: Vec<String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-u" => opts.unified_context = 3,
            "-q" | "--brief" => opts.brief = true,
            "-r" | "--recursive" => opts._recursive = true,
            "-N" | "--new-file" => opts.new_file = true,
            "--no-color" => {}
            s if s.starts_with("--unified=") => {
                let v = &s["--unified=".len()..];
                opts.unified_context = v
                    .parse()
                    .map_err(|_| format!("diff: invalid context count '{v}'"))?;
            }
            s if s.starts_with('-') && s.len() > 1 && s.chars().nth(1).unwrap() == 'U' => {
                let v = &s[2..];
                opts.unified_context = v
                    .parse()
                    .map_err(|_| format!("diff: invalid context count '{v}'"))?;
            }
            s if s.starts_with('-') => {
                return Err(format!("diff: unrecognized option '{s}'"));
            }
            _ => positional.push(arg.clone()),
        }
    }
    Ok((opts, positional))
}

impl NativeTool for Diff {
    fn execute(
        &self,
        args: &[String],
        _stdin: &[u8],
        _env: &HashMap<String, String>,
        cwd: &str,
        fs: &MemFs,
    ) -> ToolResult {
        let (opts, paths) = match parse_args(args) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult {
                    stdout: Vec::new(),
                    stderr: format!("{e}\n").into_bytes(),
                    exit_code: 2,
                };
            }
        };
        if paths.len() != 2 {
            return ToolResult {
                stdout: Vec::new(),
                stderr: b"diff: expected exactly two file arguments\n".to_vec(),
                exit_code: 2,
            };
        }
        let (a_raw, b_raw) = (&paths[0], &paths[1]);
        let a_abs = resolve(cwd, a_raw);
        let b_abs = resolve(cwd, b_raw);

        let a_bytes = match read_or_empty(fs, &a_abs, opts.new_file) {
            Ok(b) => b,
            Err(e) => return err_result(&format!("diff: {a_raw}: {e}")),
        };
        let b_bytes = match read_or_empty(fs, &b_abs, opts.new_file) {
            Ok(b) => b,
            Err(e) => return err_result(&format!("diff: {b_raw}: {e}")),
        };

        if a_bytes == b_bytes {
            return ToolResult {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            };
        }

        if opts.brief {
            let line = format!("Files {a_raw} and {b_raw} differ\n");
            return ToolResult {
                stdout: line.into_bytes(),
                stderr: Vec::new(),
                exit_code: 1,
            };
        }

        let a_text = String::from_utf8_lossy(&a_bytes);
        let b_text = String::from_utf8_lossy(&b_bytes);
        let diff = TextDiff::from_lines(a_text.as_ref(), b_text.as_ref());
        let mut out = Vec::new();
        out.extend_from_slice(format!("--- a/{a_raw}\n").as_bytes());
        out.extend_from_slice(format!("+++ b/{b_raw}\n").as_bytes());

        for group in diff.grouped_ops(opts.unified_context).iter() {
            // Compute the hunk header from the first and last op.
            let (old_start, old_len, new_start, new_len) = hunk_range(group);
            out.extend_from_slice(
                format!(
                    "@@ -{},{} +{},{} @@\n",
                    old_start + 1,
                    old_len,
                    new_start + 1,
                    new_len
                )
                .as_bytes(),
            );
            for op in group {
                for change in diff.iter_changes(op) {
                    let sign = match change.tag() {
                        ChangeTag::Delete => '-',
                        ChangeTag::Insert => '+',
                        ChangeTag::Equal => ' ',
                    };
                    out.push(sign as u8);
                    out.extend_from_slice(change.value().as_bytes());
                    if !change.value().ends_with('\n') {
                        out.push(b'\n');
                        out.extend_from_slice(b"\\ No newline at end of file\n");
                    }
                }
            }
        }

        ToolResult {
            stdout: out,
            stderr: Vec::new(),
            exit_code: 1,
        }
    }
}

fn read_or_empty(
    fs: &MemFs,
    path: &std::path::Path,
    treat_missing_as_empty: bool,
) -> Result<Vec<u8>, String> {
    match fs.read(path) {
        Ok(b) => Ok(b),
        Err(devdev_vfs::VfsError::NotFound(_)) if treat_missing_as_empty => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

fn hunk_range(ops: &[similar::DiffOp]) -> (usize, usize, usize, usize) {
    let first = ops.first().expect("non-empty op group");
    let last = ops.last().expect("non-empty op group");
    let old_start = first.old_range().start;
    let new_start = first.new_range().start;
    let old_end = last.old_range().end;
    let new_end = last.new_range().end;
    (
        old_start,
        old_end.saturating_sub(old_start),
        new_start,
        new_end.saturating_sub(new_start),
    )
}

fn err_result(msg: &str) -> ToolResult {
    ToolResult {
        stdout: Vec::new(),
        stderr: format!("{msg}\n").into_bytes(),
        exit_code: 2,
    }
}

fn resolve(cwd: &str, raw: &str) -> PathBuf {
    if raw.starts_with('/') {
        PathBuf::from(raw)
    } else if cwd == "/" {
        PathBuf::from(format!("/{raw}"))
    } else {
        PathBuf::from(format!("{cwd}/{raw}"))
    }
}
