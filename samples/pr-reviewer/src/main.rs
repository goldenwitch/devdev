//! # `pr-reviewer` — canonical end-to-end DevDev sample
//!
//! Polls a GitHub repository for open PRs and asks a Copilot ACP agent
//! to review each one as it appears or updates. Reviews print to
//! stdout. **Read-only** — never posts to the PR.
//!
//! The point of this sample is to exercise the **library surface** of
//! every DevDev crate without going through the daemon or its IPC:
//!
//! | Library         | What this sample uses                     |
//! |-----------------|-------------------------------------------|
//! | `devdev-acp`    | `AcpClient::connect_process`, prompt loop |
//! | `devdev-cli`    | `agent_command::prepare` (resolve+rewrite)|
//! | `devdev-daemon` | `CredentialStore` + provider chain        |
//! | `devdev-integrations` | `GitHubAdapter`, `pr_state_hash`    |
//!
//! If anything below ends up doing the daemon's work by hand, that's a
//! signal the daemon is hoarding logic that should live in a library
//! crate. Keep this file boring on purpose.
//!
//! ## Usage
//!
//! ```text
//! cargo run -p pr-reviewer -- goldenwitch/devdev
//! cargo run -p pr-reviewer -- goldenwitch/devdev --poll-secs 30 --once
//! ```
//!
//! Authentication: same providers the daemon uses — `GH_TOKEN` env var
//! or `gh auth login`. No new config surface.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use clap::Parser;
use devdev_acp::handler::{AcpHandler, HandlerResult};
use devdev_acp::types::{
    CreateTerminalParams, CreateTerminalResult, KillTerminalParams, NewSessionParams,
    PermissionRequestParams, PermissionResponse, PromptContent, PromptParams, ReadTextFileParams,
    ReadTextFileResult, ReleaseTerminalParams, SessionUpdate, SessionUpdateParams,
    TerminalOutputParams, TerminalOutputResult, WaitForExitParams, WaitForExitResult,
    WriteTextFileParams,
};
use devdev_acp::{AcpClient, AcpClientConfig, AcpError};
use devdev_cli::agent_command;
use devdev_daemon::credentials::{
    CredentialProvider, CredentialStore, EnvVarProvider, GhCliProvider,
};
use devdev_integrations::{GitHubAdapter, PullRequest, RepoHostAdapter, RepoHostId, pr_state_hash};
use tokio::sync::Mutex;

#[derive(Parser, Debug)]
#[command(about = "Sample: poll a GitHub repo and have a Copilot agent review every PR.")]
struct Args {
    /// Repository in `owner/repo` form. github.com only.
    repo: String,

    /// Poll interval in seconds.
    #[arg(long, default_value_t = 60)]
    poll_secs: u64,

    /// Review every currently-open PR once and exit.
    #[arg(long)]
    once: bool,

    /// Agent program to spawn. Defaults to `copilot`; resolved via
    /// `agent_command::prepare`, which handles PATHEXT on Windows and
    /// the Copilot SEA-launcher rewrite.
    #[arg(long, default_value = "copilot")]
    agent_program: String,

    /// Extra arguments to pass to the agent. Defaults match the
    /// daemon: `--acp --allow-all-tools` (Copilot CLI's ACP/NDJSON
    /// mode with non-interactive tool permissions).
    #[arg(long, num_args = 1.., default_values_t = ["--acp".to_string(), "--allow-all-tools".to_string()])]
    agent_arg: Vec<String>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let (owner, repo) = parse_repo(&args.repo)?;

    // 1. Build a `CredentialStore` exactly the way the daemon does:
    //    env-var first, then `gh auth login`. Same provider chain →
    //    same precedence → no surprises when users move from this
    //    sample to `devdev up`.
    let host_id = RepoHostId::github_com();
    let providers: Vec<Arc<dyn CredentialProvider>> = vec![
        Arc::new(EnvVarProvider::new(host_id.clone(), "GH_TOKEN")),
        Arc::new(GhCliProvider::new(host_id.clone())),
    ];
    let credentials = CredentialStore::snapshot(providers).await;
    let cred = credentials.get(&host_id).ok_or_else(|| {
        anyhow!(
            "no github.com credential found. Set GH_TOKEN or run `gh auth login` and try again."
        )
    })?;
    tracing::info!(source = ?cred.source(), "github.com credential captured");

    // 2. Build the GitHub adapter. The same `GitHubAdapter` the daemon
    //    uses — no fork, no parallel implementation.
    let github: Arc<dyn RepoHostAdapter> = Arc::new(GitHubAdapter::github_com(
        cred.token().expose().to_string(),
    ));

    // 3. Spawn the agent. `agent_command::prepare` is the one
    //    canonical entry-point that handles PATHEXT on Windows and
    //    the Copilot SEA-launcher rewrite. Don't call `Command::new`
    //    directly — that's how this PR's first dogfood found bug.
    let (program, agent_args) = agent_command::prepare(&args.agent_program, &args.agent_arg);
    let handler = Arc::new(ChunkCollector::default());
    let argv: Vec<&str> = agent_args.iter().map(String::as_str).collect();
    let acp_config = AcpClientConfig {
        idle_timeout: Duration::from_secs(300),
        ..AcpClientConfig::default()
    };
    let client = AcpClient::connect_process(
        &program,
        &argv,
        handler.clone() as Arc<dyn AcpHandler>,
        acp_config,
    )
    .await
    .context("spawn ACP agent")?;
    let init = client.initialize().await.context("initialize ACP agent")?;
    let methods: Vec<String> = init.auth_methods.iter().map(|m| m.id.clone()).collect();
    if !methods.is_empty() {
        match client.authenticate(&methods).await {
            Ok(_) | Err(AcpError::NoAuth) => {}
            Err(e) => return Err(anyhow!("authenticate ACP agent: {e}")),
        }
    }

    // 4. Loop: poll, review, sleep. Track each PR's `pr_state_hash`
    //    so we only re-review on real changes (head-sha bump or
    //    metadata edit). Mirrors the daemon's ledger dedup.
    let mut seen: HashMap<u64, String> = HashMap::new();
    let cwd = std::env::current_dir()?.to_string_lossy().into_owned();
    loop {
        let prs = github
            .list_open_prs(&owner, &repo)
            .await
            .with_context(|| format!("list open PRs for {owner}/{repo}"))?;
        tracing::info!(count = prs.len(), "polled open PRs");

        for pr in &prs {
            let hash = pr_state_hash(pr);
            if seen.get(&pr.number) == Some(&hash) {
                continue;
            }
            seen.insert(pr.number, hash);
            if let Err(e) = review_pr(&client, &handler, &cwd, &owner, &repo, pr).await {
                tracing::warn!(pr = pr.number, error = %e, "review failed; will retry on next poll");
            }
        }

        if args.once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(args.poll_secs)).await;
    }
}

/// Open one ACP session per PR review and stream the agent's reply.
async fn review_pr(
    client: &AcpClient,
    handler: &Arc<ChunkCollector>,
    cwd: &str,
    owner: &str,
    repo: &str,
    pr: &PullRequest,
) -> Result<()> {
    println!();
    println!("─── PR {owner}/{repo}#{} — {} ───", pr.number, pr.title);

    let session = client
        .new_session(NewSessionParams {
            cwd: cwd.to_string(),
            mcp_servers: Vec::new(),
        })
        .await
        .context("session/new")?;

    handler.start_session(session.session_id.clone()).await;

    let prompt = format!(
        "Review pull request #{pr_num} in {owner}/{repo}. Use the `gh` CLI to fetch the diff and \
         metadata. Identify any substantive correctness, security, or design issues. Skip style \
         nits. Be terse — one paragraph plus a bulleted issue list, or 'no significant issues' if \
         none. Do not post anything to the PR; this is a read-only review.",
        pr_num = pr.number,
    );
    let result = client
        .prompt(PromptParams {
            session_id: session.session_id.clone(),
            prompt: vec![PromptContent::Text { text: prompt }],
        })
        .await
        .context("session/prompt")?;
    tracing::debug!(stop_reason = ?result.stop_reason, "prompt completed");

    let reply = handler.finish_session(&session.session_id).await;
    if reply.trim().is_empty() {
        println!("(agent returned an empty reply)");
    } else {
        println!("{reply}");
    }
    Ok(())
}

/// Minimal `AcpHandler` that collects every `agentMessageChunk` text
/// fragment into a per-session buffer. All tool/permission/fs hooks
/// reject — Copilot CLI runs its own tools when launched with
/// `--allow-all-tools`, so we never see those callbacks.
#[derive(Default)]
struct ChunkCollector {
    buffers: Mutex<HashMap<String, String>>,
}

impl ChunkCollector {
    async fn start_session(&self, session_id: String) {
        self.buffers.lock().await.insert(session_id, String::new());
    }

    async fn finish_session(&self, session_id: &str) -> String {
        self.buffers
            .lock()
            .await
            .remove(session_id)
            .unwrap_or_default()
    }
}

#[async_trait]
impl AcpHandler for ChunkCollector {
    async fn on_session_update(&self, params: SessionUpdateParams) {
        if let SessionUpdate::AgentMessageChunk { content } = params.update {
            let mut buffers = self.buffers.lock().await;
            if let Some(buf) = buffers.get_mut(&params.session_id) {
                buf.push_str(&content.text);
            }
        }
    }

    async fn on_permission_request(
        &self,
        _params: PermissionRequestParams,
    ) -> HandlerResult<PermissionResponse> {
        Err(unsupported("session/request_permission"))
    }
    async fn on_terminal_create(
        &self,
        _params: CreateTerminalParams,
    ) -> HandlerResult<CreateTerminalResult> {
        Err(unsupported("terminal/create"))
    }
    async fn on_terminal_output(
        &self,
        _params: TerminalOutputParams,
    ) -> HandlerResult<TerminalOutputResult> {
        Err(unsupported("terminal/output"))
    }
    async fn on_terminal_wait(
        &self,
        _params: WaitForExitParams,
    ) -> HandlerResult<WaitForExitResult> {
        Err(unsupported("terminal/wait_for_exit"))
    }
    async fn on_terminal_kill(&self, _params: KillTerminalParams) -> HandlerResult<()> {
        Err(unsupported("terminal/kill"))
    }
    async fn on_terminal_release(&self, _params: ReleaseTerminalParams) -> HandlerResult<()> {
        Err(unsupported("terminal/release"))
    }
    async fn on_fs_read(&self, _params: ReadTextFileParams) -> HandlerResult<ReadTextFileResult> {
        Err(unsupported("fs/read_text_file"))
    }
    async fn on_fs_write(&self, _params: WriteTextFileParams) -> HandlerResult<()> {
        Err(unsupported("fs/write_text_file"))
    }
}

fn unsupported(method: &str) -> devdev_acp::protocol::RpcError {
    devdev_acp::protocol::RpcError {
        code: devdev_acp::protocol::error_codes::METHOD_NOT_FOUND,
        message: format!("{method} not supported by pr-reviewer sample"),
        data: None,
    }
}

fn parse_repo(s: &str) -> Result<(String, String)> {
    let mut parts = s.splitn(2, '/');
    let owner = parts.next().filter(|s| !s.is_empty());
    let repo = parts.next().filter(|s| !s.is_empty());
    match (owner, repo) {
        (Some(o), Some(r)) => Ok((o.to_string(), r.to_string())),
        _ => bail!("expected `owner/repo`, got `{s}`"),
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pr_reviewer=info,warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
