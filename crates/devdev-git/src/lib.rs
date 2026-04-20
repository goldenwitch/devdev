//! In-memory Git operations for DevDev sandbox.
//!
//! Loads `.git` directories into libgit2's in-memory object database
//! and implements read-only git subcommands (diff, log, blame, etc.).

pub mod commands;
pub mod loader;

pub use commands::{BLOCKED, GitResult, VirtualGit, VirtualGitRepo};
pub use loader::{GitLoadError, VirtualRepo};
