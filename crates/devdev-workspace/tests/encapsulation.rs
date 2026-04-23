//! Encapsulation guard: forbid direct host-fs access from paths
//! that could reach agent-supplied strings.
//!
//! The rule (enforced here): source files in this crate must not use
//! `std::fs::`, `tokio::fs::`, `std::os::unix::fs`, or
//! `std::os::windows::fs`. The only exceptions are files where we
//! provably only touch mount-point paths we own.

use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    "std::fs::",
    "tokio::fs::",
    "std::os::unix::fs",
    "std::os::windows::fs",
];

/// Files permitted to contain forbidden substrings. (None currently.)
const ALLOWED: &[&str] = &[];

fn crate_src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_forbidden_host_fs_calls() {
    let src = crate_src_dir();
    let mut files = Vec::new();
    walk(&src, &mut files);
    assert!(!files.is_empty(), "no .rs files found under {:?}", src);

    let mut offenses: Vec<String> = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(&src).unwrap_or(f);
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if ALLOWED.iter().any(|a| rel_str == *a) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            // Skip comments cheaply: trim left, check `//`.
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for needle in FORBIDDEN {
                if line.contains(needle) {
                    offenses.push(format!("{}:{}: {}", rel_str, i + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        offenses.is_empty(),
        "forbidden host-fs calls found:\n{}",
        offenses.join("\n")
    );
}
