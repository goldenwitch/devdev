---
id: daemon-lifecycle
title: "Daemon Lifecycle & IPC"
status: done
type: composition
phase: 2
crate: devdev-daemon
priority: P0
depends-on: [vfs-serialization]
effort: XL
---

# P2-02 — Daemon Lifecycle & IPC

**New crate: `devdev-daemon`.** This is the central long-running process. It owns the VFS, the sandbox engine, and all task state. The CLI commands (`devdev up`, `devdev down`, `devdev status`, etc.) talk to it over IPC.

Phase 1's `evaluate()` was a function call that created everything, ran once, and destroyed everything. This capability replaces that model with a persistent daemon that holds state across interactions.

## Scope

**In:**
- Daemon process: `daemon::start()` boots, `daemon::stop()` checkpoints and exits.
- PID file at `~/.devdev/daemon.pid`. Single-instance guard: `devdev up` fails if daemon already running.
- Signal handling: SIGTERM/SIGINT → checkpoint + clean exit. (On Windows: Ctrl+C handler via `ctrlc` crate.)
- IPC server: listen on a local socket for commands from CLI and TUI.
  - Platform: named pipe on Windows (`\\.\pipe\devdev-{user}`), Unix domain socket on Linux/macOS (`~/.devdev/daemon.sock`).
- IPC protocol: NDJSON over the socket. Request/response with `id` field for multiplexing.
- Checkpoint save/restore:
  - `devdev down` → serialize VFS (P2-00) + task state (P2-04) + shell environments → write to `~/.devdev/checkpoint.bin`.
  - `devdev up --checkpoint` → read checkpoint, restore VFS, restore tasks.
- Daemon state: holds `Arc<Mutex<MemFs>>`, task registry, ACP client, session router.
- CLI surface (these are CLI commands that send IPC messages to the daemon):
  - `devdev up [--checkpoint] [--foreground]` — start daemon
  - `devdev down` — checkpoint + stop
  - `devdev status [--json]` — running? tasks? repos?
  - `devdev task add [--auto-approve] [--dry-run] "<description>"` — create task
  - `devdev task list [--json]` — list tasks
  - `devdev task cancel <id>` — cancel task
  - `devdev task log <id> [--json]` — get task output
  - `devdev send [--auto-approve] [--json] "<message>"` — one-shot message

**Out:**
- Process supervision (systemd/launchd/Windows Service). The daemon is started by the user.
- TUI rendering (that's P2-03 — TUI connects to daemon over IPC).
- Task scheduling logic (that's P2-04 — daemon provides the runtime, tasks provides the scheduler).
- Cluster / multi-machine. One daemon per machine.

## PoC Requirement (Spec Rule 2)

Before committing to the IPC mechanism:

1. Build a throwaway PoC: named pipe on Windows + Unix domain socket on Linux/macOS, NDJSON framing, round-trip latency test.
2. Verify: can send a JSON request, receive a JSON response, <1ms latency.
3. If named pipes are painful on Windows, fall back to localhost TCP (127.0.0.1:random-port, port written to `~/.devdev/daemon.port`).

**PoC Result:** _Not yet run._

## Interface

### IPC Protocol

```
→ {"id": 1, "method": "status"}
← {"id": 1, "result": {"running": true, "tasks": 2, "repos": ["org/api-server", "org/frontend"]}}

→ {"id": 2, "method": "task/add", "params": {"description": "Monitor PR #247 in org/repo", "auto_approve": false}}
← {"id": 2, "result": {"task_id": "t-1", "status": "created"}}

→ {"id": 3, "method": "task/list"}
← {"id": 3, "result": {"tasks": [{"id": "t-1", "description": "...", "status": "polling"}]}}

→ {"id": 4, "method": "task/cancel", "params": {"task_id": "t-1"}}
← {"id": 4, "result": {"cancelled": true}}

→ {"id": 5, "method": "send", "params": {"text": "Review the latest push", "auto_approve": true}}
← {"id": 5, "result": {"response": "I reviewed the push...", "actions_taken": []}}

→ {"id": 6, "method": "attach"}
← (switches to streaming mode: messages flow bidirectionally as NDJSON)

→ {"id": 7, "method": "shutdown"}
← {"id": 7, "result": {"checkpoint_saved": true}}
(daemon exits)
```

### Rust API

```rust
pub struct DaemonConfig {
    pub data_dir: PathBuf,          // default: ~/.devdev/
    pub checkpoint_on_stop: bool,   // default: true
    pub foreground: bool,           // default: false (detach)
}

pub struct Daemon {
    config: DaemonConfig,
    vfs: Arc<Mutex<MemFs>>,
    // tasks: TaskRegistry,         // wired in P2-04
    // router: SessionRouter,       // wired in P2-06
    ipc: IpcServer,
}

impl Daemon {
    /// Boot the daemon. If checkpoint exists and requested, restore from it.
    pub async fn start(config: DaemonConfig) -> Result<Self, DaemonError>;

    /// Save checkpoint and shut down cleanly.
    pub async fn stop(&mut self) -> Result<(), DaemonError>;

    /// Run the main event loop: accept IPC connections, dispatch commands.
    pub async fn run(&mut self) -> Result<(), DaemonError>;
}

/// IPC server that listens for connections and dispatches to handlers.
pub struct IpcServer { /* platform-specific */ }

impl IpcServer {
    pub async fn bind(data_dir: &Path) -> Result<Self, IpcError>;
    pub async fn accept(&self) -> Result<IpcConnection, IpcError>;
}

/// A single IPC connection (from CLI command or TUI).
pub struct IpcConnection { /* reader + writer */ }

impl IpcConnection {
    pub async fn read_message(&mut self) -> Result<IpcRequest, IpcError>;
    pub async fn write_message(&mut self, msg: &IpcResponse) -> Result<(), IpcError>;
}
```

## Implementation Notes

- **Detach on `devdev up`:** Spawn the daemon as a child process, wait for it to write PID file, then exit the CLI. On `--foreground`, skip detach.
- **PID file lifecycle:** Write PID on start, delete on clean stop. On `devdev up`, check if PID file exists and if the process is alive. If PID exists but process is dead, clean up stale PID file and start.
- **Checkpoint atomicity:** Write to `checkpoint.tmp`, then rename to `checkpoint.bin`. Prevents corruption if the process is killed mid-write.
- **IPC multiplexing:** Each connection is independent. Multiple CLIs / TUI can connect simultaneously. The `id` field in requests lets responses be matched to requests on a single connection. The `attach` method switches a connection to streaming mode for the TUI / headless chat.
- **Graceful shutdown:** On `shutdown` IPC command or signal, cancel all tasks, checkpoint, close IPC, delete PID file, exit.
- **Data directory:** `~/.devdev/` by default. Configurable via `DEVDEV_HOME` env var. Contains: `daemon.pid`, `daemon.sock` (or `.pipe` name), `checkpoint.bin`, `logs/`.

## Files

```
crates/devdev-daemon/Cargo.toml
crates/devdev-daemon/src/lib.rs         — Daemon struct, start/stop/run
crates/devdev-daemon/src/ipc.rs         — IpcServer, IpcConnection, platform abstraction
crates/devdev-daemon/src/checkpoint.rs  — serialize/deserialize full daemon state
crates/devdev-daemon/src/pid.rs         — PID file management, single-instance guard
crates/devdev-daemon/src/signals.rs     — Signal/Ctrl+C handling
crates/devdev-cli/src/main.rs           — new subcommands: up, down, status, task, send
```

## Spec Requirements

| Req | Spec Section | Description |
|-----|-------------|-------------|
| SR-02-1 | §3.1 | `devdev up` starts daemon, optionally from checkpoint |
| SR-02-2 | §3.1 | `devdev down` saves checkpoint, shuts down cleanly |
| SR-02-3 | §3.1 | PID file, single-instance guard |
| SR-02-4 | §3.1 | Signal handling → checkpoint + exit |
| SR-02-5 | §3.1 | IPC between CLI and daemon (socket / named pipe) |
| SR-02-6 | §3.1 | `devdev status [--json]` |
| SR-02-7 | §3.1 | `devdev task add/list/cancel` CLI commands |
| SR-02-8 | §3.1 | `devdev send` one-shot message |
| SR-02-9 | §3.1 | Checkpoint is snapshot, not journal |
| SR-02-10 | §2 Principle 6 | All CLI commands produce `--json` output for headless use |

## Acceptance Tests

- [ ] `daemon_start_creates_pid_file` — start daemon, verify PID file exists and contains running PID
- [ ] `daemon_double_start_fails` — start daemon, try start again → error "already running"
- [ ] `daemon_stale_pid_cleaned` — create PID file with dead PID, start daemon → succeeds (cleans stale)
- [ ] `daemon_stop_removes_pid` — start, stop → PID file deleted
- [ ] `daemon_stop_saves_checkpoint` — start, load a repo, stop → `checkpoint.bin` exists
- [ ] `daemon_start_from_checkpoint` — start with checkpoint, verify VFS tree matches pre-stop state
- [ ] `ipc_status_returns_json` — start daemon, send `status` over IPC → valid JSON response
- [ ] `ipc_shutdown_exits_cleanly` — send `shutdown` → daemon exits with code 0, PID file removed
- [ ] `ipc_concurrent_connections` — two clients connect simultaneously, both get responses
- [ ] `signal_triggers_checkpoint` — send SIGTERM (or simulate Ctrl+C on Windows) → checkpoint saved, daemon exits
- [ ] `checkpoint_atomic_write` — verify checkpoint writes to .tmp then renames (no partial file)
- [ ] `cli_up_detaches` — `devdev up` returns immediately, daemon continues in background
- [ ] `cli_up_foreground_blocks` — `devdev up --foreground` doesn't return until stopped
- [ ] `cli_status_json` — `devdev status --json` returns parseable JSON with expected fields
- [ ] `cli_send_one_shot` — `devdev send "hello" --json` returns agent response as JSON, then exits

## Spec Compliance Checklist

- [ ] SR-02-1 through SR-02-10: all requirements covered
- [ ] PoC result recorded
- [ ] All acceptance tests passing
