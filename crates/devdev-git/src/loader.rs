//! Virtual Git repository loaded from VFS.
//!
//! Uses the fallback strategy from the spec: writes `.git` from VFS to a temp
//! directory, then opens with `git2::Repository::open()`. The temp dir is owned
//! by `VirtualRepo` and cleaned up on drop.

use std::path::Path;

use devdev_vfs::MemFs;
use thiserror::Error;

/// A git repository backed by data loaded from the VFS.
pub struct VirtualRepo {
    repo: git2::Repository,
    _tempdir: tempfile::TempDir,
}

/// Errors that can occur while loading a git repo from VFS.
#[derive(Debug, Error)]
pub enum GitLoadError {
    #[error("no .git directory found at {0}")]
    NoGitDir(String),

    #[error("invalid .git directory: {0}")]
    InvalidGitDir(String),

    #[error("VFS error: {0}")]
    VfsError(#[from] devdev_vfs::VfsError),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("git error: {0}")]
    LibGitError(#[from] git2::Error),
}

impl VirtualRepo {
    /// Load a git repository from VFS.
    ///
    /// Reads `.git` from the VFS at the given repo root, writes it to a temp
    /// directory, and opens it with libgit2.
    pub fn from_vfs(vfs: &MemFs, repo_root: &str) -> Result<Self, GitLoadError> {
        let git_dir = if repo_root == "/" {
            "/.git".to_owned()
        } else {
            format!("{repo_root}/.git")
        };

        // Verify .git exists
        if !vfs.exists(Path::new(&git_dir)) {
            return Err(GitLoadError::NoGitDir(git_dir));
        }

        // Verify HEAD exists (basic validity check)
        let head_path = format!("{git_dir}/HEAD");
        if !vfs.exists(Path::new(&head_path)) {
            return Err(GitLoadError::InvalidGitDir(
                "missing HEAD file".into(),
            ));
        }

        // Create temp directory and write .git contents
        let tempdir = tempfile::TempDir::new()?;
        let temp_root = tempdir.path();
        let temp_git = temp_root.join(".git");

        write_vfs_dir_to_host(vfs, &git_dir, &temp_git)?;

        // Open repository
        let repo = git2::Repository::open(temp_root)?;

        Ok(Self {
            repo,
            _tempdir: tempdir,
        })
    }

    /// Get a reference to the underlying git2 Repository.
    pub fn repo(&self) -> &git2::Repository {
        &self.repo
    }

    /// Get the current HEAD ref name (e.g., "refs/heads/main").
    pub fn head_ref(&self) -> Result<String, GitLoadError> {
        let head = self.repo.head()?;
        Ok(head
            .name()
            .ok_or_else(|| GitLoadError::InvalidGitDir("HEAD is not a valid UTF-8 ref".into()))?
            .to_owned())
    }

    /// Get the current HEAD commit.
    pub fn head_commit(&self) -> Result<git2::Commit<'_>, GitLoadError> {
        let head = self.repo.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit)
    }
}

/// Recursively write a VFS directory to a host filesystem path.
fn write_vfs_dir_to_host(
    vfs: &MemFs,
    vfs_dir: &str,
    host_dir: &std::path::Path,
) -> Result<(), GitLoadError> {
    std::fs::create_dir_all(host_dir)?;

    let entries = vfs.list(Path::new(vfs_dir))?;
    for entry in entries {
        let vfs_path = format!("{vfs_dir}/{}", entry.name);
        let host_path = host_dir.join(&entry.name);

        match entry.file_type {
            devdev_vfs::FileType::Directory => {
                write_vfs_dir_to_host(vfs, &vfs_path, &host_path)?;
            }
            devdev_vfs::FileType::File => {
                let content = vfs.read(Path::new(&vfs_path))?;
                std::fs::write(&host_path, content)?;
            }
            devdev_vfs::FileType::Symlink => {
                let target = vfs.readlink(Path::new(&vfs_path))?;
                // Best-effort symlink; skip if unsupported
                #[cfg(unix)]
                {
                    let _ = std::os::unix::fs::symlink(&target, &host_path);
                }
                #[cfg(windows)]
                {
                    let _ = std::os::windows::fs::symlink_file(
                        &target,
                        &host_path,
                    );
                }
            }
        }
    }

    Ok(())
}
