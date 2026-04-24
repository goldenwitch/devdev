//! Live integration tests: prove MCP tool injection works end-to-end
//! against a real `copilot --acp --allow-all-tools` subprocess.
//!
//! Two scenarios:
//!
//! 1. [`live_copilot_calls_devdev_tasks_list`] — inline
//!    [`RecordingProvider`]. Asserts the MCP server was hit (via a
//!    call counter) and that the agent echoes the distinctive task
//!    id we fed it. Tight PoC-level loop.
//!
//! 2. [`live_copilot_sees_registry_tasks`] — real
//!    [`DaemonToolProvider`] wrapping a real [`TaskRegistry`]. Proves
//!    the *production* bridge between the task registry and MCP
//!    delivers registry state to the agent. This is the path
//!    `devdev up` actually takes.
//!
//! ## Running
//!
//! Opt-in, gated behind both `--ignored` and `DEVDEV_LIVE_COPILOT=1`:
//!
//! ```powershell
//! $env:DEVDEV_LIVE_COPILOT = "1"
//! cargo test -p devdev-cli --test live_mcp -- --ignored --nocapture
//! ```
//!
//! Requires: `copilot` on PATH, already signed in (no browser flow
//! from inside the test). If the binary isn't present or isn't
//! authenticated, the tests skip with a clear message rather than
//! masquerading as a pass.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use devdev_daemon::mcp::{
    DaemonToolProvider, McpEndpoint, McpProviderError, McpServer, McpToolProvider, TaskInfo,
};
use devdev_daemon::router::SessionBackend;
use devdev_tasks::registry::TaskRegistry;
use devdev_tasks::task::{Task, TaskError, TaskMessage, TaskStatus};
use tokio::sync::Mutex;

use devdev_cli::acp_backend::AcpSessionBackend;

// ── Test providers / tasks ────────────────────────────────────────

/// Provider wrapper that records every `tasks_list` invocation so the
/// test can assert the agent actually reached into MCP (and wasn't
/// just hallucinating task data from its training prior).
#[derive(Default)]
struct RecordingProvider {
    calls: Mutex<Vec<&'static str>>,
    tasks: Vec<TaskInfo>,
}

#[async_trait]
impl McpToolProvider for RecordingProvider {
    async fn tasks_list(&self) -> Result<Vec<TaskInfo>, McpProviderError> {
        self.calls.lock().await.push("tasks_list");
        Ok(self.tasks.clone())
    }
}

impl RecordingProvider {
    async fn call_count(&self, name: &str) -> usize {
        self.calls
            .lock()
            .await
            .iter()
            .filter(|n| **n == name)
            .count()
    }
}

/// Minimal `Task` impl used only in tests — no real poll behaviour,
/// just exposes the four accessors the provider reads.
struct FakeTask {
    id: String,
    kind: &'static str,
    desc: String,
    status: TaskStatus,
}

#[async_trait]
impl Task for FakeTask {
    fn id(&self) -> &str {
        &self.id
    }
    fn describe(&self) -> String {
        self.desc.clone()
    }
    fn status(&self) -> &TaskStatus {
        &self.status
    }
    fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
    }
    async fn poll(&mut self) -> Result<Vec<TaskMessage>, TaskError> {
        Ok(vec![])
    }
    fn serialize(&self) -> Result<serde_json::Value, TaskError> {
        Ok(serde_json::json!({}))
    }
    fn task_type(&self) -> &str {
        self.kind
    }
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(60)
    }
}

// ── Environment helpers ───────────────────────────────────────────

fn live_enabled() -> bool {
    std::env::var("DEVDEV_LIVE_COPILOT")
        .ok()
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Resolve a command name to a full path on Windows, preferring
/// `.cmd` / `.exe` over `.ps1` (which `CreateProcess` can't execute
/// directly). Returns `None` if not found on PATH.
#[cfg(windows)]
fn which_windows(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(';') {
        for ext in &[".cmd", ".bat", ".exe"] {
            let candidate = std::path::Path::new(dir).join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn which_windows(_name: &str) -> Option<String> {
    None
}

fn init_tracing() {
    let default_filter =
        "devdev_acp::wire=trace,devdev_daemon::mcp=debug,devdev_acp=debug,devdev_cli=debug,warn";
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .with_target(true)
        .try_init();
}

fn resolve_copilot() -> Option<String> {
    if cfg!(windows) {
        which_windows("copilot")
    } else {
        Some("copilot".to_string())
    }
}

/// Run a single `create_session` → `send_prompt` → `destroy_session`
/// round-trip against the real Copilot binary, with the given MCP
/// endpoint injected. Returns the agent's reply text.
async fn run_prompt(
    copilot_bin: &str,
    endpoint: McpEndpoint,
    scratch_suffix: &str,
    prompt: &str,
) -> String {
    let backend = AcpSessionBackend::new(
        copilot_bin.to_string(),
        vec!["--acp".to_string(), "--allow-all-tools".to_string()],
        Some(endpoint),
    );

    let cwd = std::env::temp_dir()
        .join(format!(
            "devdev-live-mcp-{}-{}",
            std::process::id(),
            scratch_suffix
        ))
        .display()
        .to_string();
    std::fs::create_dir_all(&cwd).expect("mkdir cwd");

    let session_id =
        match tokio::time::timeout(Duration::from_secs(30), backend.create_session(&cwd)).await {
            Ok(Ok(sid)) => sid,
            Ok(Err(e)) => {
                panic!("create_session failed (is `copilot` on PATH and signed in?): {e}")
            }
            Err(_) => panic!("create_session timed out after 30s"),
        };
    eprintln!("[test:{scratch_suffix}] session created: {session_id}");

    let response = tokio::time::timeout(
        Duration::from_secs(120),
        backend.send_prompt(&session_id, prompt),
    )
    .await
    .expect("send_prompt timed out after 120s")
    .expect("send_prompt errored");

    eprintln!("[test:{scratch_suffix}] agent reply: {}", response.text);
    eprintln!(
        "[test:{scratch_suffix}] stop_reason: {}",
        response.stop_reason
    );

    let _ = backend.destroy_session(&session_id).await;
    response.text
}

const PROMPT_TEMPLATE: &str = "Call the MCP tool `devdev_tasks_list` now and reply with the `id` field of \
     the first task you receive. Do not run any shell commands.";

// ── Test 1: recording provider (PoC flavour) ──────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires live, signed-in Copilot CLI; run with DEVDEV_LIVE_COPILOT=1 and --ignored"]
async fn live_copilot_calls_devdev_tasks_list() {
    if !live_enabled() {
        eprintln!("skipped: DEVDEV_LIVE_COPILOT != 1");
        return;
    }
    init_tracing();

    let provider = Arc::new(RecordingProvider {
        calls: Mutex::new(Vec::new()),
        tasks: vec![TaskInfo {
            id: "t-live-42".into(),
            kind: "monitor-pr".into(),
            name: "monitor github/example#7".into(),
            status: "polling".into(),
        }],
    });
    let server = McpServer::start(provider.clone())
        .await
        .expect("mcp server start");
    let endpoint = server.endpoint().clone();
    eprintln!("[test:recording] MCP server listening at {}", endpoint.url);

    let copilot_bin = match resolve_copilot() {
        Some(p) => p,
        None => {
            server.shutdown().await;
            eprintln!("skipped: `copilot` not on PATH");
            return;
        }
    };
    eprintln!("[test:recording] using copilot binary: {copilot_bin}");

    let reply = run_prompt(&copilot_bin, endpoint, "recording", PROMPT_TEMPLATE).await;

    server.shutdown().await;

    // Proof 1: the MCP server saw at least one call.
    let count = provider.call_count("tasks_list").await;
    assert!(
        count >= 1,
        "expected ≥1 devdev_tasks_list call; reply was: {reply:?}"
    );

    // Proof 2: the reply echoes our distinctive task id (no hallucination).
    assert!(
        reply.contains("t-live-42"),
        "agent reply should echo the task id; got: {reply:?}"
    );
}

// ── Test 2: real daemon provider (production bridge) ──────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires live, signed-in Copilot CLI; run with DEVDEV_LIVE_COPILOT=1 and --ignored"]
async fn live_copilot_sees_registry_tasks() {
    if !live_enabled() {
        eprintln!("skipped: DEVDEV_LIVE_COPILOT != 1");
        return;
    }
    init_tracing();

    // Build the exact provider stack `devdev up` uses: a shared
    // `Arc<Mutex<TaskRegistry>>` wrapped in `DaemonToolProvider`.
    // Seed the registry with a task bearing a distinctive id that
    // won't collide with anything the model might hallucinate.
    let registry = Arc::new(Mutex::new(TaskRegistry::new()));
    {
        let mut reg = registry.lock().await;
        reg.add(Box::new(FakeTask {
            id: "t-registry-9001".into(),
            kind: "monitor-pr",
            desc: "monitor github/devdev#registry-proof".into(),
            status: TaskStatus::Polling,
        }));
    }
    let provider = Arc::new(DaemonToolProvider::new(
        Arc::clone(&registry),
        Arc::new(Mutex::new(devdev_workspace::Fs::new())),
    ));

    let server = McpServer::start(provider).await.expect("mcp server start");
    let endpoint = server.endpoint().clone();
    eprintln!("[test:registry] MCP server listening at {}", endpoint.url);

    let copilot_bin = match resolve_copilot() {
        Some(p) => p,
        None => {
            server.shutdown().await;
            eprintln!("skipped: `copilot` not on PATH");
            return;
        }
    };
    eprintln!("[test:registry] using copilot binary: {copilot_bin}");

    let reply = run_prompt(&copilot_bin, endpoint, "registry", PROMPT_TEMPLATE).await;

    server.shutdown().await;

    // Production-path proof: the task added to the registry flowed
    // through `DaemonToolProvider` → `McpServer` → Copilot and back.
    assert!(
        reply.contains("t-registry-9001"),
        "agent reply should echo the registry-backed task id; got: {reply:?}"
    );
}
