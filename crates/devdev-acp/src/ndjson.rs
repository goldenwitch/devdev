//! NDJSON (newline-delimited JSON) reader and writer.

use std::io::{self, BufRead, Write};

use crate::protocol::Message;

/// Writes ACP messages as newline-delimited JSON.
pub struct NdjsonWriter<W: Write> {
    writer: W,
}

impl<W: Write> NdjsonWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Serialize and write a single message, followed by a newline.
    pub fn send(&mut self, msg: &Message) -> io::Result<()> {
        let json = serde_json::to_string(msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.writer.write_all(json.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }

    /// Get a reference to the inner writer.
    pub fn inner(&self) -> &W {
        &self.writer
    }
}

/// Reads ACP messages from a newline-delimited JSON stream.
pub struct NdjsonReader<R: BufRead> {
    reader: R,
}

impl<R: BufRead> NdjsonReader<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }

    /// Read and deserialize the next message. Returns `None` at EOF.
    pub fn recv(&mut self) -> io::Result<Option<Message>> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 {
                return Ok(None); // EOF
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue; // skip blank lines
            }
            let msg: Message = serde_json::from_str(trimmed)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            return Ok(Some(msg));
        }
    }
}
