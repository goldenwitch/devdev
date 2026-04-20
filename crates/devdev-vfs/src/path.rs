use std::path::{Component, Path, PathBuf};

/// Normalize a virtual path to an absolute canonical form.
///
/// - Resolves `.` and `..` components
/// - Strips redundant separators
/// - Always returns a path rooted at `/`
/// - `..` at root stays at root (cannot escape)
pub fn normalize(path: &Path) -> PathBuf {
    let mut components: Vec<String> = Vec::new();

    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => {
                components.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop();
            }
            Component::Normal(s) => {
                if let Some(s) = s.to_str() {
                    components.push(s.to_owned());
                }
            }
        }
    }

    if components.is_empty() {
        PathBuf::from("/")
    } else {
        PathBuf::from(format!("/{}", components.join("/")))
    }
}

/// Resolve a potentially relative `path` against the given `base` (cwd),
/// then normalize the result.
pub fn resolve(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() || path.to_str().is_some_and(|s| s.starts_with('/')) {
        normalize(path)
    } else {
        normalize(&base.join(path))
    }
}

/// Return the parent directory of a normalized path.
/// Returns `/` for the root itself.
pub fn parent(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if p.as_os_str().is_empty() => PathBuf::from("/"),
        Some(p) => p.to_path_buf(),
        None => PathBuf::from("/"),
    }
}

/// Extract the final component (file/dir name) from a path.
pub fn file_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify a clean absolute path passes through normalization unchanged.
    #[test]
    fn normalize_absolute() {
        assert_eq!(normalize(Path::new("/a/b/c")), PathBuf::from("/a/b/c"));
    }

    /// Verify `.` (current dir) and `..` (parent dir) are resolved correctly
    /// in the middle of a path.
    #[test]
    fn normalize_dot_and_dotdot() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    /// Verify `..` at the root cannot escape — it clamps to `/`.
    #[test]
    fn normalize_dotdot_at_root() {
        assert_eq!(normalize(Path::new("/../../a")), PathBuf::from("/a"));
    }

    /// Verify the root path normalizes to itself.
    #[test]
    fn normalize_root() {
        assert_eq!(normalize(Path::new("/")), PathBuf::from("/"));
    }

    /// Verify a relative path is joined to the base (cwd) before normalizing.
    #[test]
    fn resolve_relative() {
        assert_eq!(
            resolve(Path::new("foo/bar"), Path::new("/home")),
            PathBuf::from("/home/foo/bar")
        );
    }

    /// Verify an absolute path ignores the base entirely.
    #[test]
    fn resolve_absolute_ignores_base() {
        assert_eq!(
            resolve(Path::new("/etc/config"), Path::new("/home")),
            PathBuf::from("/etc/config")
        );
    }

    /// Verify parent of root is root (no escape).
    #[test]
    fn parent_of_root() {
        assert_eq!(parent(Path::new("/")), PathBuf::from("/"));
    }

    /// Verify parent of a nested path strips the last component.
    #[test]
    fn parent_of_nested() {
        assert_eq!(parent(Path::new("/a/b/c")), PathBuf::from("/a/b"));
    }
}
