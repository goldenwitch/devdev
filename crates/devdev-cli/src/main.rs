//! `devdev` binary — thin argparse + output wrapper over
//! [`devdev_cli::evaluate`].
//!
//! Option surface mirrors `capabilities/14-test-harness.md`. Every
//! non-obvious concern (VFS loading, agent orchestration, prompt
//! shape) lives in the library; this file only translates
//! `--flag value` into `EvalConfig` / `EvalContext` and formats the
//! result.
//!
//! Exit codes:
//!
//! | 0 | evaluation completed, verdict printed            |
//! | 1 | evaluation failed (see stderr)                   |
//! | 2 | invalid CLI arguments — delegated to `clap`      |

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use devdev_cli::daemon_cli::{
    run_down, run_send, run_status, run_up, DownArgs, SendArgs, StatusArgs, UpArgs,
};
use devdev_cli::{
    DEFAULT_CLI_HANG_TIMEOUT, DEFAULT_COMMAND_TIMEOUT, DEFAULT_SESSION_TIMEOUT,
    DEFAULT_WORKSPACE_LIMIT, EvalConfig, EvalContext, EvalError, PreferenceFile, Transport,
    emit_startup_banner, evaluate, init_tracing, render_human, render_json,
};

#[derive(Parser, Debug)]
#[command(name = "devdev", version, about = "DevDev sandboxed agent evaluator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run one evaluation against a local repo.
    Eval(EvalArgs),
    /// Start the DevDev daemon (foreground).
    Up(UpArgs),
    /// Ask a running daemon to shut down.
    Down(DownArgs),
    /// Send a one-shot prompt to the running daemon.
    Send(SendArgs),
    /// Print daemon status.
    Status(StatusArgs),
}

#[derive(Parser, Debug)]
struct EvalArgs {
    /// Path to the local repository to evaluate.
    #[arg(long)]
    repo: PathBuf,

    /// Evaluation task description (one line).
    #[arg(long)]
    task: String,

    /// Optional unified-diff file to include in the prompt.
    #[arg(long)]
    diff: Option<PathBuf>,

    /// Directory of `*.md` preference files rendered into the prompt,
    /// in filename order.
    #[arg(long)]
    preferences: Option<PathBuf>,

    /// VFS memory cap, in bytes.
    #[arg(long, default_value_t = DEFAULT_WORKSPACE_LIMIT)]
    workspace_limit: u64,

    /// Whole-evaluation wall-clock budget, in seconds.
    #[arg(long, default_value_t = DEFAULT_SESSION_TIMEOUT.as_secs())]
    timeout: u64,

    /// Per-command wall-clock cap inside the sandbox, in seconds.
    #[arg(long, default_value_t = DEFAULT_COMMAND_TIMEOUT.as_secs())]
    command_timeout: u64,

    /// Idle-silence timeout between agent messages, in seconds.
    #[arg(long, default_value_t = DEFAULT_CLI_HANG_TIMEOUT.as_secs())]
    cli_hang_timeout: u64,

    /// Emit the result as a single-line JSON object on stdout.
    #[arg(long)]
    json: bool,

    /// Enable `DEBUG`-level tracing on stderr.
    #[arg(long)]
    verbose: bool,

    /// Also write `TRACE` output to the given file.
    #[arg(long)]
    trace_file: Option<PathBuf>,

    /// Override the agent invocation. Rarely needed.
    #[arg(long, default_value = "copilot")]
    agent_program: String,

    /// Extra args passed to the agent binary. Defaults to
    /// `--acp --stdio`.
    #[arg(long, num_args = 0..)]
    agent_arg: Vec<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Eval(args) => match run_eval(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(exit) => exit,
        },
        Command::Up(args) => run_async(run_up(args)),
        Command::Down(args) => run_async(run_down(args)),
        Command::Send(args) => run_async(run_send(args)),
        Command::Status(args) => run_async(run_status(args)),
    }
}

/// Drive an async command to completion on a fresh tokio runtime,
/// mapping `anyhow::Error` to exit code 1 with a stderr diagnostic.
fn run_async<F>(fut: F) -> ExitCode
where
    F: std::future::Future<Output = anyhow::Result<()>>,
{
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("devdev: could not build tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };
    match rt.block_on(fut) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("devdev: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_eval(args: EvalArgs) -> Result<(), ExitCode> {
    let _trace_guard = init_tracing(args.verbose, args.trace_file.as_deref());
    emit_startup_banner(1, "devdev-cli");

    let config = EvalConfig {
        workspace_limit: args.workspace_limit,
        command_timeout: Duration::from_secs(args.command_timeout),
        session_timeout: Duration::from_secs(args.timeout),
        cli_hang_timeout: Duration::from_secs(args.cli_hang_timeout),
        include_git: true,
    };

    let diff = match args.diff.as_deref() {
        Some(p) => Some(std::fs::read_to_string(p).map_err(|e| {
            eprintln!("devdev: could not read --diff {}: {e}", p.display());
            ExitCode::from(1)
        })?),
        None => None,
    };

    let preferences = match args.preferences.as_deref() {
        Some(dir) => load_preferences(dir).map_err(|e| {
            eprintln!(
                "devdev: could not load preferences from {}: {e}",
                dir.display()
            );
            ExitCode::from(1)
        })?,
        None => Vec::new(),
    };

    let context = EvalContext {
        task: args.task,
        diff,
        preferences,
        focus_paths: Vec::new(),
    };

    let transport = build_transport(&args.agent_program, &args.agent_arg);

    let rt = tokio::runtime::Runtime::new().map_err(|e| {
        eprintln!("devdev: could not build tokio runtime: {e}");
        ExitCode::from(1)
    })?;

    let result = rt.block_on(async move {
        evaluate(&args.repo, config, context, transport).await
    });

    match result {
        Ok(r) => {
            let text = if args.json {
                render_json(&r)
            } else {
                render_human(&r)
            };
            print!("{text}");
            Ok(())
        }
        Err(e) => {
            report_error(&e);
            Err(ExitCode::from(1))
        }
    }
}

/// Canonicalise the argv pair into a [`Transport`]. An empty
/// `agent_arg` list is sugar for the stock `--acp --stdio` invocation
/// of `copilot`.
fn build_transport(program: &str, args: &[String]) -> Transport {
    if program == "copilot" && args.is_empty() {
        return Transport::copilot();
    }
    let args = if args.is_empty() {
        vec!["--acp".into(), "--stdio".into()]
    } else {
        args.to_vec()
    };
    Transport::SpawnProcess {
        program: program.to_owned(),
        args,
    }
}

/// Read every `*.md` file in `dir` (non-recursive) into a
/// [`PreferenceFile`]. Sorted by filename so runs are deterministic.
fn load_preferences(dir: &std::path::Path) -> std::io::Result<Vec<PreferenceFile>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("preference")
            .to_owned();
        let content = std::fs::read_to_string(&path)?;
        out.push(PreferenceFile { name, content });
    }
    Ok(out)
}

/// One-line diagnostic to stderr summarising the failure.
fn report_error(err: &EvalError) {
    match err {
        EvalError::RepoTooLarge { total, limit } => {
            eprintln!("devdev: repo too large: {total} bytes (limit {limit})");
        }
        EvalError::Timeout(d) => {
            eprintln!("devdev: evaluation timed out after {:.1}s", d.as_secs_f64());
        }
        EvalError::AuthenticationFailed(msg) => {
            eprintln!("devdev: authentication failed: {msg}");
        }
        EvalError::CliCrashed => {
            eprintln!("devdev: agent subprocess exited unexpectedly");
        }
        EvalError::VfsLoad(e) => {
            eprintln!("devdev: vfs load failed: {e}");
        }
        EvalError::Acp(e) => {
            eprintln!("devdev: acp error: {e}");
        }
        EvalError::Io(e) => {
            eprintln!("devdev: io error: {e}");
        }
    }
}

