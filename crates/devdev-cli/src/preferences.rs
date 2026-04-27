//! `.devdev/` preference file discovery + loader.
//!
//! Walks from `start_dir` upward looking for `.devdev/*.md` files,
//! then falls back to `~/.devdev/*.md`. Repo-local files always win
//! when titles collide (later layers shadow earlier ones).
//!
//! Used by:
//! - `MonitorPrTask` (Phase B2) — preference *paths* are injected into
//!   the per-PR session prompt so the agent reads them on demand.
//! - The forthcoming `devdev_preferences_list` MCP tool — surfaces
//!   the loaded files so the agent can decide which to read.

use std::path::{Path, PathBuf};

/// One discovered preference file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferenceFile {
    /// Absolute path to the `.md` file.
    pub path: PathBuf,
    /// First-line `# Title` if present, else the file stem.
    pub title: String,
    /// Full UTF-8 body (lossy on bad bytes — preference files are
    /// human-written prose; we never refuse to load).
    pub body: String,
    /// Discovery layer (`Repo` < `Parent` < `Home`). Earlier layers
    /// win; the loader returns files in priority order.
    pub layer: PreferenceLayer,
}

/// Where a preference file came from. Kept as an enum (not a path
/// depth) so we can adjust precedence without re-walking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceLayer {
    /// `<start>/.devdev/`. Highest priority.
    Repo,
    /// Any ancestor's `.devdev/` between repo and home.
    Parent,
    /// `~/.devdev/`. Lowest priority.
    Home,
}

/// Errors the loader surfaces. We deliberately return `Ok(vec![])`
/// rather than erroring on a missing `.devdev/` directory; the only
/// failure modes are filesystem I/O on directories that *do* exist.
#[derive(thiserror::Error, Debug)]
pub enum PreferenceError {
    #[error("read_dir {path}: {source}")]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read_file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Walk from `start_dir` up to (but not including) `home_dir` and
/// collect every `.devdev/*.md` file in priority order. Adds
/// `~/.devdev/*.md` last.
///
/// `home_dir = None` skips the home layer (useful for tests).
pub fn discover(
    start_dir: &Path,
    home_dir: Option<&Path>,
) -> Result<Vec<PreferenceFile>, PreferenceError> {
    let mut out = Vec::new();
    let mut seen_titles: std::collections::HashSet<String> = Default::default();

    let mut layer = PreferenceLayer::Repo;
    let mut cursor = Some(start_dir.to_path_buf());
    while let Some(dir) = cursor {
        let prefs_dir = dir.join(".devdev");
        if prefs_dir.is_dir() {
            for f in load_dir(&prefs_dir, layer)? {
                if seen_titles.insert(f.title.clone()) {
                    out.push(f);
                }
            }
        }
        // After the start dir's own `.devdev/`, every ancestor we
        // touch is a parent.
        layer = PreferenceLayer::Parent;
        cursor = dir.parent().map(Path::to_path_buf);
        // Stop the moment we'd cross into the home directory; we'll
        // load it explicitly below so it lands in the `Home` layer.
        if let (Some(home), Some(next)) = (home_dir, cursor.as_deref())
            && next == home
        {
            break;
        }
    }

    if let Some(home) = home_dir {
        let prefs_dir = home.join(".devdev");
        if prefs_dir.is_dir() {
            for f in load_dir(&prefs_dir, PreferenceLayer::Home)? {
                if seen_titles.insert(f.title.clone()) {
                    out.push(f);
                }
            }
        }
    }

    Ok(out)
}

fn load_dir(dir: &Path, layer: PreferenceLayer) -> Result<Vec<PreferenceFile>, PreferenceError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| PreferenceError::ReadDir {
            path: dir.to_path_buf(),
            source,
        })?
        .filter_map(|r| r.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    // Stable order so the agent sees the same files in the same order
    // every run. Lexicographic on the file name is fine.
    entries.sort();

    let mut out = Vec::with_capacity(entries.len());
    for path in entries {
        let body = std::fs::read_to_string(&path).map_err(|source| PreferenceError::ReadFile {
            path: path.clone(),
            source,
        })?;
        let title = extract_title(&body).unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("preference")
                .to_string()
        });
        out.push(PreferenceFile {
            path,
            title,
            body,
            layer,
        });
    }
    Ok(out)
}

/// Returns the text after the first leading `# ` line, or `None`.
fn extract_title(body: &str) -> Option<String> {
    body.lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l[2..].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(path: &Path, contents: &str) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn missing_dir_is_not_error() {
        let tmp = tempdir().unwrap();
        let out = discover(tmp.path(), None).expect("ok");
        assert!(out.is_empty());
    }

    #[test]
    fn extracts_title_from_h1() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join(".devdev").join("style.md"),
            "# Code Style\n\nUse Rustfmt.\n",
        );
        let files = discover(tmp.path(), None).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].title, "Code Style");
        assert_eq!(files[0].layer, PreferenceLayer::Repo);
    }

    #[test]
    fn falls_back_to_stem_when_no_h1() {
        let tmp = tempdir().unwrap();
        write(
            &tmp.path().join(".devdev").join("vibes.md"),
            "Just prose.\n",
        );
        let files = discover(tmp.path(), None).unwrap();
        assert_eq!(files[0].title, "vibes");
    }

    #[test]
    fn home_layer_loaded_with_lower_priority() {
        let tmp = tempdir().unwrap();
        let home = tempdir().unwrap();
        write(
            &tmp.path().join(".devdev").join("a.md"),
            "# Shared\nrepo wins\n",
        );
        write(
            &home.path().join(".devdev").join("b.md"),
            "# Home Only\nhome\n",
        );
        // Title collision: both define "Shared"; repo must win.
        write(
            &home.path().join(".devdev").join("a.md"),
            "# Shared\nhome version\n",
        );

        let files = discover(tmp.path(), Some(home.path())).unwrap();
        // Two unique titles; repo's "Shared" first, home's "Home Only" second.
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].title, "Shared");
        assert_eq!(files[0].layer, PreferenceLayer::Repo);
        assert!(files[0].body.contains("repo wins"));
        assert_eq!(files[1].title, "Home Only");
        assert_eq!(files[1].layer, PreferenceLayer::Home);
    }

    #[test]
    fn parent_layer_loaded_between_repo_and_home() {
        let root = tempdir().unwrap();
        let parent = root.path().join("workspaces");
        let repo = parent.join("project");
        fs::create_dir_all(&repo).unwrap();
        write(&repo.join(".devdev").join("a.md"), "# Repo Pref\nlocal\n");
        write(
            &parent.join(".devdev").join("b.md"),
            "# Parent Pref\nshared\n",
        );

        let files = discover(&repo, None).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].layer, PreferenceLayer::Repo);
        assert_eq!(files[1].layer, PreferenceLayer::Parent);
        assert_eq!(files[1].title, "Parent Pref");
    }

    #[test]
    fn non_md_files_are_ignored() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join(".devdev").join("note.txt"), "no");
        write(&tmp.path().join(".devdev").join("a.md"), "# A\n");
        let files = discover(tmp.path(), None).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].title, "A");
    }

    #[test]
    fn sorted_lexicographically_within_layer() {
        let tmp = tempdir().unwrap();
        write(&tmp.path().join(".devdev").join("z.md"), "# Z\n");
        write(&tmp.path().join(".devdev").join("a.md"), "# A\n");
        write(&tmp.path().join(".devdev").join("m.md"), "# M\n");
        let files = discover(tmp.path(), None).unwrap();
        let titles: Vec<_> = files.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "M", "Z"]);
    }
}
