//! Virtual workspace driver.
//!
//! Three collaborators glued together by [`Workspace`]:
//!
//! * [`mem::Fs`] — inode-centric in-memory filesystem (the backing
//!   store).
//! * [`driver`] — platform-native FS-in-userspace driver that presents
//!   `Fs` at a real host path (FUSE on Linux, WinFSP on Windows).
//! * [`exec`] + [`pty`] — spawn a real host binary inside the mount
//!   under a PTY with a curated environment.
//!
//! This crate is the post-Phase-3 consolidation of the original
//! in-memory sandbox crates (`devdev-vfs`/`-wasm`/`-git`/`-shell`).
//! See `docs/internals/capabilities/README.md` for the crate-map history.

pub mod driver;
pub mod exec;
pub mod mem;
pub mod pty;

pub use exec::{ExecError, Workspace};
pub use mem::{
    DEFAULT_LIMIT, DirEntry, Errno, Fs, Ino, InodeAttr, Kind, ROOT_INO, SNAPSHOT_MAGIC,
    SNAPSHOT_VERSION, SetAttr, Snapshot, SnapshotBody, SnapshotInode, Timespec,
};
