//! Thin `portable-pty` wrapper. Spawns a child in a freshly-created
//! PTY with a fully-curated environment (no host inheritance unless
//! the caller explicitly passes it through).

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::path::Path;

use portable_pty::{
    Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system,
};

pub use portable_pty::PtySize as Size;

pub struct Pty {
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
}

impl Pty {
    /// Spawn `cmd` with `args`, `cwd`, and the exact `env` set (no
    /// inheritance).
    pub fn spawn(
        cmd: &OsStr,
        args: &[&OsStr],
        cwd: &Path,
        env: &[(OsString, OsString)],
        size: PtySize,
    ) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|e| io::Error::other(e.to_string()))?;

        let mut builder = CommandBuilder::new(cmd);
        for a in args {
            builder.arg(a);
        }
        builder.cwd(cwd);
        builder.env_clear();
        for (k, v) in env {
            builder.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| io::Error::other(e.to_string()))?;
        // Slave is kept open in the child; drop our end.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| io::Error::other(e.to_string()))?;

        Ok(Self {
            master: pair.master,
            child,
            reader: Some(reader),
            writer,
        })
    }

    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(r) = self.reader.as_mut() else {
            return Ok(0);
        };
        r.read(buf)
    }

    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    /// Steal the reader so a dedicated thread can drive it. After
    /// this, [`Pty::read`] returns `Ok(0)`.
    pub fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().map_err(io::Error::other)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait().map_err(io::Error::other)
    }

    pub fn kill(&mut self) -> io::Result<()> {
        self.child.kill().map_err(io::Error::other)
    }
}
