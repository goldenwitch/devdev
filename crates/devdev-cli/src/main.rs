//! `devdev` binary — daemon lifecycle subcommands.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use devdev_cli::daemon_cli::{
    DownArgs, InitArgs, PreferencesEditArgs, PreferencesListArgs, RepoUnwatchArgs, RepoWatchArgs,
    SendArgs, StatusArgs, UpArgs, run_down, run_init, run_preferences_edit, run_preferences_list,
    run_repo_unwatch, run_repo_watch, run_send, run_status, run_up,
};

#[derive(Parser, Debug)]
#[command(name = "devdev", version, about = "DevDev daemon CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start the DevDev daemon (foreground).
    Up(UpArgs),
    /// Ask a running daemon to shut down.
    Down(DownArgs),
    /// Send a one-shot prompt to the running daemon.
    Send(SendArgs),
    /// Print daemon status.
    Status(StatusArgs),
    /// Run the Vibe Check scribe to record `.devdev/*.md` preferences.
    Init(InitArgs),
    /// Repository watch operations.
    #[command(subcommand)]
    Repo(RepoCommand),
    /// Inspect or edit `.devdev/*.md` preference files.
    #[command(subcommand)]
    Preferences(PreferencesCommand),
}

#[derive(Subcommand, Debug)]
enum RepoCommand {
    /// Start polling a `<owner>/<repo>` for PR events.
    Watch(RepoWatchArgs),
    /// Stop polling a `<owner>/<repo>`.
    Unwatch(RepoUnwatchArgs),
}

#[derive(Subcommand, Debug)]
enum PreferencesCommand {
    /// List discovered preference files.
    List(PreferencesListArgs),
    /// Open `$EDITOR` on a preference file (creates one if absent).
    Edit(PreferencesEditArgs),
}

fn main() -> ExitCode {
    // Initialize tracing subscriber once at startup. Respects RUST_LOG;
    // defaults to info level if not set.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();

    let cli = Cli::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("devdev: could not build tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };
    let result = match cli.command {
        Command::Up(args) => rt.block_on(run_up(args)),
        Command::Down(args) => rt.block_on(run_down(args)),
        Command::Send(args) => rt.block_on(run_send(args)),
        Command::Status(args) => rt.block_on(run_status(args)),
        Command::Init(args) => rt.block_on(run_init(args)),
        Command::Repo(RepoCommand::Watch(args)) => rt.block_on(run_repo_watch(args)),
        Command::Repo(RepoCommand::Unwatch(args)) => rt.block_on(run_repo_unwatch(args)),
        Command::Preferences(PreferencesCommand::List(args)) => run_preferences_list(args),
        Command::Preferences(PreferencesCommand::Edit(args)) => run_preferences_edit(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("devdev: {e}");
            ExitCode::from(1)
        }
    }
}
