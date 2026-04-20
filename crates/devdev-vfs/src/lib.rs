//! In-memory virtual filesystem for DevDev sandbox.
//!
//! Provides a tree-based in-memory filesystem with POSIX-like operations,
//! glob expansion, symlink support, and configurable memory caps.

use std::path::PathBuf;

use thiserror::Error;

pub mod glob;
pub mod loader;
pub mod memfs;
pub mod path;
pub mod types;

pub use loader::{LoadError, LoadOptions, LoadProgress, MAX_DEPTH, load_repo};
pub use memfs::MemFs;
pub use types::{DirEntry, FileStat, FileType, MemoryUsage};

/// VFS error type.
#[derive(Debug, Error)]
pub enum VfsError {
    #[error("not found: {0}")]
    NotFound(PathBuf),

    #[error("already exists: {0}")]
    AlreadyExists(PathBuf),

    #[error("is a directory: {0}")]
    IsADirectory(PathBuf),

    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("not a symlink: {0}")]
    NotASymlink(PathBuf),

    #[error("directory not empty: {0}")]
    DirectoryNotEmpty(PathBuf),

    #[error("symlink loop detected at: {0}")]
    SymlinkLoop(PathBuf),

    #[error("capacity exceeded: requested {requested} bytes, used {used}/{limit}")]
    CapacityExceeded {
        requested: u64,
        used: u64,
        limit: u64,
    },

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("invalid glob pattern: {0}")]
    InvalidGlob(String),
}

pub type VfsResult<T> = Result<T, VfsError>;

