//! Public types for [`crate::evaluate`].
//!
//! Kept in their own module so tests and downstream crates can
//! construct inputs without pulling in the orchestration code.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

/// Default VFS memory cap: 2 GiB. Large enough for any real repo, small
/// enough to protect a 16 GB dev box from a pathological input.
pub const DEFAULT_WORKSPACE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// Default per-command wall-clock limit inside the sandbox.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Default whole-evaluation wall-clock budget.
pub const DEFAULT_SESSION_TIMEOUT: Duration = Duration::from_secs(600);

/// Default idle-silence window before the ACP client kills the agent.
pub const DEFAULT_CLI_HANG_TIMEOUT: Duration = Duration::from_secs(60);

/// Tunable knobs for one evaluation. All fields have sensible defaults
/// via [`EvalConfig::default`].
#[derive(Debug, Clone)]
pub struct EvalConfig {
    /// VFS memory cap, in bytes. Reject repos larger than this before
    /// spawning the agent.
    pub workspace_limit: u64,
    /// Per-command timeout inside the sandbox (plumbed into
    /// `SandboxHandler::HandlerConfig`).
    pub command_timeout: Duration,
    /// Outer wall-clock budget for the whole `evaluate` call.
    pub session_timeout: Duration,
    /// Idle-silence budget enforced by `AcpClient` between messages.
    pub cli_hang_timeout: Duration,
    /// Whether to load the repo's `.git` directory into the VFS. When
    /// `false` the evaluator installs [`crate::stub_git::StubGit`].
    pub include_git: bool,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            workspace_limit: DEFAULT_WORKSPACE_LIMIT,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            session_timeout: DEFAULT_SESSION_TIMEOUT,
            cli_hang_timeout: DEFAULT_CLI_HANG_TIMEOUT,
            include_git: true,
        }
    }
}

/// How `evaluate()` reaches the agent.
///
/// `SpawnProcess` is the production path; `Connected` is the testing /
/// embedding path that skips `Command::spawn` and hands a pre-built
/// NDJSON pipe pair straight to `AcpClient::connect_transport`.
pub enum Transport {
    SpawnProcess {
        program: String,
        args: Vec<String>,
    },
    Connected {
        reader: Box<dyn AsyncRead + Send + Unpin>,
        writer: Box<dyn AsyncWrite + Send + Unpin>,
    },
}

impl Transport {
    /// Canonical production transport: `copilot --acp --stdio`.
    pub fn copilot() -> Self {
        Self::SpawnProcess {
            program: "copilot".into(),
            args: vec!["--acp".into(), "--stdio".into()],
        }
    }
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnProcess { program, args } => f
                .debug_struct("SpawnProcess")
                .field("program", program)
                .field("args", args)
                .finish(),
            Self::Connected { .. } => f.debug_struct("Connected").finish_non_exhaustive(),
        }
    }
}

/// Everything the prompt template needs.
#[derive(Debug, Clone)]
pub struct EvalContext {
    /// One-sentence task description, appended to the header.
    pub task: String,
    /// Optional unified diff to embed in the prompt.
    pub diff: Option<String>,
    /// Preference files rendered in declaration order.
    pub preferences: Vec<PreferenceFile>,
    /// Optional list of paths to spotlight in the prompt.
    pub focus_paths: Vec<String>,
}

/// One named preference snippet (e.g. `STYLE.md`, `REVIEWING.md`).
#[derive(Debug, Clone)]
pub struct PreferenceFile {
    pub name: String,
    pub content: String,
}

/// Successful evaluation outcome.
#[derive(Debug, Clone)]
pub struct EvalResult {
    /// Concatenation of every `agent_message_chunk.text` seen during
    /// the prompt turn. Nothing else — no thought text, no tool call
    /// metadata.
    pub verdict: String,
    /// The `StopReason` from `PromptResult`, as a canonical snake_case
    /// string: `"end_turn" | "max_tokens" | "max_turn_requests" |
    /// "refusal" | "cancelled"`.
    pub stop_reason: String,
    /// One entry per completed `terminal/create` call, in issuance
    /// order.
    pub tool_calls: Vec<ToolCallLog>,
    /// Wall-clock duration of the whole `evaluate` call.
    pub duration: Duration,
    /// `true` if a `.git` directory was found and loaded.
    pub is_git_repo: bool,
    /// Basic byte / file counts for the loaded VFS.
    pub repo_stats: RepoStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallLog {
    pub command: String,
    pub exit_code: i32,
    pub duration: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStats {
    pub files: u64,
    pub bytes: u64,
}

/// Failure modes for [`crate::evaluate`].
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("repo too large: {total} bytes (limit {limit})")]
    RepoTooLarge { total: u64, limit: u64 },
    #[error("failed to load repo into VFS: {0}")]
    VfsLoad(#[from] devdev_vfs::LoadError),
    #[error("acp error: {0}")]
    Acp(#[from] devdev_acp::AcpError),
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),
    #[error("evaluation exceeded {0:?}")]
    Timeout(Duration),
    #[error("agent subprocess exited unexpectedly")]
    CliCrashed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
