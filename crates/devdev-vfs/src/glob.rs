use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};

use crate::types::Node;

/// Expand a glob pattern against the VFS tree.
///
/// The pattern is relative to `base` (typically the cwd).
/// Returns a sorted list of matching absolute paths.
pub fn expand(
    pattern: &str,
    base: &Path,
    tree: &BTreeMap<PathBuf, Node>,
) -> Result<Vec<PathBuf>, crate::VfsError> {
    let full_pattern = if pattern.starts_with('/') {
        pattern.to_owned()
    } else {
        let base_str = base.to_str().unwrap_or("/");
        if base_str == "/" {
            format!("/{pattern}")
        } else {
            format!("{base_str}/{pattern}")
        }
    };

    let matcher = build_matcher(&full_pattern)?;

    let mut results: Vec<PathBuf> = tree
        .keys()
        .filter(|path| {
            let path_str = path.to_str().unwrap_or("");
            // Don't match the root itself
            path_str != "/" && matcher.is_match(path_str)
        })
        .cloned()
        .collect();

    results.sort();
    Ok(results)
}

fn build_matcher(pattern: &str) -> Result<GlobMatcher, crate::VfsError> {
    let glob = GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map_err(|e| crate::VfsError::InvalidGlob(e.to_string()))?;
    Ok(glob.compile_matcher())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    /// Build a BTreeMap tree from a list of path strings.
    /// Uses forward-slash string formatting (not PathBuf::push) to avoid
    /// Windows backslash contamination in tree keys.
    fn make_tree(paths: &[&str]) -> BTreeMap<PathBuf, Node> {
        let mut tree = BTreeMap::new();
        tree.insert(
            PathBuf::from("/"),
            Node::Directory {
                mode: 0o755,
                modified: SystemTime::now(),
            },
        );
        for p in paths {
            // Ensure parent directories exist using `/`-joined paths
            let parts: Vec<&str> = p.trim_start_matches('/').split('/').collect();
            for i in 1..parts.len() {
                let ancestor = format!("/{}", parts[..i].join("/"));
                tree.entry(PathBuf::from(&ancestor)).or_insert(Node::Directory {
                    mode: 0o755,
                    modified: SystemTime::now(),
                });
            }
            let path = PathBuf::from(p);
            if p.ends_with('/') {
                tree.insert(
                    path,
                    Node::Directory {
                        mode: 0o755,
                        modified: SystemTime::now(),
                    },
                );
            } else {
                tree.insert(
                    path,
                    Node::File {
                        content: Vec::new(),
                        mode: 0o644,
                        modified: SystemTime::now(),
                    },
                );
            }
        }
        tree
    }

    /// Verify `*.rs` in a subdirectory matches only .rs files in that dir,
    /// not .txt files or files in other directories.
    #[test]
    fn glob_star_rs() {
        let tree = make_tree(&["/src/main.rs", "/src/lib.rs", "/src/util.txt", "/README.md"]);
        let results = expand("*.rs", Path::new("/src"), &tree).unwrap();
        assert_eq!(
            results,
            vec![PathBuf::from("/src/lib.rs"), PathBuf::from("/src/main.rs")]
        );
    }

    /// Verify `**/*.rs` recursively matches .rs files at all depths,
    /// including nested subdirectories and sibling directories.
    #[test]
    fn glob_double_star() {
        let tree = make_tree(&[
            "/src/main.rs",
            "/src/lib.rs",
            "/src/sub/mod.rs",
            "/tests/test.rs",
        ]);
        let results = expand("**/*.rs", Path::new("/"), &tree).unwrap();
        assert_eq!(
            results,
            vec![
                PathBuf::from("/src/lib.rs"),
                PathBuf::from("/src/main.rs"),
                PathBuf::from("/src/sub/mod.rs"),
                PathBuf::from("/tests/test.rs"),
            ]
        );
    }

    /// Verify `?` wildcard matches exactly one character — both digits
    /// and letters.
    #[test]
    fn glob_question_mark() {
        let tree = make_tree(&["/a1.txt", "/a2.txt", "/ab.txt"]);
        let results = expand("a?.txt", Path::new("/"), &tree).unwrap();
        assert_eq!(
            results,
            vec![
                PathBuf::from("/a1.txt"),
                PathBuf::from("/a2.txt"),
                PathBuf::from("/ab.txt"),
            ]
        );
    }

    /// Verify that a malformed glob pattern (unclosed bracket) returns
    /// an error rather than panicking or matching nothing silently.
    #[test]
    fn glob_invalid_pattern() {
        let tree = make_tree(&[]);
        let result = expand("[invalid", Path::new("/"), &tree);
        assert!(result.is_err());
    }
}
