//! Host-to-VFS repository loader.
//!
//! Two-pass loading: first walks the host tree to calculate total size,
//! then copies files/directories/symlinks into the VFS. Fails fast if
//! total size exceeds the VFS capacity limit.

use std::path::{Path, PathBuf};

use crate::{MemFs, VfsError};

/// Hard cap on host-directory recursion depth. Prevents a pathological
/// tree (or a symlink loop that slips past the symlink filter) from
/// blowing the stack or running the walker forever. 64 is deeper than
/// any real-world repo layout.
pub const MAX_DEPTH: usize = 64;

/// Options controlling how a repository is loaded.
pub struct LoadOptions {
    /// Whether to include the `.git` directory (default: true).
    pub include_git: bool,
    /// Progress callback, called for each file loaded.
    pub progress: Option<Box<dyn Fn(LoadProgress)>>,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            include_git: true,
            progress: None,
        }
    }
}

/// Progress information emitted during loading.
#[derive(Debug, Clone)]
pub struct LoadProgress {
    pub files_loaded: u64,
    pub bytes_loaded: u64,
    pub current_path: String,
}

/// Errors that can occur while loading a repository.
#[derive(Debug)]
pub enum LoadError {
    HostPathNotFound(PathBuf),
    NotADirectory(PathBuf),
    ExceedsLimit { total_bytes: u64, limit: u64 },
    /// Host tree is deeper than [`MAX_DEPTH`]. Usually a symlink loop
    /// that evaded the symlink filter, or an adversarial input.
    ExceedsDepth { depth: usize, limit: usize },
    IoError(std::io::Error),
    VfsError(VfsError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostPathNotFound(p) => write!(f, "host path not found: {}", p.display()),
            Self::NotADirectory(p) => write!(f, "not a directory: {}", p.display()),
            Self::ExceedsLimit { total_bytes, limit } => {
                write!(f, "repo size {total_bytes} bytes exceeds VFS limit {limit} bytes")
            }
            Self::ExceedsDepth { depth, limit } => {
                write!(f, "host tree depth {depth} exceeds limit {limit}")
            }
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::VfsError(e) => write!(f, "VFS error: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

impl From<VfsError> for LoadError {
    fn from(e: VfsError) -> Self {
        Self::VfsError(e)
    }
}

/// Load a host filesystem directory into the VFS.
///
/// Two-pass: first calculates total size, then loads if within capacity.
/// Files are placed under `prefix` (use `/` for the traditional root mount).
/// Returns the total bytes loaded.
pub fn load_repo(
    host_path: &Path,
    vfs: &mut MemFs,
    options: &LoadOptions,
) -> Result<u64, LoadError> {
    load_repo_at(host_path, vfs, Path::new("/"), options)
}

/// Load a host filesystem directory into the VFS under a given `prefix`.
///
/// Two-pass: first calculates total size, then loads if within capacity.
/// Returns the total bytes loaded.
pub fn load_repo_at(
    host_path: &Path,
    vfs: &mut MemFs,
    prefix: &Path,
    options: &LoadOptions,
) -> Result<u64, LoadError> {
    // Validate host path
    if !host_path.exists() {
        return Err(LoadError::HostPathNotFound(host_path.to_owned()));
    }
    if !host_path.is_dir() {
        return Err(LoadError::NotADirectory(host_path.to_owned()));
    }

    // Pass 1: calculate total size
    let total_bytes = calculate_total_size(host_path, options)?;
    let limit = vfs.usage().bytes_limit;
    let already_used = vfs.usage().bytes_used;
    if total_bytes + already_used > limit {
        return Err(LoadError::ExceedsLimit {
            total_bytes,
            limit,
        });
    }

    // Pass 2: load files
    let mut files_loaded: u64 = 0;
    let mut bytes_loaded: u64 = 0;
    let prefix_str = prefix.to_string_lossy().to_string();
    load_dir_recursive(
        host_path,
        host_path,
        vfs,
        &prefix_str,
        options,
        &mut files_loaded,
        &mut bytes_loaded,
        0,
    )?;

    Ok(bytes_loaded)
}

/// Walk the host tree and sum all file sizes.
fn calculate_total_size(root: &Path, options: &LoadOptions) -> Result<u64, LoadError> {
    let mut total: u64 = 0;
    for entry in walk_dir(root, options)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_file() {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

/// Recursively load a host directory into the VFS.
fn load_dir_recursive(
    root: &Path,
    current: &Path,
    vfs: &mut MemFs,
    prefix: &str,
    options: &LoadOptions,
    files_loaded: &mut u64,
    bytes_loaded: &mut u64,
    depth: usize,
) -> Result<(), LoadError> {
    if depth > MAX_DEPTH {
        return Err(LoadError::ExceedsDepth {
            depth,
            limit: MAX_DEPTH,
        });
    }
    let read_dir = std::fs::read_dir(current)?;

    for entry in read_dir {
        let entry = entry?;
        let host_path = entry.path();
        let file_type = entry.file_type()?;

        // Skip .git if not requested
        if !options.include_git && entry.file_name() == ".git" {
            continue;
        }

        // Compute VFS path: strip the root prefix and prepend the target prefix
        let relative = host_path.strip_prefix(root).unwrap_or(&host_path);
        let rel_str = relative.to_string_lossy().replace('\\', "/");
        let vfs_path_str = if prefix == "/" {
            format!("/{rel_str}")
        } else {
            format!("{prefix}/{rel_str}")
        };
        let vfs_path = Path::new(&vfs_path_str);

        if file_type.is_symlink() {
            let target = std::fs::read_link(&host_path)?;
            let target_str = target.to_string_lossy().replace('\\', "/");
            vfs.symlink(Path::new(&target_str), vfs_path)?;
        } else if file_type.is_dir() {
            vfs.mkdir_p(vfs_path)?;
            load_dir_recursive(
                root,
                &host_path,
                vfs,
                prefix,
                options,
                files_loaded,
                bytes_loaded,
                depth + 1,
            )?;
        } else if file_type.is_file() {
            let content = std::fs::read(&host_path)?;
            let size = content.len() as u64;

            // Ensure parent dir exists
            if let Some(parent) = vfs_path.parent() {
                let parent_str = parent.to_string_lossy();
                if parent_str != "/" {
                    vfs.mkdir_p(parent)?;
                }
            }

            vfs.write(vfs_path, &content)?;

            // Preserve permissions (Unix)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = entry.metadata()?.permissions().mode();
                let _ = vfs.chmod(vfs_path, mode);
            }

            *files_loaded += 1;
            *bytes_loaded += size;

            if let Some(ref cb) = options.progress {
                cb(LoadProgress {
                    files_loaded: *files_loaded,
                    bytes_loaded: *bytes_loaded,
                    current_path: vfs_path_str,
                });
            }
        }
    }

    Ok(())
}

/// Walk a host directory recursively, yielding DirEntry items.
fn walk_dir(
    root: &Path,
    options: &LoadOptions,
) -> Result<Vec<Result<std::fs::DirEntry, std::io::Error>>, LoadError> {
    let mut results = Vec::new();
    walk_dir_inner(root, options, &mut results, 0)?;
    Ok(results)
}

fn walk_dir_inner(
    current: &Path,
    options: &LoadOptions,
    results: &mut Vec<Result<std::fs::DirEntry, std::io::Error>>,
    depth: usize,
) -> Result<(), LoadError> {
    if depth > MAX_DEPTH {
        return Err(LoadError::ExceedsDepth {
            depth,
            limit: MAX_DEPTH,
        });
    }
    for entry in std::fs::read_dir(current)? {
        match entry {
            Ok(e) => {
                if !options.include_git && e.file_name() == ".git" {
                    continue;
                }
                let ft = e.file_type();
                let name = e.file_name();
                // Skip symlinks during the host walk entirely — they
                // are replicated as VFS symlinks by load_dir_recursive
                // and must never be *followed* here, or a loop would
                // hang the walker and inflate calculate_total_size.
                let is_symlink = ft.as_ref().map(|t| t.is_symlink()).unwrap_or(false);
                if is_symlink {
                    results.push(Ok(e));
                    continue;
                }
                results.push(Ok(e));
                if let Ok(ft) = ft
                    && ft.is_dir()
                {
                    let path = current.join(name);
                    walk_dir_inner(&path, options, results, depth + 1)?;
                }
            }
            Err(err) => {
                results.push(Err(err));
            }
        }
    }
    Ok(())
}
