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
//!   gated `live_mcp` tests). MonitorPr tasks now drive the agent
//!   via `devdev_daemon::runner::RouterRunner`, which holds one
//!   session per task and forwards prompts produced by `EventBus`
//!   triggers (PR opened/updated/closed).
//! * `ApprovalPolicy::AutoApprove` is hard-wired; approval-gate UX
//!   arrives with the TUI work.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use tokio::sync::{Mutex, watch};

use devdev_daemon::dispatch::{DispatchContext, spawn_event_coordinator};
use devdev_daemon::ipc::{IpcClient, IpcServer, read_port};
use devdev_daemon::ledger::NdjsonLedger;
use devdev_daemon::mcp::{DaemonToolProvider, McpServer};
use devdev_daemon::router::SessionRouter;
use devdev_daemon::{Daemon, DaemonConfig, DaemonError, server};
use devdev_integrations::{GitHubAdapter, MockAdapter, RepoHostAdapter};
use devdev_tasks::approval::{ApprovalPolicy, approval_channel};
use devdev_tasks::events::EventBus;
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

#[derive(Parser, Debug, Clone)]
pub struct RepoWatchArgs {
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Override poll interval (default: 60 seconds).
    #[arg(long)]
    pub poll_interval_secs: Option<u64>,

    /// Repository in `owner/repo` form.
    pub repo: String,
}

#[derive(Parser, Debug, Clone)]
pub struct RepoUnwatchArgs {
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Repository in `owner/repo` form.
    pub repo: String,
}

#[derive(Parser, Debug, Clone)]
pub struct InitArgs {
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Working directory whose `.devdev/` will receive preference
    /// files. Defaults to the current directory.
    #[arg(long)]
    pub workdir: Option<PathBuf>,

    /// End the conversation after one round-trip (used by tests).
    #[arg(long, hide = true)]
    pub one_shot: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct PreferencesListArgs {
    /// Working directory to scan for `.devdev/*.md`. Defaults to CWD.
    #[arg(long)]
    pub workdir: Option<PathBuf>,

    /// Skip the home (`~/.devdev/`) layer.
    #[arg(long)]
    pub no_home: bool,

    /// Emit JSON (`[{path, title, layer}]`) instead of a human table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct PreferencesEditArgs {
    /// Title (matched case-insensitively against `# Title` lines) or
    /// file stem.
    pub name: String,

    /// Working directory to scan. Defaults to CWD.
    #[arg(long)]
    pub workdir: Option<PathBuf>,
}

// ─── Helpers ───────────────────────────────────────────────────

fn resolve_data_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(DaemonConfig::default_data_dir)
}

/// Build the default repo-host adapter from environment.
///
/// Selection precedence: explicit `flag` > `DEVDEV_REPO_HOST_ADAPTER`
/// env var > `DEVDEV_GITHUB_ADAPTER` (legacy alias) > `"live"`.
/// `"live"` resolves to a github.com [`GitHubAdapter`] using the
/// `CredentialStore` snapshot (which already considered `GH_TOKEN`
/// and the `gh` CLI); if no credential is available we fall back to
/// the host-agnostic [`MockAdapter`] so dev/test flows still
/// progress.
///
/// Multi-host wiring (GHE, ADO) is configured per repo in
/// preferences and resolved through the daemon-side host registry;
/// this function only seeds the *default* adapter for the legacy
/// single-host code paths that haven't migrated yet.
fn select_github_adapter(
    flag: Option<&str>,
    credentials: &devdev_daemon::credentials::CredentialStore,
) -> Arc<dyn RepoHostAdapter> {
    use devdev_integrations::host::RepoHostId;

    let choice = flag
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("DEVDEV_REPO_HOST_ADAPTER").ok())
        .or_else(|| std::env::var("DEVDEV_GITHUB_ADAPTER").ok())
        .unwrap_or_else(|| "live".to_string());

    match choice.as_str() {
        "mock" => Arc::new(MockAdapter::new()),
        _ => match credentials.get(&RepoHostId::github_com()) {
            Some(cred) => Arc::new(GitHubAdapter::github_com(
                cred.token().expose().to_string(),
            )),
            None => {
                eprintln!(
                    "devdev: no github.com credential available; falling back to mock adapter (set GH_TOKEN or run `gh auth login`)"
                );
                Arc::new(MockAdapter::new())
            }
        },
    }
}

/// Helper alias used while the agent integration evolved; the actual
/// agent seam is now [`devdev_daemon::runner::RouterRunner`], built
/// per-task in [`DispatchContext::handle_task_add`].
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

    // Build the shared approval channel + credential snapshot up-
    // front so both the MCP provider (sender side, via `devdev_ask`)
    // and the dispatch IPC (receiver side, via `approval_response`)
    // can hold halves.
    let policy = ApprovalPolicy::AutoApprove;
    let (gate, handle) = approval_channel(policy, APPROVAL_TIMEOUT);
    let approval_gate = Arc::new(Mutex::new(gate));
    let approval_handle = Arc::new(Mutex::new(handle));

    // Sample credentials exactly once. After this point the store is
    // immutable; mutating env vars or `gh auth login`-ing will not
    // affect tokens served from the snapshot until the daemon
    // restarts.
    let credentials = {
        use devdev_daemon::credentials::{
            CredentialProvider, CredentialStore, EnvVarProvider, GhCliProvider,
        };
        use devdev_integrations::host::RepoHostId;

        let github = RepoHostId::github_com();
        // Env var first (deterministic, scriptable), then `gh` CLI.
        let providers: Vec<Arc<dyn CredentialProvider>> = vec![
            Arc::new(EnvVarProvider::new(github.clone(), "GH_TOKEN")),
            Arc::new(GhCliProvider::new(github.clone())),
        ];
        let store = CredentialStore::snapshot(providers).await;
        match store.get(&github) {
            Some(c) => eprintln!(
                "DevDev: github.com credential captured (source: {:?}); devdev_ask will release on approval",
                c.source()
            ),
            None => eprintln!(
                "DevDev: no github.com credential (set GH_TOKEN or run `gh auth login`)"
            ),
        }
        Arc::new(store)
    };

    let mcp_provider = Arc::new(
        DaemonToolProvider::new(Arc::clone(&tasks), Arc::clone(&fs))
            .with_ask(Arc::clone(&approval_gate), Arc::clone(&credentials)),
    );
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
    let github = select_github_adapter(args.github.as_deref(), &credentials);
    // Multi-host registry. Today we only seed the github.com slot
    // from the default adapter; preferences-driven population (one
    // entry per `[[repo]]` block) lands as a follow-up.
    let host_registry = {
        use devdev_daemon::host_registry::RepoHostRegistry;
        use devdev_integrations::host::RepoHostId;
        Arc::new(RepoHostRegistry::single(
            RepoHostId::github_com(),
            Arc::clone(&github),
        ))
    };
    let event_bus = EventBus::new();

    let ledger_path = data_dir.join("ledger.ndjson");
    let ledger = match NdjsonLedger::open(&ledger_path) {
        Ok(l) => Arc::new(l) as Arc<dyn devdev_tasks::ledger::IdempotencyLedger>,
        Err(e) => {
            return Err(anyhow!(
                "failed to open ledger at {}: {e}",
                ledger_path.display()
            ));
        }
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let ctx = Arc::new(DispatchContext::new(
        router,
        tasks,
        github,
        host_registry,
        approval_gate,
        approval_handle,
        event_bus,
        ledger,
        policy,
        credentials,
        shutdown_tx.clone(),
        fs,
    ));

    // Background coordinator: any PR event published on the bus
    // becomes a `MonitorPrTask` (created on first observation).
    let coordinator = spawn_event_coordinator(Arc::clone(&ctx), shutdown_tx.subscribe());

    // Background task scheduler: tick every 5s and poll every
    // registered task. Each task's `poll()` should self-throttle
    // against its own `poll_interval()` if it cares; today the
    // RepoWatchTask polls on every tick — adequate for dogfood,
    // but overdue for a per-task timer.
    let scheduler_ctx = Arc::clone(&ctx);
    let mut scheduler_shutdown = shutdown_tx.subscribe();
    let scheduler = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    scheduler_ctx.poll_all_tasks().await;
                }
                changed = scheduler_shutdown.changed() => {
                    if changed.is_err() || *scheduler_shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    });

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
    let _ = coordinator.await;
    let _ = scheduler.await;

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

// ─── repo watch / unwatch ──────────────────────────────────────

fn split_owner_repo(slug: &str) -> Result<(&str, &str)> {
    slug.split_once('/')
        .filter(|(o, r)| !o.is_empty() && !r.is_empty() && !r.contains('/'))
        .ok_or_else(|| anyhow!("expected `owner/repo`, got `{slug}`"))
}

pub async fn run_repo_watch(args: RepoWatchArgs) -> Result<()> {
    let (owner, repo) = split_owner_repo(&args.repo)?;
    let data_dir = resolve_data_dir(args.data_dir);
    let mut client = connect_ipc(&data_dir).await?;
    let mut params = serde_json::json!({ "owner": owner, "repo": repo });
    if let Some(secs) = args.poll_interval_secs {
        params["poll_interval_secs"] = secs.into();
    }
    let resp = client
        .request("repo/watch", params)
        .await
        .context("sending repo/watch IPC request")?;
    if let Some(err) = resp.error {
        return Err(anyhow!("repo/watch failed: {}", err.message));
    }
    let result = resp.result.unwrap_or_default();
    let task_id = result["task_id"].as_str().unwrap_or("?");
    let already = result["already_watching"].as_bool().unwrap_or(false);
    if already {
        println!("already watching {owner}/{repo} as {task_id}");
    } else {
        println!("watching {owner}/{repo} as {task_id}");
    }
    Ok(())
}

pub async fn run_repo_unwatch(args: RepoUnwatchArgs) -> Result<()> {
    let (owner, repo) = split_owner_repo(&args.repo)?;
    let data_dir = resolve_data_dir(args.data_dir);
    let mut client = connect_ipc(&data_dir).await?;
    let resp = client
        .request(
            "repo/unwatch",
            serde_json::json!({ "owner": owner, "repo": repo }),
        )
        .await
        .context("sending repo/unwatch IPC request")?;
    if let Some(err) = resp.error {
        return Err(anyhow!("repo/unwatch failed: {}", err.message));
    }
    println!("unwatched {owner}/{repo}");
    Ok(())
}

// ─── init (Vibe Check scribe) ──────────────────────────────────

const VIBE_CHECK_SYSTEM_PROMPT: &str = include_str!("vibe_check_prompt.md");
/// Cross-platform home-directory lookup. We deliberately avoid the
/// `dirs` crate to keep `devdev-cli` dependencies lean; the env
/// variables we read are the same ones `dirs` consults first.
fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(p) = std::env::var("USERPROFILE")
        && !p.is_empty()
    {
        return Some(PathBuf::from(p));
    }
    None
}
pub async fn run_init(args: InitArgs) -> Result<()> {
    let data_dir = resolve_data_dir(args.data_dir);
    let workdir = args
        .workdir
        .clone()
        .unwrap_or(std::env::current_dir().context("resolving workdir")?);

    let mut client = connect_ipc(&data_dir).await?;

    // Seed the session with the scribe persona + current workdir hint.
    let preamble = format!(
        "{VIBE_CHECK_SYSTEM_PROMPT}\n\nWorking directory: {}\n",
        workdir.display()
    );
    eprintln!("DevDev Vibe Check — interviewing you for preferences. Type a blank line to finish.");
    eprintln!(
        "(Files will be written under {})\n",
        workdir.join(".devdev").display()
    );

    let opening = send_one(
        &mut client,
        format!("{preamble}\n\nGreet the user and ask the first question."),
    )
    .await?;
    println!("scribe> {opening}\n");

    if args.one_shot {
        return Ok(());
    }

    let stdin = std::io::stdin();
    loop {
        eprint!("you> ");
        use std::io::Write;
        std::io::stderr().flush().ok();
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let reply = send_one(&mut client, line.to_string()).await?;
        println!("scribe> {reply}\n");
    }

    eprintln!("\nVibe Check complete. Inspect with `devdev preferences list`.");
    Ok(())
}

async fn send_one(client: &mut devdev_daemon::ipc::IpcClient, text: String) -> Result<String> {
    let resp = client
        .request("send", serde_json::json!({ "text": text }))
        .await
        .context("sending IPC send request")?;
    if let Some(err) = resp.error {
        return Err(anyhow!("send failed: {}", err.message));
    }
    let result = resp
        .result
        .ok_or_else(|| anyhow!("daemon returned neither result nor error"))?;
    Ok(result
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

// ─── preferences list / edit ───────────────────────────────────

pub fn run_preferences_list(args: PreferencesListArgs) -> Result<()> {
    let workdir = args
        .workdir
        .clone()
        .unwrap_or(std::env::current_dir().context("resolving workdir")?);
    let home = if args.no_home { None } else { home_dir() };
    let files = crate::preferences::discover(&workdir, home.as_deref())
        .context("discovering preferences")?;

    if args.json {
        let json: Vec<_> = files
            .iter()
            .map(|f| {
                serde_json::json!({
                    "path": f.path,
                    "title": f.title,
                    "layer": format!("{:?}", f.layer),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
        return Ok(());
    }

    if files.is_empty() {
        println!("(no preferences found — run `devdev init` to create some)");
        return Ok(());
    }
    for f in &files {
        println!("[{:?}] {} — {}", f.layer, f.title, f.path.display());
    }
    Ok(())
}

pub fn run_preferences_edit(args: PreferencesEditArgs) -> Result<()> {
    let workdir = args
        .workdir
        .clone()
        .unwrap_or(std::env::current_dir().context("resolving workdir")?);
    let files = crate::preferences::discover(&workdir, home_dir().as_deref())
        .context("discovering preferences")?;
    let needle = args.name.to_lowercase();
    let hit = files.iter().find(|f| {
        f.title.to_lowercase() == needle
            || f.path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase() == needle)
                .unwrap_or(false)
    });
    let target = match hit {
        Some(f) => f.path.clone(),
        None => {
            // Default to a new file under repo-local .devdev/.
            let safe: String = args
                .name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            workdir.join(".devdev").join(format!("{safe}.md"))
        }
    };
    if let Some(p) = target.parent() {
        std::fs::create_dir_all(p).ok();
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
        if cfg!(windows) {
            "notepad".into()
        } else {
            "nano".into()
        }
    });
    let status = std::process::Command::new(&editor)
        .arg(&target)
        .status()
        .with_context(|| format!("launching $EDITOR ({editor})"))?;
    if !status.success() {
        return Err(anyhow!("{editor} exited with {}", status));
    }
    Ok(())
}
