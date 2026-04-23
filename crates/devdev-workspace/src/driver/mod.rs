//! Platform-specific virtual-fs driver.
//!
//! A `VirtualFsDriver` owns a mount: it takes a shared handle to the
//! in-memory `Fs` (behind a `Mutex`) and presents it at a real host
//! path using the OS's native filesystem-in-userspace mechanism.
//! Linux uses FUSE (`fuser` crate). Windows will use WinFSP in a
//! later phase — currently a stub.
//!
//! Drivers are single-mount: one driver = one mount point. Drop the
//! driver to unmount.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub mod fuse;

#[cfg(target_os = "windows")]
pub mod winfsp;

#[cfg(target_os = "windows")]
mod winfsp_sys;

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("driver not yet implemented for this platform")]
    Unimplemented,

    #[error("mount failed: {0}")]
    Mount(String),

    #[error("mount point {0} is not a directory or does not exist")]
    BadMountPoint(PathBuf),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// A handle to a live mount. Drop to unmount.
pub trait MountHandle: Send {
    /// The host path where the virtual fs is mounted.
    fn mount_point(&self) -> &std::path::Path;
}
