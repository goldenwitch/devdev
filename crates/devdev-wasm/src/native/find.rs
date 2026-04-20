//! Native `find` implementation backed by `globset` + VFS walk.
//!
//! Flag surface (per `capabilities/04-tool-registry.md`):
//!   -name <glob>       match basename
//!   -iname <glob>      case-insensitive basename match
//!   -type f|d          filter by file or directory
//!   -maxdepth <N>      max recursion depth (0 = starting point only)
//!   -mindepth <N>      min depth
//!   -path <glob>       match full relative path
//!
//! Output: one absolute path per line, lexicographic within each directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use devdev_vfs::{FileType, MemFs};
use globset::GlobMatcher;

use crate::native::NativeTool;
use crate::registry::ToolResult;

pub(crate) struct Find;

enum TypeFilter {
    File,
    Dir,
}

#[derive(Default)]
struct Options {
    name: Option<GlobMatcher>,
    iname: Option<GlobMatcher>,
    path: Option<GlobMatcher>,
    type_: Option<TypeFilter>,
    maxdepth: Option<usize>,
    mindepth: Option<usize>,
}

fn parse_glob(pattern: &str, case_insensitive: bool) -> Result<GlobMatcher, String> {
    let mut builder = globset::GlobBuilder::new(pattern);
    builder.case_insensitive(case_insensitive);
    builder
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| format!("find: invalid glob '{pattern}': {e}"))
}

fn parse_args(args: &[String]) -> Result<(Vec<String>, Options), String> {
    let mut starts: Vec<String> = Vec::new();
    let mut opts = Options::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-name" => {
                let v = next_value(args, &mut i, "-name")?;
                opts.name = Some(parse_glob(&v, false)?);
            }
            "-iname" => {
                let v = next_value(args, &mut i, "-iname")?;
                opts.iname = Some(parse_glob(&v, true)?);
            }
            "-path" | "-wholename" => {
                let v = next_value(args, &mut i, "-path")?;
                opts.path = Some(parse_glob(&v, false)?);
            }
            "-type" => {
                let v = next_value(args, &mut i, "-type")?;
                opts.type_ = Some(match v.as_str() {
                    "f" => TypeFilter::File,
                    "d" => TypeFilter::Dir,
                    other => return Err(format!("find: unsupported -type '{other}'")),
                });
            }
            "-maxdepth" => {
                let v = next_value(args, &mut i, "-maxdepth")?;
                opts.maxdepth = Some(
                    v.parse()
                        .map_err(|_| format!("find: -maxdepth wants integer, got '{v}'"))?,
                );
            }
            "-mindepth" => {
                let v = next_value(args, &mut i, "-mindepth")?;
                opts.mindepth = Some(
                    v.parse()
                        .map_err(|_| format!("find: -mindepth wants integer, got '{v}'"))?,
                );
            }
            flag if flag.starts_with('-') => {
                return Err(format!("find: unsupported predicate '{flag}'"));
            }
            _ => {
                starts.push(a.clone());
                i += 1;
                continue;
            }
        }
    }
    if starts.is_empty() {
        starts.push(".".into());
    }
    Ok((starts, opts))
}

fn next_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    if *i >= args.len() {
        return Err(format!("find: {flag} requires an argument"));
    }
    let v = args[*i].clone();
    *i += 1;
    Ok(v)
}

impl NativeTool for Find {
    fn execute(
        &self,
        args: &[String],
        _stdin: &[u8],
        _env: &HashMap<String, String>,
        cwd: &str,
        fs: &MemFs,
    ) -> ToolResult {
        let (starts, opts) = match parse_args(args) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult {
                    stdout: Vec::new(),
                    stderr: format!("{e}\n").into_bytes(),
                    exit_code: 1,
                };
            }
        };

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut had_error = false;

        for start_raw in &starts {
            let start_abs = resolve(cwd, start_raw);
            if !fs.exists(&start_abs) {
                stderr.extend_from_slice(
                    format!(
                        "find: '{}': No such file or directory\n",
                        start_raw
                    )
                    .as_bytes(),
                );
                had_error = true;
                continue;
            }
            walk_and_emit(
                fs,
                start_raw,
                &start_abs,
                &start_abs,
                0,
                &opts,
                &mut stdout,
            );
        }

        ToolResult {
            stdout,
            stderr,
            exit_code: if had_error { 1 } else { 0 },
        }
    }
}

/// Depth-first walk rooted at `abs`, emitting matches to `out`.
///
/// * `display_root` — the literal path the user passed on the command line
///   (e.g. `"."`, `"src"`). Preserved as the prefix of every emitted line.
/// * `root_abs` — the absolute VFS path corresponding to `display_root`.
///   Immutable across recursion so relative suffixes compute correctly.
/// * `abs` — the node currently being visited.
fn walk_and_emit(
    fs: &MemFs,
    display_root: &str,
    root_abs: &Path,
    abs: &Path,
    depth: usize,
    opts: &Options,
    out: &mut Vec<u8>,
) {
    let stat = match fs.stat(abs) {
        Ok(s) => s,
        Err(_) => return,
    };

    let display = display_for(display_root, root_abs, abs);
    if matches_filters(&display, stat.file_type, depth, opts) {
        out.extend_from_slice(display.as_bytes());
        out.push(b'\n');
    }

    if stat.file_type != FileType::Directory {
        return;
    }
    if let Some(max) = opts.maxdepth
        && depth >= max
    {
        return;
    }
    let mut entries = match fs.list(abs) {
        Ok(v) => v,
        Err(_) => return,
    };
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in entries {
        walk_and_emit(
            fs,
            display_root,
            root_abs,
            &entry.path,
            depth + 1,
            opts,
            out,
        );
    }
}

fn matches_filters(path: &str, ft: FileType, depth: usize, opts: &Options) -> bool {
    if let Some(min) = opts.mindepth
        && depth < min
    {
        return false;
    }
    if let Some(tf) = &opts.type_ {
        match (tf, ft) {
            (TypeFilter::File, FileType::File) => {}
            (TypeFilter::Dir, FileType::Directory) => {}
            _ => return false,
        }
    }
    let basename = std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned());
    if let Some(g) = &opts.name
        && !g.is_match(&basename)
    {
        return false;
    }
    if let Some(g) = &opts.iname
        && !g.is_match(&basename)
    {
        return false;
    }
    if let Some(g) = &opts.path
        && !g.is_match(path)
    {
        return false;
    }
    true
}

/// Build the output label for `path`, preserving the user-supplied `display_root`
/// (e.g. `"."` stays as `./sub/file`, `"src"` becomes `src/sub/file`).
fn display_for(display_root: &str, root_abs: &Path, path_abs: &Path) -> String {
    let root = root_abs.to_string_lossy();
    let p = path_abs.to_string_lossy();
    if path_abs == root_abs {
        return display_root.to_owned();
    }
    let suffix = p.strip_prefix(root.as_ref()).unwrap_or(&p);
    let suffix = suffix.trim_start_matches('/');
    if display_root.ends_with('/') {
        format!("{display_root}{suffix}")
    } else {
        format!("{display_root}/{suffix}")
    }
}

fn resolve(cwd: &str, raw: &str) -> PathBuf {
    if raw == "." {
        return PathBuf::from(cwd);
    }
    if raw.starts_with('/') {
        PathBuf::from(raw)
    } else if cwd == "/" {
        PathBuf::from(format!("/{raw}"))
    } else {
        PathBuf::from(format!("{cwd}/{raw}"))
    }
}
