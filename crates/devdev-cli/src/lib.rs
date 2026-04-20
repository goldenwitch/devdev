//! Sandbox-orchestration library backing the `devdev` binary.
//!
//! Exposes [`evaluate`] — the top-level entry point that loads a host
//! repository into the VFS, spins up a sandboxed ACP agent, drives one
//! prompt turn, and returns a verdict plus tool-call log.
//!
//! The binary (`src/main.rs`) is a thin argparse wrapper over this
//! library. Tests drive `evaluate` directly via a scripted fake agent
//! over `tokio::io::duplex`.

pub mod config;
pub mod eval;
pub mod output;
pub mod prompt;
pub mod stub_git;
pub mod tracing_setup;
pub mod verdict;

pub use config::{
    DEFAULT_CLI_HANG_TIMEOUT, DEFAULT_COMMAND_TIMEOUT, DEFAULT_SESSION_TIMEOUT,
    DEFAULT_WORKSPACE_LIMIT, EvalConfig, EvalContext, EvalError, EvalResult, PreferenceFile,
    RepoStats, ToolCallLog, Transport,
};
pub use eval::evaluate;
pub use output::{render_human, render_json};
pub use prompt::format_prompt;
pub use stub_git::{OwnedVirtualGit, StubGit};
pub use tracing_setup::{TracingGuard, emit_startup_banner, init as init_tracing};
pub use verdict::{FanoutTraceLogger, ToolCallCollector, VerdictCollector};
