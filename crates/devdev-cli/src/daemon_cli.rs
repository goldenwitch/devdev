//! Daemon-facing subcommands: `devdev up / down / send / status`.
//!
//! Thin glue that wires the existing `devdev-daemon` machinery
//! (`Daemon`, `IpcServer`, `DispatchContext`, `SessionRouter`) into
//! the `devdev` binary. All state lives in the daemon crate — this
//! module only wires it up.
//!
//! ## v1 scope caveats
//!
//! * `--foreground` is the only supported lifecycle: even without the
//!   flag we still run foreground. Real daemonisation (fork/detach)
//!   is a follow-up; the flag is accepted now so scripts don't break
//!   later.
//! * The [`AcpSessionBackend`](crate::acp_backend::AcpSessionBackend)
//!   is live — `devdev send` spawns `copilot --acp --allow-all-tools`,
//!   multiplexes sessions, and surfaces agent replies (proven by the
//!   gated `live_mcp` tests). The remaining stub is
//!   [`placeholder_review_fn`]: `MonitorPrTask`'s review callback
//!   returns an empty string, so `task/add monitor_pr` succeeds but
//!   posts no review text. Wiring it to a per-task router session is
//!   cap 22's work.
//! * `ApprovalPolicy::AutoApprove` is hard-wired; approval-gate UX
//!   arrives with the TUI work.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use tokio::sync::{Mutex, watch};

use devdev_daemon::dispatch::DispatchContext;
use devdev_daemon::ipc::{IpcClient, IpcServer, read_port};
use devdev_daemon::mcp::{DaemonToolProvider, McpServer};
use devdev_daemon::router::SessionRouter;
use devdev_daemon::{Daemon, DaemonConfig, DaemonError, server};
use devdev_integrations::{GitHubAdapter, LiveGitHubAdapter, MockGitHubAdapter};
use devdev_tasks::approval::{ApprovalPolicy, approval_channel};
use devdev_tasks::monitor_pr::ReviewFn;
use devdev_tasks::registry::TaskRegistry;

use crate::acp_backend::AcpSessionBackend;

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

// ─── Args ──────────────────────────────────────────────────────

#[derive(Parser, Debug, Clone)]
pub struct UpArgs {
    /// Daemon data directory (defaults to `$DEVDEV_HOME` or `~/.devdev`).
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Restore VFS + task state from the last checkpoint.
    #[arg(long)]
    pub checkpoint: bool,

    /// Run attached to the current terminal (v1 always runs foreground;
    /// flag is reserved for future detached mode).
    #[arg(long)]
    pub foreground: bool,

    /// GitHub adapter backend. Overrides `$DEVDEV_GITHUB_ADAPTER`.
    #[arg(long, value_parser = ["live", "mock"])]
    pub github: Option<String>,

    /// Agent program (only used once the session backend is wired).
    #[arg(long, default_value = "copilot")]
    pub agent_program: String,

    /// Extra arguments passed to the agent program. Defaults to
    /// `--acp --allow-all-tools` for Copilot CLI — ACP/NDJSON mode with
    /// non-interactive tool permissions, validated by the P2-06 PoC.
    /// Override with `--agent-arg ...` (repeat) when using a different
    /// agent; pass `--agent-arg ""` to clear.
    #[arg(long, num_args = 0.., default_values_t = ["--acp".to_string(), "--allow-all-tools".to_string()])]
    pub agent_arg: Vec<String>,
}

#[derive(Parser, Debug, Clone)]
pub struct DownArgs {
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
}

#[derive(Parser, Debug, Clone)]
pub struct SendArgs {
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Emit the raw IPC response JSON on stdout.
    #[arg(long)]
    pub json: bool,

    /// Prompt text to forward to the interactive session.
    pub text: String,
}

#[derive(Parser, Debug, Clone)]
pub struct StatusArgs {
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Emit the raw IPC response JSON on stdout.
    #[arg(long)]
    pub json: bool,
}

// ─── Helpers ───────────────────────────────────────────────────

fn resolve_data_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(DaemonConfig::default_data_dir)
}

fn select_github_adapter(flag: Option<&str>) -> Arc<dyn GitHubAdapter> {
    let choice = flag
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("DEVDEV_GITHUB_ADAPTER").ok())
        .unwrap_or_else(|| "live".to_string());

    match choice.as_str() {
        "mock" => Arc::new(MockGitHubAdapter::new()),
        _ => match LiveGitHubAdapter::from_env() {
            Ok(adapter) => Arc::new(adapter),
            Err(e) => {
                eprintln!(
                    "devdev: GH_TOKEN not available ({e}); falling back to mock GitHub adapter"
                );
                Arc::new(MockGitHubAdapter::new())
            }
        },
    }
}

/// Placeholder review callback. TODO: swap for a real router-backed
/// review once `AcpSessionBackend` is wired.
fn placeholder_review_fn() -> ReviewFn {
    Arc::new(|_prompt: String| Box::pin(async move { Ok(String::new()) }))
}

async fn connect_ipc(data_dir: &Path) -> Result<IpcClient> {
    let port = read_port(data_dir)
        .with_context(|| format!("reading port file in {}", data_dir.display()))?
        .ok_or_else(|| {
            anyhow!(
                "daemon not running (no port file in {})",
                data_dir.display()
            )
        })?;
    IpcClient::connect(port)
        .await
        .with_context(|| format!("connecting to daemon on port {port}"))
}

// ─── up ────────────────────────────────────────────────────────

/// Run the daemon in the foreground until Ctrl-C or an IPC `shutdown`.
///
/// Real daemonisation (fork/detach on unix, service on windows) is
/// explicitly out of v1 scope — `--foreground` is accepted but doesn't
/// change behaviour. All lifecycle state (PID file, port file,
/// checkpoint) is owned by the `Daemon` / `IpcServer` types; this
/// function is just the composition root.
pub async fn run_up(args: UpArgs) -> Result<()> {
    let data_dir = resolve_data_dir(args.data_dir.clone());

    let config = DaemonConfig {
        data_dir: data_dir.clone(),
        checkpoint_on_stop: true,
        foreground: true,
    };

    let daemon = match Daemon::start(config, args.checkpoint).await {
        Ok(d) => d,
        Err(DaemonError::AlreadyRunning(pid)) => {
            return Err(anyhow!("daemon already running (PID {pid})"));
        }
        Err(e) => return Err(anyhow!("failed to start daemon: {e}")),
    };

    let server = IpcServer::bind()
        .await
        .context("binding IPC server on localhost")?;
    let port = server.port();
    server
        .write_port_file(&data_dir)
        .context("writing daemon.port file")?;

    // Dispatch context wiring.
    // Task registry is built first so the MCP server can wrap it in a
    // provider and have its URL ready before the ACP backend spawns.
    let tasks = Arc::new(Mutex::new(TaskRegistry::new()));

    // Start the local MCP server. Loopback-only, bearer-auth'd, stateless.
    // Dropped below after the IPC accept loop exits.
    //
    // Both the MCP provider and the dispatch context share the
    // daemon's `Arc<Mutex<Fs>>` so an agent-driven `devdev_fs_write`
    // and a user-driven `fs/read` IPC call observe the same bytes.
    let fs = Arc::clone(&daemon.fs);
    let mcp_provider = Arc::new(DaemonToolProvider::new(Arc::clone(&tasks), Arc::clone(&fs)));
    let mcp_server = McpServer::start(mcp_provider)
        .await
        .context("starting local MCP server")?;
    let mcp_endpoint = mcp_server.endpoint().clone();
    eprintln!("DevDev MCP server listening at {}", mcp_endpoint.url);

    let backend = Arc::new(AcpSessionBackend::new(
        args.agent_program.clone(),
        args.agent_arg.clone(),
        Some(mcp_endpoint),
    ));
    let router = Arc::new(SessionRouter::new(backend));
    let github = select_github_adapter(args.github.as_deref());
    let policy = ApprovalPolicy::AutoApprove;
    let (_gate, handle) = approval_channel(policy, APPROVAL_TIMEOUT);
    let approval_handle = Arc::new(Mutex::new(handle));
    let review_fn = placeholder_review_fn();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let ctx = Arc::new(DispatchContext::new(
        router,
        tasks,
        github,
        approval_handle,
        review_fn,
        policy,
        shutdown_tx.clone(),
        fs,
    ));

    let server_task = tokio::spawn(server::run(Arc::clone(&ctx), server, shutdown_rx));

    eprintln!(
        "DevDev daemon started (pid {}, port {port})",
        std::process::id()
    );

    // Wait for either Ctrl-C or an IPC `shutdown` call.
    let mut shutdown_watch = shutdown_tx.subscribe();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            let _ = shutdown_tx.send(true);
        }
        _ = shutdown_watch.changed() => {
            // `shutdown` IPC method already flipped the flag.
        }
    }

    // Let the accept loop observe the flag and exit.
    let _ = server_task.await;

    // Stop the MCP server so its port is released before we claim
    // shutdown is done. Shutdown is graceful — no pending requests.
    mcp_server.shutdown().await;

    if let Err(e) = daemon.stop().await {
        eprintln!("devdev: error during shutdown: {e}");
    }

    // Best-effort: remove the port file too. (PID file is removed by Daemon::stop.)
    let _ = std::fs::remove_file(data_dir.join("daemon.port"));

    eprintln!("Checkpoint saved. Daemon stopped.");
    Ok(())
}

// ─── down ──────────────────────────────────────────────────────

pub async fn run_down(args: DownArgs) -> Result<()> {
    let data_dir = resolve_data_dir(args.data_dir);
    let mut client = connect_ipc(&data_dir).await?;
    let resp = client
        .request("shutdown", serde_json::json!({}))
        .await
        .context("sending shutdown IPC request")?;
    if let Some(err) = resp.error {
        return Err(anyhow!("daemon refused shutdown: {}", err.message));
    }
    eprintln!("Shutdown requested.");
    Ok(())
}

// ─── send ──────────────────────────────────────────────────────

pub async fn run_send(args: SendArgs) -> Result<()> {
    let data_dir = resolve_data_dir(args.data_dir);
    let mut client = connect_ipc(&data_dir).await?;
    let resp = client
        .request("send", serde_json::json!({ "text": args.text }))
        .await
        .context("sending IPC send request")?;

    if args.json {
        println!("{}", serde_json::to_string(&resp)?);
        return Ok(());
    }

    if let Some(err) = resp.error {
        return Err(anyhow!("send failed: {}", err.message));
    }
    let result = resp
        .result
        .ok_or_else(|| anyhow!("daemon returned neither result nor error"))?;
    let response_text = result
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    println!("{response_text}");
    Ok(())
}

// ─── status ────────────────────────────────────────────────────

pub async fn run_status(args: StatusArgs) -> Result<()> {
    let data_dir = resolve_data_dir(args.data_dir);
    let mut client = connect_ipc(&data_dir).await?;
    let resp = client
        .request("status", serde_json::json!({}))
        .await
        .context("sending status IPC request")?;

    if args.json {
        let payload = resp
            .result
            .clone()
            .ok_or_else(|| anyhow!("daemon returned no result"))?;
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    if let Some(err) = resp.error {
        return Err(anyhow!("status failed: {}", err.message));
    }
    let result = resp
        .result
        .ok_or_else(|| anyhow!("daemon returned neither result nor error"))?;
    let tasks = result.get("tasks").and_then(|v| v.as_u64()).unwrap_or(0);
    let sessions = result.get("sessions").and_then(|v| v.as_u64()).unwrap_or(0);
    println!("tasks={tasks}");
    println!("sessions={sessions}");
    Ok(())
}
