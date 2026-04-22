//! Daemon server: accept loop that dispatches IPC requests.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{info, warn};

use crate::dispatch::DispatchContext;
use crate::ipc::IpcServer;

/// Run the daemon's accept loop until shutdown.
pub async fn run(
    ctx: Arc<DispatchContext>,
    server: IpcServer,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            accept_result = server.accept() => {
                match accept_result {
                    Ok(conn) => {
                        let ctx = Arc::clone(&ctx);
                        tokio::spawn(handle_connection(ctx, conn));
                    }
                    Err(e) => {
                        warn!("accept error: {e}");
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("daemon server shutting down");
                    break;
                }
            }
        }
    }
}

/// Handle a single client connection.
async fn handle_connection(ctx: Arc<DispatchContext>, mut conn: crate::ipc::IpcConnection) {
    loop {
        match conn.read_request().await {
            Ok(Some(req)) => {
                let resp = ctx.dispatch(req).await;
                if let Err(e) = conn.write_response(&resp).await {
                    warn!("write error: {e}");
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!("read error: {e}");
                break;
            }
        }
    }
}
