# 03 — The Agent Loop

This document specifies the upper layer: how DevDev drives a coding
agent against a workspace. The workspace alone is a library; once you
want the full product — a daemon that manages sessions, routes
events, and orchestrates an agent's work over time — you are in the
territory this document describes.

Two protocols are at play. Both are documented here because both
affect the observable behaviour of a DevDev instance.

## ACP: agent work

**ACP** (Agent Communication Protocol) is the protocol over which
the coding agent itself operates. DevDev spawns the agent as a
subprocess in ACP mode; communication is structured JSON over the
subprocess's stdio, in both directions.

The agent DevDev targets today is the **GitHub Copilot CLI**, invoked
as `copilot --acp --allow-all-tools`. Other ACP-speaking agents would
in principle work; the contract is the protocol, not the vendor.

### What DevDev sends

- **Session creation.** DevDev tells the agent "start a session for
  task X; the working directory is this mount path." The agent
  allocates internal state and replies with a session identifier.
- **Prompts.** DevDev sends text prompts — the task's current
  context, a question, an instruction — targeted at a specific
  session.
- **Session destruction.** DevDev tells the agent to drop session
  state when the task completes or is cancelled.

### What DevDev receives

- **Streaming response chunks.** The agent emits incremental output
  as it works. DevDev forwards these to whatever client is
  listening (the TUI, the CLI, an IPC consumer).
- **Completion signals.** The agent marks the end of a response
  turn. DevDev uses this to settle the session state.
- **Tool-use telemetry.** Under `--allow-all-tools` the agent
  invokes its own tools (shell, fs edit, web) directly against
  the workspace; DevDev observes tool usage via ACP session
  updates rather than intercepting each call. A stricter "hook
  every tool" profile exists as an option but is not the default
  path.

### Why `--allow-all-tools`

Three reasons:

1. **Throughput.** Hook-every-tool profiles pay a latency cost on
   every invocation. For iterative work that issues hundreds of
   tool calls, the cost is prohibitive.
2. **Fidelity.** The agent's native tools are well-tuned; proxying
   them through DevDev means writing and maintaining adapters for
   every tool surface we care about.
3. **The workspace already bounds the filesystem view.** The tools
   operating inside the mount see the mount, not the host tree.
   Process-level containment is a separate concern (see roadmap).

The tradeoff is that DevDev cannot, under this profile, *decline*
an individual tool call. Rejection happens at the task/approval
level, not per-tool.

### Multiplexing

A single Copilot CLI subprocess hosts multiple ACP sessions — one
per active task, plus one for interactive chat use. DevDev was
designed originally to run a pool of agent processes; empirically,
one subprocess multiplexes cleanly across tasks and the pool was
retained only as a future fallback.

If the subprocess dies, DevDev:

1. Detects the exit.
2. Respawns the subprocess.
3. Recreates each active task's session, re-injecting its context.
4. Tasks resume their work.

Tasks are durable; sessions are not. This is a deliberate
asymmetry — sessions are cheap, tasks are not.

## MCP: DevDev-specific context

**MCP** (Model Context Protocol) is the *other* protocol DevDev
uses, and it runs in the opposite direction from ACP.

Where ACP is "DevDev speaks to the agent," MCP is "the agent
speaks to DevDev." The agent, while working in a session, may need
to ask DevDev things only DevDev knows: *what tasks are running?
what has this task already observed? what preference files apply
to this event?*

DevDev exposes an MCP server — a Streamable-HTTP endpoint the
Copilot CLI connects to during session boot — carrying a curated
set of tools specific to the DevDev host:

- **Task introspection.** Query the task list, a task's history,
  a task's accumulated context.
- **Ledger lookups.** Ask whether DevDev has already evaluated a
  given artefact state (see [04-tasks.md](04-tasks.md)).
- **Preference discovery.** Enumerate the markdown preference files
  that exist for the current user context.

MCP tools are additive, not coercive. The agent is not required to
use them; they are there when the agent reasons that they would
help.

### Why a separate protocol for DevDev tools

ACP's hook surface was considered for this role and rejected. Hooks
are tied to the agent's own tool-call lifecycle, fire on every
invocation, and are expensive to filter. MCP tools are explicit,
discoverable, and only fire when the agent deliberately asks.

## The daemon

The daemon is the long-lived process that owns the agent subprocess,
the MCP server, the task registry, and the IPC surface.

### Lifecycle

- **`up`** — start the daemon. Binds the IPC listener, spawns the
  agent subprocess, starts the MCP server, reloads any persisted
  task state.
- **`down`** — stop the daemon cleanly. Checkpoint active tasks,
  destroy agent sessions, shut down the subprocess, release the
  IPC socket.
- **`status`** — a one-shot query answered by the live daemon.
  Reports subprocess health, active task count, session count.

### IPC surface

Clients talk to the daemon over a newline-delimited JSON protocol
on a local TCP port (today) or a Unix socket / named pipe (future).
The protocol is deliberately simple so that a shell script, a TUI, a
web client, or a webhook receiver can all drive it.

Today's commands:

- `send` — push a prompt into a specific session; receive the
  response stream.
- `status` — report-current-state, as above.
- Task-management commands are roadmap (see `ROADMAP.md`).

### State persistence

The daemon is state-preserving across restarts within reason:

- The workspace blob for each active task is serialised to disk on
  clean shutdown.
- The task registry (task IDs, kinds, last-seen artefact states) is
  serialised alongside.
- Agent session IDs are **not** persisted — they belong to the
  subprocess, which does not survive a restart. On boot, DevDev
  recreates sessions from task context.

## How a request flows

An end-to-end trace of "user asks a question via `devdev send`":

1. The CLI client opens an IPC connection and writes a `send`
   message with a target session and prompt text.
2. The daemon receives the message, routes it to its session
   manager, looks up the corresponding ACP session.
3. The session manager calls into the ACP backend, which writes a
   prompt message to the agent subprocess's stdin.
4. The agent begins work. As it invokes its own tools against the
   mounted workspace, the tools see the in-memory filesystem.
5. As the agent reasons, it may call MCP tools on the DevDev host
   (over the Streamable-HTTP endpoint) to fetch task state or
   preference files.
6. The agent emits streaming response chunks over ACP. The session
   manager forwards each chunk back through the IPC surface to the
   CLI client.
7. When the agent marks the turn complete, the daemon emits a
   completion signal and closes the response stream. The client
   prints the final message and exits.

## Approvals and side effects

By design, DevDev never takes destructive action on the user's
behalf by default. When a task produces an output destined for the
outside world — a PR comment, a ticket update, a push — the output
is staged locally and requires the user's one-click approval
before it leaves the machine.

A `--rude` flag disables the approval gate for users who have
calibrated DevDev to their preferences and want autonomous
operation. The gate is off by opt-in only; the default is
"nothing leaves the machine without you clicking yes."

## Roadmap from this layer's perspective

- **Task-management IPC commands.** Creating, cancelling, listing
  tasks is not yet in the IPC surface.
- **Scout routing.** A lightweight preamble LLM that picks which
  preference files to load before the main agent gets the prompt
  is a design goal, not yet implemented.
- **Webhook receivers.** External events (GitHub webhooks,
  Jira transitions) arrive at the daemon via IPC today; direct
  webhook ingress is future work.
- **Process containment.** The agent subprocess and any tools it
  launches run with the user's full privileges. Real containment
  is a cross-cutting roadmap item; when it lands it will affect
  both the workspace launcher and the agent subprocess.
