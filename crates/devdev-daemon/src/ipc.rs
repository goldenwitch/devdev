//! IPC types and transport for daemon ↔ CLI/TUI communication.
//!
//! Protocol: NDJSON over TCP (localhost). Each message is a JSON object
//! terminated by `\n`. Requests have an `id` field for multiplexing.

use serde::{Deserialize, Serialize};

/// An IPC request from a CLI/TUI client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// An IPC response from the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcError>,
}

impl IpcResponse {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, code: i32, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(IpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcError {
    pub code: i32,
    pub message: String,
}

/// IPC server listening for connections on TCP localhost.
pub struct IpcServer {
    listener: tokio::net::TcpListener,
    port: u16,
}

impl IpcServer {
    /// Bind to a random available port on localhost.
    pub async fn bind() -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    /// The port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Accept a new connection.
    pub async fn accept(&self) -> Result<IpcConnection, std::io::Error> {
        let (stream, _addr) = self.listener.accept().await?;
        Ok(IpcConnection::new(stream))
    }

    /// Write the port to `data_dir/daemon.port` so CLI clients can find us.
    pub fn write_port_file(&self, data_dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(data_dir.join("daemon.port"), self.port.to_string())
    }
}

/// Read the daemon port from the port file.
pub fn read_port(data_dir: &std::path::Path) -> std::io::Result<Option<u16>> {
    let path = data_dir.join("daemon.port");
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(content.trim().parse().ok())
}

/// A single IPC connection (from a CLI command or TUI).
pub struct IpcConnection {
    reader: tokio::io::BufReader<tokio::io::ReadHalf<tokio::net::TcpStream>>,
    writer: tokio::io::WriteHalf<tokio::net::TcpStream>,
}

impl IpcConnection {
    fn new(stream: tokio::net::TcpStream) -> Self {
        let (read_half, write_half) = tokio::io::split(stream);
        Self {
            reader: tokio::io::BufReader::new(read_half),
            writer: write_half,
        }
    }

    /// Read a JSON request line.
    pub async fn read_request(&mut self) -> Result<Option<IpcRequest>, std::io::Error> {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let req: IpcRequest = serde_json::from_str(line.trim())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Some(req))
    }

    /// Write a JSON response line.
    pub async fn write_response(&mut self, resp: &IpcResponse) -> Result<(), std::io::Error> {
        use tokio::io::AsyncWriteExt;
        let mut data = serde_json::to_vec(resp)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        data.push(b'\n');
        self.writer.write_all(&data).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

/// Client-side IPC connection to the daemon.
pub struct IpcClient {
    conn: IpcConnection,
    next_id: u64,
}

impl IpcClient {
    /// Connect to the daemon at the given port.
    pub async fn connect(port: u16) -> Result<Self, std::io::Error> {
        let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await?;
        Ok(Self {
            conn: IpcConnection::new(stream),
            next_id: 1,
        })
    }

    /// Send a request and read the response.
    pub async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<IpcResponse, std::io::Error> {
        let id = self.next_id;
        self.next_id += 1;

        let req = IpcRequest {
            id,
            method: method.to_string(),
            params,
        };

        use tokio::io::AsyncWriteExt;
        let mut data = serde_json::to_vec(&req)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        data.push(b'\n');
        self.conn.writer.write_all(&data).await?;
        self.conn.writer.flush().await?;

        // Read response line.
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        let n = self.conn.reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(IpcResponse::err(id, -1, "connection closed"));
        }
        serde_json::from_str(line.trim())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
