//! Async NDJSON transport over arbitrary `AsyncRead` / `AsyncWrite`.
//!
//! Keeps the wire format identical to [`crate::ndjson`] (sync) while making
//! the client testable without spawning a subprocess — any duplex byte
//! stream works.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::protocol::Message;

/// Async, single-message reader. Wrap any `AsyncRead + Unpin` (stdout pipe,
/// in-memory duplex, TCP stream…).
pub struct AsyncNdjsonReader<R: AsyncRead + Unpin> {
    reader: BufReader<R>,
    line: String,
}

impl<R: AsyncRead + Unpin> AsyncNdjsonReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            reader: BufReader::new(inner),
            line: String::new(),
        }
    }

    /// Read the next message. `Ok(None)` at EOF.
    pub async fn recv(&mut self) -> io::Result<Option<Message>> {
        loop {
            self.line.clear();
            let n = self.reader.read_line(&mut self.line).await?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = self.line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: Message = serde_json::from_str(trimmed)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            return Ok(Some(msg));
        }
    }
}

/// Async, single-message writer.
pub struct AsyncNdjsonWriter<W: AsyncWrite + Unpin> {
    writer: W,
}

impl<W: AsyncWrite + Unpin> AsyncNdjsonWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub async fn send(&mut self, msg: &Message) -> io::Result<()> {
        let mut json =
            serde_json::to_vec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        json.push(b'\n');
        self.writer.write_all(&json).await?;
        self.writer.flush().await
    }
}
