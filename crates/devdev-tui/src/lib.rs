//! Chat TUI & Headless Mode for DevDev.
//!
//! Two modes of human interaction with the daemon: a terminal UI for
//! interactive use, and a headless NDJSON pipe for CI/scripting/embedding.

pub mod chat;
pub mod headless;
pub mod ipc_client;

pub use chat::{ChatMessage, ChatRole};
pub use ipc_client::{ConnectError, DaemonConnection, DaemonEvent};
