//! Virtual workspace driver.
//!
//! Phase 1 is complete: inode-centric MemFs (`mem`). Phase 2 adds the
//! OS-native filesystem driver (`driver`), PTY wrapper (`pty`), and
//! high-level `exec` entry point. Higher-level orchestration (the
//! `Workspace` struct that glues MemFs + driver + exec together)
//! lives in this module.

pub mod driver;
pub mod exec;
pub mod mem;
pub mod pty;

pub use exec::{ExecError, Workspace};
pub use mem::{
    DEFAULT_LIMIT, DirEntry, Errno, Fs, Ino, InodeAttr, Kind, ROOT_INO, SNAPSHOT_MAGIC,
    SNAPSHOT_VERSION, SetAttr, Snapshot, SnapshotBody, SnapshotInode, Timespec,
};
