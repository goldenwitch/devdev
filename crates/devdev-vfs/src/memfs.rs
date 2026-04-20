use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::glob;
use crate::path as vpath;
use crate::types::*;
use crate::{VfsError, VfsResult};

const DEFAULT_LIMIT: u64 = 2 * 1024 * 1024 * 1024; // 2 GB
const DEFAULT_FILE_MODE: u32 = 0o644;
const DEFAULT_DIR_MODE: u32 = 0o755;
const MAX_SYMLINK_DEPTH: usize = 40;

/// In-memory virtual filesystem backed by a `BTreeMap`.
pub struct MemFs {
    tree: BTreeMap<PathBuf, Node>,
    cwd: PathBuf,
    bytes_used: u64,
    bytes_limit: u64,
}

impl MemFs {
    /// Create a new empty VFS with the default 2 GB limit.
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_LIMIT)
    }

    /// Create a new empty VFS with a custom memory limit.
    pub fn with_limit(bytes_limit: u64) -> Self {
        let mut tree = BTreeMap::new();
        tree.insert(
            PathBuf::from("/"),
            Node::Directory {
                mode: DEFAULT_DIR_MODE,
                modified: SystemTime::now(),
            },
        );
        MemFs {
            tree,
            cwd: PathBuf::from("/"),
            bytes_used: 0,
            bytes_limit,
        }
    }

    // ── helpers ──────────────────────────────────────────────────

    /// Resolve a user-supplied path against `cwd` and normalize it.
    fn abs(&self, path: &Path) -> PathBuf {
        vpath::resolve(path, &self.cwd)
    }

    /// Follow symlinks to find the final concrete path.
    /// Returns the resolved path and the node's file type.
    fn resolve_symlinks(&self, path: &Path) -> VfsResult<PathBuf> {
        let mut current = self.abs(path);
        let mut seen = 0;

        loop {
            match self.tree.get(&current) {
                Some(Node::Symlink { target, .. }) => {
                    seen += 1;
                    if seen > MAX_SYMLINK_DEPTH {
                        return Err(VfsError::SymlinkLoop(current));
                    }
                    // Resolve symlink target relative to the symlink's parent dir
                    let parent = vpath::parent(&current);
                    current = vpath::resolve(target, &parent);
                }
                Some(_) => return Ok(current),
                None => return Err(VfsError::NotFound(current)),
            }
        }
    }

    /// Ensure the parent directory of `path` exists.
    fn ensure_parent(&self, path: &Path) -> VfsResult<()> {
        let parent = vpath::parent(path);
        if parent.as_os_str() == "/" {
            return Ok(());
        }
        let resolved = self.resolve_symlinks(&parent)?;
        match self.tree.get(&resolved) {
            Some(Node::Directory { .. }) => Ok(()),
            Some(_) => Err(VfsError::NotADirectory(resolved)),
            None => Err(VfsError::NotFound(resolved)),
        }
    }

    /// Check if adding `additional` bytes would exceed the cap.
    fn check_capacity(&self, additional: u64) -> VfsResult<()> {
        if self.bytes_used + additional > self.bytes_limit {
            Err(VfsError::CapacityExceeded {
                requested: additional,
                used: self.bytes_used,
                limit: self.bytes_limit,
            })
        } else {
            Ok(())
        }
    }

    // ── File I/O ────────────────────────────────────────────────

    pub fn read(&self, path: &Path) -> VfsResult<Vec<u8>> {
        let resolved = self.resolve_symlinks(path)?;
        match self.tree.get(&resolved) {
            Some(Node::File { content, .. }) => Ok(content.clone()),
            Some(Node::Directory { .. }) => Err(VfsError::IsADirectory(resolved)),
            Some(Node::Symlink { .. }) => unreachable!("symlinks resolved above"),
            None => Err(VfsError::NotFound(resolved)),
        }
    }

    pub fn write(&mut self, path: &Path, data: &[u8]) -> VfsResult<()> {
        let abs = self.abs(path);

        // Resolve through symlinks if the path already exists as a symlink
        let resolved = match self.resolve_symlinks(path) {
            Ok(p) => p,
            Err(VfsError::NotFound(_)) => abs.clone(),
            Err(e) => return Err(e),
        };

        self.ensure_parent(&resolved)?;

        let new_size = data.len() as u64;

        // Account for existing file size
        let old_size = match self.tree.get(&resolved) {
            Some(Node::File { content, .. }) => content.len() as u64,
            Some(Node::Directory { .. }) => return Err(VfsError::IsADirectory(resolved)),
            _ => 0,
        };

        if new_size > old_size {
            self.check_capacity(new_size - old_size)?;
        }
        self.bytes_used = self.bytes_used - old_size + new_size;

        self.tree.insert(
            resolved,
            Node::File {
                content: data.to_vec(),
                mode: DEFAULT_FILE_MODE,
                modified: SystemTime::now(),
            },
        );
        Ok(())
    }

    pub fn append(&mut self, path: &Path, data: &[u8]) -> VfsResult<()> {
        let resolved = self.resolve_symlinks(path)?;
        let additional = data.len() as u64;
        self.check_capacity(additional)?;

        match self.tree.get_mut(&resolved) {
            Some(Node::File {
                content, modified, ..
            }) => {
                content.extend_from_slice(data);
                *modified = SystemTime::now();
                self.bytes_used += additional;
                Ok(())
            }
            Some(Node::Directory { .. }) => Err(VfsError::IsADirectory(resolved)),
            _ => Err(VfsError::NotFound(resolved)),
        }
    }

    pub fn truncate(&mut self, path: &Path, size: u64) -> VfsResult<()> {
        let resolved = self.resolve_symlinks(path)?;

        // Read current size first to compute capacity check
        let old_size = match self.tree.get(&resolved) {
            Some(Node::File { content, .. }) => content.len() as u64,
            Some(Node::Directory { .. }) => return Err(VfsError::IsADirectory(resolved)),
            _ => return Err(VfsError::NotFound(resolved)),
        };

        if size > old_size {
            let growth = size - old_size;
            self.check_capacity(growth)?;
        }

        match self.tree.get_mut(&resolved) {
            Some(Node::File {
                content, modified, ..
            }) => {
                if size > old_size {
                    content.resize(size as usize, 0);
                    self.bytes_used += size - old_size;
                } else {
                    let shrink = old_size - size;
                    content.truncate(size as usize);
                    self.bytes_used -= shrink;
                }
                *modified = SystemTime::now();
                Ok(())
            }
            _ => unreachable!("already checked above"),
        }
    }

    // ── Metadata ────────────────────────────────────────────────

    /// Stat a path. Does NOT follow symlinks — returns the symlink itself.
    pub fn lstat(&self, path: &Path) -> VfsResult<FileStat> {
        let abs = self.abs(path);
        match self.tree.get(&abs) {
            Some(node) => Ok(node.stat()),
            None => Err(VfsError::NotFound(abs)),
        }
    }

    /// Stat a path, following symlinks.
    pub fn stat(&self, path: &Path) -> VfsResult<FileStat> {
        let resolved = self.resolve_symlinks(path)?;
        match self.tree.get(&resolved) {
            Some(node) => Ok(node.stat()),
            None => Err(VfsError::NotFound(resolved)),
        }
    }

    pub fn exists(&self, path: &Path) -> bool {
        self.resolve_symlinks(path).is_ok()
    }

    pub fn chmod(&mut self, path: &Path, mode: u32) -> VfsResult<()> {
        let resolved = self.resolve_symlinks(path)?;
        match self.tree.get_mut(&resolved) {
            Some(Node::File {
                mode: m, modified, ..
            })
            | Some(Node::Directory {
                mode: m, modified, ..
            }) => {
                *m = mode;
                *modified = SystemTime::now();
                Ok(())
            }
            Some(Node::Symlink { .. }) => Ok(()), // chmod on symlinks is a no-op
            None => Err(VfsError::NotFound(resolved)),
        }
    }

    // ── Directories ─────────────────────────────────────────────

    pub fn mkdir(&mut self, path: &Path) -> VfsResult<()> {
        let abs = self.abs(path);
        self.ensure_parent(&abs)?;

        if self.tree.contains_key(&abs) {
            return Err(VfsError::AlreadyExists(abs));
        }

        self.tree.insert(
            abs,
            Node::Directory {
                mode: DEFAULT_DIR_MODE,
                modified: SystemTime::now(),
            },
        );
        Ok(())
    }

    pub fn mkdir_p(&mut self, path: &Path) -> VfsResult<()> {
        let abs = self.abs(path);
        let mut parts = Vec::new();

        for component in abs.components().skip(1) {
            if let Some(s) = component.as_os_str().to_str() {
                parts.push(s.to_owned());
            }
            let current = PathBuf::from(format!("/{}", parts.join("/")));
            match self.tree.get(&current) {
                Some(Node::Directory { .. }) => continue,
                Some(_) => return Err(VfsError::NotADirectory(current)),
                None => {
                    self.tree.insert(
                        current.clone(),
                        Node::Directory {
                            mode: DEFAULT_DIR_MODE,
                            modified: SystemTime::now(),
                        },
                    );
                }
            };
        }
        Ok(())
    }

    pub fn remove(&mut self, path: &Path) -> VfsResult<()> {
        let abs = self.abs(path);
        if abs.as_os_str() == "/" {
            return Err(VfsError::PermissionDenied(
                "cannot remove root directory".into(),
            ));
        }

        match self.tree.get(&abs) {
            Some(Node::Directory { .. }) => {
                // Check if directory is empty (no children)
                let prefix = if abs.to_str().unwrap_or("").ends_with('/') {
                    abs.to_string_lossy().to_string()
                } else {
                    format!("{}/", abs.to_string_lossy())
                };
                let has_children = self
                    .tree
                    .keys()
                    .any(|k| k.to_string_lossy().starts_with(&prefix));
                if has_children {
                    return Err(VfsError::DirectoryNotEmpty(abs));
                }
                self.tree.remove(&abs);
                Ok(())
            }
            Some(Node::File { content, .. }) => {
                self.bytes_used -= content.len() as u64;
                self.tree.remove(&abs);
                Ok(())
            }
            Some(Node::Symlink { .. }) => {
                self.tree.remove(&abs);
                Ok(())
            }
            None => Err(VfsError::NotFound(abs)),
        }
    }

    pub fn remove_r(&mut self, path: &Path) -> VfsResult<()> {
        let abs = self.abs(path);
        if abs.as_os_str() == "/" {
            return Err(VfsError::PermissionDenied(
                "cannot remove root directory".into(),
            ));
        }

        if !self.tree.contains_key(&abs) {
            return Err(VfsError::NotFound(abs));
        }

        let prefix = format!("{}/", abs.to_string_lossy());

        // Collect all paths to remove (children + the path itself)
        let to_remove: Vec<PathBuf> = self
            .tree
            .keys()
            .filter(|k| *k == &abs || k.to_string_lossy().starts_with(&prefix))
            .cloned()
            .collect();

        for p in &to_remove {
            if let Some(Node::File { content, .. }) = self.tree.get(p) {
                self.bytes_used -= content.len() as u64;
            }
            self.tree.remove(p);
        }

        Ok(())
    }

    pub fn list(&self, path: &Path) -> VfsResult<Vec<DirEntry>> {
        let resolved = self.resolve_symlinks(path)?;
        match self.tree.get(&resolved) {
            Some(Node::Directory { .. }) => {}
            Some(_) => return Err(VfsError::NotADirectory(resolved)),
            None => return Err(VfsError::NotFound(resolved)),
        }

        let prefix = if resolved.as_os_str() == "/" {
            "/".to_string()
        } else {
            format!("{}/", resolved.to_string_lossy())
        };

        let mut entries = Vec::new();
        for (k, node) in &self.tree {
            let key_str = k.to_string_lossy();
            if !key_str.starts_with(&prefix) {
                continue;
            }
            let remainder = &key_str[prefix.len()..];
            // Only direct children — no further `/` in the remainder
            if remainder.is_empty() || remainder.contains('/') {
                continue;
            }
            entries.push(DirEntry {
                name: remainder.to_string(),
                path: k.clone(),
                file_type: node.file_type(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    // ── Path operations ─────────────────────────────────────────

    pub fn rename(&mut self, from: &Path, to: &Path) -> VfsResult<()> {
        let from_abs = self.abs(from);
        let to_abs = self.abs(to);

        if from_abs.as_os_str() == "/" {
            return Err(VfsError::PermissionDenied("cannot rename root".into()));
        }

        if !self.tree.contains_key(&from_abs) {
            return Err(VfsError::NotFound(from_abs));
        }

        self.ensure_parent(&to_abs)?;

        // Collect all paths under from_abs (including itself)
        let from_prefix = format!("{}/", from_abs.to_string_lossy());
        let to_move: Vec<(PathBuf, Node)> = self
            .tree
            .keys()
            .filter(|k| *k == &from_abs || k.to_string_lossy().starts_with(&from_prefix))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|old_path| {
                let node = self.tree.remove(&old_path).unwrap();
                let new_path = if old_path == from_abs {
                    to_abs.clone()
                } else {
                    let suffix = &old_path.to_string_lossy()[from_abs.to_string_lossy().len()..];
                    PathBuf::from(format!("{}{suffix}", to_abs.to_string_lossy()))
                };
                (new_path, node)
            })
            .collect();

        for (path, node) in to_move {
            self.tree.insert(path, node);
        }

        Ok(())
    }

    pub fn symlink(&mut self, target: &Path, link: &Path) -> VfsResult<()> {
        let link_abs = self.abs(link);
        self.ensure_parent(&link_abs)?;

        if self.tree.contains_key(&link_abs) {
            return Err(VfsError::AlreadyExists(link_abs));
        }

        self.tree.insert(
            link_abs,
            Node::Symlink {
                target: target.to_path_buf(),
                modified: SystemTime::now(),
            },
        );
        Ok(())
    }

    pub fn readlink(&self, path: &Path) -> VfsResult<PathBuf> {
        let abs = self.abs(path);
        match self.tree.get(&abs) {
            Some(Node::Symlink { target, .. }) => Ok(target.clone()),
            Some(_) => Err(VfsError::NotASymlink(abs)),
            None => Err(VfsError::NotFound(abs)),
        }
    }

    pub fn realpath(&self, path: &Path) -> VfsResult<PathBuf> {
        self.resolve_symlinks(path)
    }

    // ── Search ──────────────────────────────────────────────────

    pub fn glob(&self, pattern: &str) -> VfsResult<Vec<PathBuf>> {
        glob::expand(pattern, &self.cwd, &self.tree)
    }

    // ── Working directory ───────────────────────────────────────

    pub fn getcwd(&self) -> &Path {
        &self.cwd
    }

    pub fn chdir(&mut self, path: &Path) -> VfsResult<()> {
        let resolved = self.resolve_symlinks(path)?;
        match self.tree.get(&resolved) {
            Some(Node::Directory { .. }) => {
                self.cwd = resolved;
                Ok(())
            }
            Some(_) => Err(VfsError::NotADirectory(resolved)),
            None => Err(VfsError::NotFound(resolved)),
        }
    }

    // ── Memory management ───────────────────────────────────────

    pub fn usage(&self) -> MemoryUsage {
        MemoryUsage {
            bytes_used: self.bytes_used,
            bytes_limit: self.bytes_limit,
        }
    }

    pub fn set_limit(&mut self, bytes: u64) {
        self.bytes_limit = bytes;
    }

    // ── Internals exposed for other crates ──────────────────────

    /// Get a reference to the internal tree (used by loader, wasm engine, etc.)
    pub fn tree(&self) -> &BTreeMap<PathBuf, Node> {
        &self.tree
    }

    /// Get a mutable reference to the internal tree (used by loader).
    pub fn tree_mut(&mut self) -> &mut BTreeMap<PathBuf, Node> {
        &mut self.tree
    }

    /// Directly adjust the bytes_used counter (used by loader).
    pub fn add_bytes_used(&mut self, bytes: u64) {
        self.bytes_used += bytes;
    }
}

impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}
