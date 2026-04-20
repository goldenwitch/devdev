//! Top-level `evaluate()` orchestrator for cap 13.
//!
//! Composes every lower-level crate:
//!
//! 1. Build [`MemFs`] with the configured cap.
//! 2. Load the host path into the VFS (two-pass `load_repo`).
//! 3. Try to promote the loaded `.git` into a `VirtualRepo`; fall back
//!    to [`StubGit`] on miss.
//! 4. Wire a `SandboxHandler` with a `FnOnce` closure that builds the
//!    `!Send` `ShellSession` on the worker thread.
//! 5. Install a `FanoutTraceLogger` so both sinks ([`VerdictCollector`],
//!    [`ToolCallCollector`]) see every hook event.
//! 6. `AcpClient::connect_*` → `initialize` → `authenticate` (if
//!    needed) → `new_session` → `prompt`, all inside a
//!    `tokio::time::timeout(session_timeout, …)`.
//! 7. Drain the collectors into an `EvalResult`. Always call
//!    `client.shutdown()`, even on error.

use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use devdev_acp::{
    AcpClient, AcpClientConfig, AcpError, HandlerConfig, SandboxHandler,
};
use devdev_acp::trace::TraceLogger;
use devdev_acp::types::{
    NewSessionParams, PromptContent, PromptParams,
};
use devdev_git::{VirtualGit, VirtualRepo};
use devdev_shell::ShellSession;
use devdev_vfs::{LoadError, LoadOptions, MemFs, load_repo};
use devdev_wasm::{ToolEngine, WasmToolRegistry};

use crate::config::{
    EvalConfig, EvalContext, EvalError, EvalResult, RepoStats, Transport,
};
use crate::prompt::format_prompt;
use crate::stub_git::{OwnedVirtualGit, StubGit};
use crate::verdict::{FanoutTraceLogger, ToolCallCollector, VerdictCollector};

/// Run one evaluation. Returns once the agent has stopped or an error
/// has been raised. The VFS is dropped before this future resolves —
/// the `Arc<Mutex<MemFs>>` held by the caller should be the only
/// remaining strong reference.
pub async fn evaluate(
    repo_path: &Path,
    config: EvalConfig,
    context: EvalContext,
    transport: Transport,
) -> Result<EvalResult, EvalError> {
    let started = Instant::now();

    // ── 1-2. VFS + load ────────────────────────────────────────────
    let vfs = Arc::new(StdMutex::new(MemFs::with_limit(config.workspace_limit)));
    let repo_stats = load_repo_into_vfs(&vfs, repo_path, config.include_git, config.workspace_limit)?;

    // ── 3. .git → VirtualRepo | StubGit ────────────────────────────
    let (owned_repo, is_git_repo) = if config.include_git {
        try_load_virtual_repo(&vfs)
    } else {
        (None, false)
    };

    // ── 4-5. SandboxHandler + Fanout trace ─────────────────────────
    let verdict_collector = Arc::new(VerdictCollector::new());
    let tool_collector = Arc::new(ToolCallCollector::new());
    let fanout: Arc<dyn TraceLogger> = Arc::new(FanoutTraceLogger::new(vec![
        verdict_collector.clone(),
        tool_collector.clone(),
    ]));

    let handler_config = HandlerConfig {
        command_timeout: config.command_timeout,
        ..HandlerConfig::default()
    };

    let vfs_for_shell = vfs.clone();
    let handler_vfs = vfs.clone();

    // Build the shell on the worker thread: ToolEngine + VirtualGit are
    // constructed inside the closure because neither libgit2 nor the
    // wasmtime engine are `Send`.
    let build_shell = move || -> ShellSession {
        let tools: Arc<dyn ToolEngine> = match WasmToolRegistry::new() {
            Ok(r) => Arc::new(r),
            // A failed wasmtime engine init is fatal for the shell
            // worker, but we can't surface it from this closure — fall
            // back to a wasm-less shell by panicking here would drop
            // the whole handler. Instead, panic loudly: cap 04's
            // registry init only fails on host-level misconfiguration,
            // which is not a recoverable condition.
            Err(e) => panic!("wasm registry init failed: {e}"),
        };
        let git: Arc<StdMutex<dyn VirtualGit>> = match owned_repo {
            Some(repo) => Arc::new(StdMutex::new(OwnedVirtualGit::new(repo))),
            None => Arc::new(StdMutex::new(StubGit)),
        };
        ShellSession::new(vfs_for_shell, tools, git)
    };

    let handler = Arc::new(
        SandboxHandler::with_config(build_shell, handler_vfs, handler_config)
            .with_trace(fanout),
    );

    // ── 6. ACP connect + negotiation ───────────────────────────────
    let acp_config = AcpClientConfig {
        idle_timeout: config.cli_hang_timeout,
        ..AcpClientConfig::default()
    };
    let client = match transport {
        Transport::SpawnProcess { program, args } => {
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            AcpClient::connect_process(&program, &arg_refs, handler, acp_config).await?
        }
        Transport::Connected { reader, writer } => {
            AcpClient::connect_transport(reader, writer, handler, acp_config).await?
        }
    };

    // Wrap the whole negotiation+prompt in the session timeout.
    let prompt_text = format_prompt_from_context(&context, repo_path);
    let orchestration = run_conversation(&client, prompt_text);

    let outcome =
        match tokio::time::timeout(config.session_timeout, orchestration).await {
            Ok(result) => result,
            Err(_) => Err(EvalError::Timeout(config.session_timeout)),
        };

    // Drain collectors before shutdown — the trace hooks have already
    // fired and pushed into the mutexes; shutdown only tears down I/O
    // tasks. We drain here to avoid holding the client reference past
    // `shutdown().await`.
    let verdict = verdict_collector.take();
    let tool_calls = tool_collector.take();

    // Always shut down — even on error — to kill the subprocess and
    // free the trace handles (they're cloned inside the handler).
    let _ = client.shutdown().await;

    let stop_reason = outcome?;

    Ok(EvalResult {
        verdict,
        stop_reason,
        tool_calls,
        duration: started.elapsed(),
        is_git_repo,
        repo_stats,
    })
}

/// Step 1-2: build MemFs, run `load_repo`, translate `ExceedsLimit` to
/// our typed variant.
fn load_repo_into_vfs(
    vfs: &Arc<StdMutex<MemFs>>,
    repo_path: &Path,
    include_git: bool,
    limit: u64,
) -> Result<RepoStats, EvalError> {
    let options = LoadOptions {
        include_git,
        progress: None,
    };
    let mut guard = vfs.lock().expect("vfs mutex poisoned");
    let bytes = match load_repo(repo_path, &mut guard, &options) {
        Ok(n) => n,
        Err(LoadError::ExceedsLimit { total_bytes, .. }) => {
            return Err(EvalError::RepoTooLarge {
                total: total_bytes,
                limit,
            });
        }
        Err(e) => return Err(EvalError::VfsLoad(e)),
    };
    // `file_count` isn't tracked on MemFs; walk the tree to count
    // regular files. Cheap: the tree is already in memory.
    use devdev_vfs::types::Node;
    let files = guard
        .tree()
        .values()
        .filter(|node| matches!(node, Node::File { .. }))
        .count() as u64;
    Ok(RepoStats { files, bytes })
}

/// Step 3: best-effort `VirtualRepo::from_vfs("/")`. Returns
/// `(None, false)` if no `.git` was loaded.
fn try_load_virtual_repo(vfs: &Arc<StdMutex<MemFs>>) -> (Option<VirtualRepo>, bool) {
    let guard = vfs.lock().expect("vfs mutex poisoned");
    match VirtualRepo::from_vfs(&guard, "/") {
        Ok(repo) => (Some(repo), true),
        Err(_) => (None, false),
    }
}

/// Derive a `repo_name` from the host path and render the prompt.
fn format_prompt_from_context(ctx: &EvalContext, repo_path: &Path) -> String {
    let repo_name = repo_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");
    format_prompt(ctx, repo_name)
}

/// Steps 8-12: initialize → authenticate → new_session → prompt.
/// Returns the stop-reason string on success.
async fn run_conversation(
    client: &AcpClient,
    prompt_text: String,
) -> Result<String, EvalError> {
    let init = client.initialize().await.map_err(map_acp)?;

    if !init.auth_methods.is_empty() {
        let advertised: Vec<String> =
            init.auth_methods.iter().map(|m| m.kind.clone()).collect();
        match client.authenticate(&advertised).await {
            Ok(_) => {}
            Err(AcpError::Rpc { message, .. }) => {
                return Err(EvalError::AuthenticationFailed(message));
            }
            Err(AcpError::NoAuth) => {
                return Err(EvalError::AuthenticationFailed(
                    "no usable authentication strategy".into(),
                ));
            }
            Err(e) => return Err(map_acp(e)),
        }
    }

    let sess = client
        .new_session(NewSessionParams {
            cwd: "/".into(),
            mcp_servers: vec![],
        })
        .await
        .map_err(map_acp)?;

    let prompt = PromptParams {
        session_id: sess.session_id,
        prompt: vec![PromptContent::Text { text: prompt_text }],
    };
    let result = client.prompt(prompt).await.map_err(map_acp)?;
    Ok(result.stop_reason.as_str().to_owned())
}

/// Map any `AcpError::AgentDisconnected` (from our sentinel path) to
/// [`EvalError::CliCrashed`]; everything else bubbles as `Acp(_)`.
fn map_acp(e: AcpError) -> EvalError {
    match e {
        AcpError::AgentDisconnected | AcpError::BrokenPipe | AcpError::SubprocessCrashed(_) => {
            EvalError::CliCrashed
        }
        other => EvalError::Acp(other),
    }
}
