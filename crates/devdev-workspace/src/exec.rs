//! `Workspace` top-level type and `Workspace::exec`.
//!
//! `Workspace` owns the in-memory [`Fs`] plus an optional platform
//! driver that mounts it at a host tempdir. `exec` runs a real host
//! binary inside that mount under a PTY with a curated environment.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::PtySize;

use crate::driver::{DriverError, MountHandle};
use crate::mem::Fs;
use crate::pty::Pty;

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("workspace is not mounted")]
    NotMounted,
    #[error("spawn failed: {0}")]
    Spawn(io::Error),
    #[error("i/o error: {0}")]
    Io(io::Error),
    #[error("exec timed out")]
    Timeout,
}

pub struct Workspace {
    fs: Arc<Mutex<Fs>>,
    driver: Option<Box<dyn MountHandle>>,
    _mount_tempdir: Option<tempfile::TempDir>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            fs: Arc::new(Mutex::new(Fs::new())),
            driver: None,
            _mount_tempdir: None,
        }
    }

    pub fn from_fs(fs: Fs) -> Self {
        Self {
            fs: Arc::new(Mutex::new(fs)),
            driver: None,
            _mount_tempdir: None,
        }
    }

    pub fn fs(&self) -> Arc<Mutex<Fs>> {
        self.fs.clone()
    }

    pub fn mount_point(&self) -> Option<&Path> {
        self.driver.as_deref().map(|d| d.mount_point())
    }

    /// Mount the virtual fs at a freshly-created tempdir.
    #[cfg(target_os = "linux")]
    pub fn mount(&mut self) -> Result<PathBuf, DriverError> {
        let tmp = tempfile::Builder::new()
            .prefix("devdev-ws-")
            .tempdir()
            .map_err(DriverError::Io)?;
        let mp = tmp.path().to_path_buf();
        let driver = crate::driver::fuse::FuseDriver::mount(self.fs.clone(), &mp)?;
        self.driver = Some(Box::new(driver));
        self._mount_tempdir = Some(tmp);
        Ok(mp)
    }

    /// Mount the virtual fs at an auto-selected free drive letter.
    /// WinFSP does not support mounting an in-memory FS at an
    /// arbitrary directory, so we take a drive letter (Z: down).
    #[cfg(target_os = "windows")]
    pub fn mount(&mut self) -> Result<PathBuf, DriverError> {
        let driver = crate::driver::winfsp::WinFspDriver::mount_auto(self.fs.clone())?;
        let mp = driver.mount_point().to_path_buf();
        self.driver = Some(Box::new(driver));
        Ok(mp)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    pub fn mount(&mut self) -> Result<PathBuf, DriverError> {
        Err(DriverError::Unimplemented)
    }

    /// Run a command inside the mounted workspace. `cwd_in_fs` is a
    /// POSIX path relative to the mount root (must start with `/`).
    /// Streams combined stdout+stderr into `output`. Returns the
    /// child's exit code.
    ///
    /// The child runs with a curated environment — nothing is
    /// inherited from the parent except `PATH`. The complete set of
    /// variables the child sees is:
    ///
    /// - `HOME=/home/agent`
    /// - `CARGO_HOME=/home/agent/.cargo`
    /// - `USER=agent`
    /// - `LOGNAME=agent`
    /// - `SHELL=/bin/sh`
    /// - `TERM=dumb`
    /// - `PATH=<inherited from host>`
    ///
    /// Anything else (e.g. `LD_*`, `WSL_*`, user locale, parent shell
    /// state) is stripped. Drivers may inject `TERM` / `PWD` / etc.
    /// on top of this baseline; see `tests/env_sanitization.rs` for
    /// the full accepted set.
    pub fn exec(
        &self,
        cmd: &OsStr,
        args: &[&OsStr],
        cwd_in_fs: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<i32, ExecError> {
        let Some(driver) = self.driver.as_deref() else {
            return Err(ExecError::NotMounted);
        };
        if cwd_in_fs.is_empty() || cwd_in_fs[0] != b'/' {
            return Err(ExecError::Spawn(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cwd_in_fs must start with '/'",
            )));
        }
        let host_cwd = join_mount(driver.mount_point(), cwd_in_fs);

        let env = curated_env();
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut pty = Pty::spawn(cmd, args, &host_cwd, &env, size).map_err(ExecError::Spawn)?;

        // Dedicated reader thread: portable-pty's reader is blocking.
        let reader = pty.take_reader();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let reader_handle = reader.map(|mut r| {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match r.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
        });

        let exit_code: i32;
        loop {
            while let Ok(chunk) = rx.try_recv() {
                output.extend_from_slice(&chunk);
            }
            match pty.try_wait().map_err(ExecError::Io)? {
                Some(status) => {
                    exit_code = status.exit_code() as i32;
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }

        // Give the reader a moment to drain the final bytes after EOF.
        if let Some(h) = reader_handle {
            // Drop the pty so the slave fd closes and reader sees EOF.
            drop(pty);
            let _ = h.join();
        }
        while let Ok(chunk) = rx.try_recv() {
            output.extend_from_slice(&chunk);
        }

        Ok(exit_code)
    }
}

fn curated_env() -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = vec![
        ("HOME".into(), "/home/agent".into()),
        ("CARGO_HOME".into(), "/home/agent/.cargo".into()),
        ("USER".into(), "agent".into()),
        ("LOGNAME".into(), "agent".into()),
        ("SHELL".into(), "/bin/sh".into()),
        ("TERM".into(), "dumb".into()),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        env.push(("PATH".into(), path));
    }
    env
}

fn join_mount(mount: &Path, cwd_in_fs: &[u8]) -> PathBuf {
    // Strip the leading slash(es); everything after is relative.
    let mut i = 0;
    while i < cwd_in_fs.len() && cwd_in_fs[i] == b'/' {
        i += 1;
    }
    let tail = &cwd_in_fs[i..];
    if tail.is_empty() {
        return mount.to_path_buf();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        mount.join(OsStr::from_bytes(tail))
    }
    #[cfg(not(unix))]
    {
        // Exec is gated to mounted workspaces; on non-Linux we never
        // get here because `mount` returns Unimplemented.
        let s = String::from_utf8_lossy(tail);
        mount.join(s.as_ref())
    }
}
