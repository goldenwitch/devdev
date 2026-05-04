//! Daemon lifecycle & IPC for DevDev.
//!
//! The daemon is the long-running process that owns the virtual
//! workspace filesystem (`Fs`) and task state. CLI commands talk
//! to it over IPC.

pub mod checkpoint;
pub mod credentials;
pub mod dispatch;
pub mod host_registry;
pub mod ipc;
pub mod ledger;
pub mod mcp;
pub mod pid;
pub mod router;
pub mod runner;
pub mod server;

use std::path::PathBuf;
use std::sync::Arc;

use devdev_workspace::Fs;
use tokio::sync::Mutex;

/// Daemon configuration.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Data directory (default: `~/.devdev/`).
    pub data_dir: PathBuf,
    /// Whether to save a checkpoint on stop.
    pub checkpoint_on_stop: bool,
    /// Run in foreground (don't detach).
    pub foreground: bool,
}

impl DaemonConfig {
    /// Resolve the default data directory from `DEVDEV_HOME` or `~/.devdev/`.
    pub fn default_data_dir() -> PathBuf {
        if let Ok(val) = std::env::var("DEVDEV_HOME") {
            return PathBuf::from(val);
        }
        dirs_or_home().join(".devdev")
    }
}

fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            data_dir: DaemonConfig::default_data_dir(),
            checkpoint_on_stop: true,
            foreground: false,
        }
    }
}

/// Daemon error type.
#[derive(thiserror::Error, Debug)]
pub enum DaemonError {
    #[error("daemon already running (PID {0})")]
    AlreadyRunning(u32),

    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("VFS error: {0}")]
    Vfs(#[from] devdev_workspace::Errno),
}

/// The daemon process.
pub struct Daemon {
    pub config: DaemonConfig,
    pub fs: Arc<Mutex<Fs>>,
}

impl Daemon {
    /// Boot the daemon, optionally restoring from a checkpoint.
    pub async fn start(config: DaemonConfig, from_checkpoint: bool) -> Result<Self, DaemonError> {
        std::fs::create_dir_all(&config.data_dir)?;

        // Single-instance guard.
        if let Some(pid) = pid::read_pid(&config.data_dir)? {
            if pid::is_alive(pid) {
                return Err(DaemonError::AlreadyRunning(pid));
            }
            // Stale PID file — clean it up.
            pid::remove_pid(&config.data_dir)?;
        }

        // Restore or create fresh filesystem.
        let fs = if from_checkpoint {
            let cp_path = config.data_dir.join("checkpoint.bin");
            if cp_path.exists() {
                let data = std::fs::read(&cp_path)?;
                Fs::deserialize(&data)?
            } else {
                Fs::new()
            }
        } else {
            Fs::new()
        };

        // Write PID file.
        pid::write_pid(&config.data_dir)?;

        Ok(Daemon {
            config,
            fs: Arc::new(Mutex::new(fs)),
        })
    }

    /// Save checkpoint and shut down cleanly.
    pub async fn stop(&self) -> Result<(), DaemonError> {
        if self.config.checkpoint_on_stop {
            self.save_checkpoint().await?;
        }
        pid::remove_pid(&self.config.data_dir)?;
        Ok(())
    }

    /// Save a filesystem checkpoint atomically.
    pub async fn save_checkpoint(&self) -> Result<(), DaemonError> {
        let data = {
            let fs = self.fs.lock().await;
            fs.serialize()
        };
        checkpoint::atomic_write(&self.config.data_dir.join("checkpoint.bin"), &data)?;
        Ok(())
    }
}
