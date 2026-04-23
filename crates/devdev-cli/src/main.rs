//! `devdev` binary — daemon lifecycle subcommands.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use devdev_cli::daemon_cli::{
    run_down, run_send, run_status, run_up, DownArgs, SendArgs, StatusArgs, UpArgs,
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
}

fn main() -> ExitCode {
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
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("devdev: {e}");
            ExitCode::from(1)
        }
    }
}
