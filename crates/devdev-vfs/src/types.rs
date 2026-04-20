use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

/// Type of a filesystem node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
    Symlink,
}

impl fmt::Display for FileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileType::File => write!(f, "file"),
            FileType::Directory => write!(f, "directory"),
            FileType::Symlink => write!(f, "symlink"),
        }
    }
}

/// Metadata about a filesystem node.
#[derive(Debug, Clone)]
pub struct FileStat {
    pub size: u64,
    pub file_type: FileType,
    pub permissions: u32,
    pub modified: SystemTime,
}

/// An entry within a directory listing.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub file_type: FileType,
}

/// Memory usage tracking for the VFS.
#[derive(Debug, Clone, Copy)]
pub struct MemoryUsage {
    pub bytes_used: u64,
    pub bytes_limit: u64,
}

/// Internal node representation in the in-memory tree.
#[derive(Debug, Clone)]
pub enum Node {
    File {
        content: Vec<u8>,
        mode: u32,
        modified: SystemTime,
    },
    Directory {
        mode: u32,
        modified: SystemTime,
    },
    Symlink {
        target: PathBuf,
        modified: SystemTime,
    },
}

impl Node {
    pub(crate) fn file_type(&self) -> FileType {
        match self {
            Node::File { .. } => FileType::File,
            Node::Directory { .. } => FileType::Directory,
            Node::Symlink { .. } => FileType::Symlink,
        }
    }

    pub(crate) fn mode(&self) -> u32 {
        match self {
            Node::File { mode, .. } => *mode,
            Node::Directory { mode, .. } => *mode,
            Node::Symlink { .. } => 0o777,
        }
    }

    pub(crate) fn modified(&self) -> SystemTime {
        match self {
            Node::File { modified, .. } => *modified,
            Node::Directory { modified, .. } => *modified,
            Node::Symlink { modified, .. } => *modified,
        }
    }

    pub(crate) fn size(&self) -> u64 {
        match self {
            Node::File { content, .. } => content.len() as u64,
            Node::Directory { .. } => 0,
            Node::Symlink { target, .. } => target.as_os_str().len() as u64,
        }
    }

    pub(crate) fn stat(&self) -> FileStat {
        FileStat {
            size: self.size(),
            file_type: self.file_type(),
            permissions: self.mode(),
            modified: self.modified(),
        }
    }
}
