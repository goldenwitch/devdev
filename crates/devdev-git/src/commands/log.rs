//! `git log` — commit history.
//!
//! Supported flags (P0):
//!   --oneline           `<sha7> <subject>`
//!   -n <N> / -<N>       limit
//!   --author=<pat>      substring match against author name
//!   --format=<fmt>      supports %H %h %s %an %ae %ad
//!
//! Default format (no flags) matches `git log --no-color` medium format:
//!
//! ```text
//! commit <full-sha>
//! Author: <name> <email>
//! Date:   <date>
//!
//!     <subject>
//!
//!     <body>
//! ```

use git2::{Commit, Repository, Sort};

use super::GitResult;

#[derive(Default)]
struct Options {
    oneline: bool,
    limit: Option<usize>,
    author: Option<String>,
    format: Option<String>,
    since: Option<String>,
    follow: bool,
    paths: Vec<String>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut opts = Options::default();
    let mut i = 0;
    let mut after_dashdash = false;
    while i < args.len() {
        let a = &args[i];
        if after_dashdash {
            opts.paths.push(a.clone());
            i += 1;
            continue;
        }
        match a.as_str() {
            "--" => {
                after_dashdash = true;
            }
            "--oneline" => opts.oneline = true,
            "--follow" => opts.follow = true,
            "-n" => {
                i += 1;
                let v = args.get(i).ok_or("git log: -n requires an argument")?;
                opts.limit = Some(v.parse().map_err(|_| format!("git log: invalid -n '{v}'"))?);
            }
            s if s.starts_with("--author=") => {
                opts.author = Some(s["--author=".len()..].to_owned());
            }
            s if s.starts_with("--format=") => {
                opts.format = Some(s["--format=".len()..].to_owned());
            }
            s if s.starts_with("--pretty=") => {
                opts.format = Some(s["--pretty=".len()..].to_owned());
            }
            s if s.starts_with("--since=") || s.starts_with("--after=") => {
                let val = if s.starts_with("--since=") {
                    &s["--since=".len()..]
                } else {
                    &s["--after=".len()..]
                };
                opts.since = Some(val.to_owned());
            }
            // -<N> shorthand, e.g. -5
            s if s.starts_with('-')
                && s.len() > 1
                && s[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                opts.limit = Some(s[1..].parse().unwrap());
            }
            s if s.starts_with("-n") && s.len() > 2 => {
                let v = &s[2..];
                opts.limit = Some(v.parse().map_err(|_| format!("git log: invalid -n '{v}'"))?);
            }
            s if s.starts_with('-') => {
                return Err(format!("git log: unsupported flag '{s}'"));
            }
            _ => {
                // Bare argument after flags — treat as path
                opts.paths.push(a.clone());
            }
        }
        i += 1;
    }
    Ok(opts)
}

pub fn run(repo: &Repository, args: &[String]) -> GitResult {
    let opts = match parse(args) {
        Ok(o) => o,
        Err(e) => return GitResult::err(e, 128),
    };

    // Parse --since cutoff into a git timestamp (seconds since epoch).
    let since_cutoff: Option<i64> = opts.since.as_ref().and_then(|s| parse_since(s));

    let mut walker = match repo.revwalk() {
        Ok(w) => w,
        Err(e) => return GitResult::err(format!("git log: {e}"), 128),
    };
    if let Err(e) = walker.push_head() {
        return GitResult::err(format!("git log: {e}"), 128);
    }
    if let Err(e) = walker.set_sorting(Sort::TIME) {
        return GitResult::err(format!("git log: {e}"), 128);
    }

    let mut out = Vec::new();
    let mut emitted = 0usize;
    for oid in walker {
        if let Some(limit) = opts.limit
            && emitted >= limit
        {
            break;
        }
        let oid = match oid {
            Ok(o) => o,
            Err(e) => return GitResult::err(format!("git log: {e}"), 128),
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(e) => return GitResult::err(format!("git log: {e}"), 128),
        };

        // --since / --after: skip commits older than cutoff.
        if let Some(cutoff) = since_cutoff {
            if commit.time().seconds() < cutoff {
                // Commits are sorted by time; all subsequent are older.
                break;
            }
        }

        // --author filter
        if let Some(pat) = &opts.author {
            let name = commit.author().name().unwrap_or("").to_owned();
            if !name.contains(pat) {
                continue;
            }
        }

        // -- <path> filter: skip commits that don't touch any listed path.
        if !opts.paths.is_empty() && !commit_touches_paths(repo, &commit, &opts.paths) {
            continue;
        }

        if let Some(fmt) = &opts.format {
            render_format(&commit, fmt, &mut out);
        } else if opts.oneline {
            render_oneline(&commit, &mut out);
        } else {
            render_default(&commit, &mut out, emitted == 0);
        }
        emitted += 1;
    }

    GitResult::ok(out)
}

/// Parse a --since value into seconds since epoch.
/// Supports: ISO 8601 dates (YYYY-MM-DD), Unix timestamps.
fn parse_since(s: &str) -> Option<i64> {
    // Try Unix timestamp first.
    if let Ok(ts) = s.parse::<i64>() {
        return Some(ts);
    }
    // Try YYYY-MM-DD.
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() == 3 {
        let y: i64 = parts[0].parse().ok()?;
        let m: i64 = parts[1].parse().ok()?;
        let d: i64 = parts[2].parse().ok()?;
        // Approximate: convert to days since epoch, then to seconds.
        // Uses the same civil_from_days logic in reverse.
        let epoch_days = days_from_civil(y as i32, m as u32, d as u32);
        return Some(epoch_days * 86_400);
    }
    None
}

/// Inverse of civil_from_days: given (y, m, d) return days since 1970-01-01.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = y as i64;
    let m = m as i64;
    let d = d as i64;
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Check whether a commit touches any of the given paths.
fn commit_touches_paths(repo: &Repository, commit: &Commit<'_>, paths: &[String]) -> bool {
    let tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => return false,
    };

    // Get parent tree (or empty tree for the root commit).
    let parent_tree = commit
        .parent(0)
        .ok()
        .and_then(|p| p.tree().ok());

    let mut diff_opts = git2::DiffOptions::new();
    for path in paths {
        diff_opts.pathspec(path);
    }

    let diff = match repo.diff_tree_to_tree(
        parent_tree.as_ref(),
        Some(&tree),
        Some(&mut diff_opts),
    ) {
        Ok(d) => d,
        Err(_) => return false,
    };

    diff.deltas().count() > 0
}

fn render_oneline(commit: &Commit<'_>, out: &mut Vec<u8>) {
    let oid = commit.id().to_string();
    let summary = commit.summary().unwrap_or("");
    out.extend_from_slice(format!("{} {}\n", &oid[..7], summary).as_bytes());
}

fn render_default(commit: &Commit<'_>, out: &mut Vec<u8>, first: bool) {
    if !first {
        out.push(b'\n');
    }
    let author = commit.author();
    out.extend_from_slice(format!("commit {}\n", commit.id()).as_bytes());
    out.extend_from_slice(
        format!(
            "Author: {} <{}>\n",
            author.name().unwrap_or(""),
            author.email().unwrap_or("")
        )
        .as_bytes(),
    );
    out.extend_from_slice(format!("Date:   {}\n\n", format_time(&author.when())).as_bytes());
    let msg = commit.message().unwrap_or("");
    for line in msg.lines() {
        out.extend_from_slice(b"    ");
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
}

fn render_format(commit: &Commit<'_>, fmt: &str, out: &mut Vec<u8>) {
    let oid = commit.id().to_string();
    let author = commit.author();
    let mut buf = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            buf.push(c);
            continue;
        }
        match chars.next() {
            Some('H') => buf.push_str(&oid),
            Some('h') => buf.push_str(&oid[..7]),
            Some('s') => buf.push_str(commit.summary().unwrap_or("")),
            Some('a') => match chars.next() {
                Some('n') => buf.push_str(author.name().unwrap_or("")),
                Some('e') => buf.push_str(author.email().unwrap_or("")),
                Some('d') => buf.push_str(&format_time(&author.when())),
                Some(other) => {
                    buf.push('%');
                    buf.push('a');
                    buf.push(other);
                }
                None => {
                    buf.push('%');
                    buf.push('a');
                }
            },
            Some('n') => buf.push('\n'),
            Some(other) => {
                buf.push('%');
                buf.push(other);
            }
            None => buf.push('%'),
        }
    }
    out.extend_from_slice(buf.as_bytes());
    out.push(b'\n');
}

/// Format a git timestamp in the standard `git log` date format:
/// `Wed Apr 19 12:34:56 2026 +0000`.
pub(crate) fn format_time(t: &git2::Time) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let secs = t.seconds();
    let offset_min = t.offset_minutes();
    let secs_u = if secs < 0 { 0 } else { secs as u64 };
    let sys = UNIX_EPOCH + Duration::from_secs(secs_u);
    let (y, m, d, hh, mm, ss, dow) = ymd_hms_dow(sys);
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let sign = if offset_min >= 0 { '+' } else { '-' };
    let off = offset_min.abs();
    format!(
        "{} {} {} {:02}:{:02}:{:02} {} {}{:02}{:02}",
        days[dow],
        months[(m - 1) as usize],
        d,
        hh,
        mm,
        ss,
        y,
        sign,
        off / 60,
        off % 60
    )
}

/// Proleptic Gregorian breakdown of a `SystemTime` into `(y, m, d, hh, mm, ss, weekday)`.
/// Weekday is 0=Sunday … 6=Saturday.
fn ymd_hms_dow(st: std::time::SystemTime) -> (i32, u32, u32, u32, u32, u32, usize) {
    let dur = st.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let total = dur.as_secs() as i64;
    let days = total.div_euclid(86_400);
    let secs = total.rem_euclid(86_400);
    let hh = (secs / 3600) as u32;
    let mm = ((secs / 60) % 60) as u32;
    let ss = (secs % 60) as u32;
    let dow = ((days + 4).rem_euclid(7)) as usize; // 1970-01-01 was a Thursday
    let (y, m, d) = civil_from_days(days);
    (y, m, d, hh, mm, ss, dow)
}

/// Hinnant's civil-from-days algorithm. `z` is days since 1970-01-01.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (y, m, d)
}
