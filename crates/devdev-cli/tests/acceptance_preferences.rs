//! Integration tests for `.devdev/` preference loader.

use std::fs;
use std::path::Path;

use devdev_cli::preferences::{PreferenceLayer, discover};

fn write(path: &Path, contents: &str) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn discovers_repo_only_when_no_home() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        &tmp.path().join(".devdev").join("style.md"),
        "# Style\nbe terse",
    );
    let files = discover(tmp.path(), None).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].layer, PreferenceLayer::Repo);
    assert_eq!(files[0].title, "Style");
    assert!(files[0].body.contains("be terse"));
}

#[test]
fn precedence_repo_over_parent_over_home() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let parent = root.path().join("ws");
    let repo = parent.join("project");
    fs::create_dir_all(&repo).unwrap();
    // Same title in three layers; repo wins.
    write(&repo.join(".devdev").join("a.md"), "# Vibes\nrepo wins\n");
    write(
        &parent.join(".devdev").join("a.md"),
        "# Vibes\nparent loses\n",
    );
    write(
        &home.path().join(".devdev").join("a.md"),
        "# Vibes\nhome loses\n",
    );

    let files = discover(&repo, Some(home.path())).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].layer, PreferenceLayer::Repo);
    assert!(files[0].body.contains("repo wins"));
}

#[test]
fn missing_devdev_dir_yields_empty_vec_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    let files = discover(tmp.path(), None).unwrap();
    assert!(files.is_empty());
}

#[test]
fn home_only_files_loaded_when_repo_has_none() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    write(
        &home.path().join(".devdev").join("global.md"),
        "# Global\nshared rules\n",
    );
    let files = discover(tmp.path(), Some(home.path())).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].layer, PreferenceLayer::Home);
}
