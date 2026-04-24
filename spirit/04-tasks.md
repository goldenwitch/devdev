# 04 — Tasks

A **task** is the unit of durable, resumable, event-driven work that
sits above the agent loop. A DevDev task is not a single prompt and
not a single tool call; it is a long-lived process that observes some
external state, decides when to involve the agent, and produces
outputs gated on the user's approval.

This document specifies the task model. The MonitorPR task is used
throughout as a concrete example, because it is the exemplar: the
first real task, the one the model was shaped around.

## Why a task model at all

An agent session alone does not solve the workflows DevDev targets.
Sessions are synchronous: you send a prompt, you get a response, the
session is done until you prompt again. The workflows DevDev targets
are *reactive*: "when a new commit lands on this PR, do X"; "while
this ticket is in progress, watch for Y."

A task provides the state machine that turns reactive observations
into synchronous agent invocations:

- It knows what it is watching.
- It knows what it has already observed (so it does not re-react to
  the same state twice).
- It knows what outputs it has staged, and whether they have been
  approved, published, or declined.
- It can be serialised and resumed across daemon restarts without
  losing its place.

Tasks are the durable atoms of DevDev's work. Agent sessions are
transient; task state is persistent.

## Task shape

Every task implements the same small interface. Reduced to its
irreducible pieces:

- **An identifier.** Stable across the task's lifetime; used as
  the handle in IPC and for the session-router mapping.
- **A kind.** `monitor-pr`, `evaluate`, and future kinds. Kind
  determines how the task interprets its inputs and outputs.
- **A reference to what it's watching.** A PR URL, a ticket ID, a
  commit SHA — whatever unique handle lets the task observe the
  external state.
- **Accumulated context.** Prior observations and intermediate
  artefacts, carried forward into future agent sessions so the task
  does not start from zero on every poll.
- **Serialisation.** Every task can persist itself to a blob and
  hydrate from one. The daemon checkpoints the blob on graceful
  shutdown and on demand.

A task's behaviour is exercised via three entrypoints:

- **On creation** — prepare initial state. Parse the reference,
  seed the workspace, fetch initial external state. May or may not
  involve the agent.
- **On poll** — called periodically by the daemon. The task
  inspects external state; if something changed, it decides what to
  do about it (often: drive an agent session and stage an output).
- **On terminal transition** — the task decides it is finished
  (the PR merged, the ticket closed). It cleans up, destroys any
  session, and marks itself complete.

The poll cadence is the daemon's concern, not the task's. Tasks are
stateless between polls except through their serialised blob.

## Idempotency

Tasks *must* be idempotent with respect to external state. Concretely:

If MonitorPR polls a PR and sees the same commit SHA it saw last
time, it **does not** re-review. It returns early.

The mechanism is a local ledger: an on-disk key-value store mapping
`(task-kind, reference, last-seen-state) → observation-hash`. Before
acting, a task consults the ledger. After acting, it records the new
state.

This is not a performance optimisation. It is a correctness
requirement. A task that re-reviews the same PR every poll interval
produces a stream of duplicate comments, which is worse than
producing none. The ledger is how DevDev avoids being a bad citizen
of the user's notification stream.

## Outputs

Task outputs are always staged, never published directly. The
staging contract is:

1. The task produces a candidate output — a review text, a ticket
   comment, a commit message.
2. The daemon stores it locally, associated with the task, in a
   state of "awaiting approval."
3. The user sees the staged output (today: via the CLI; future: via
   the TUI or a notification UI) and either approves, declines, or
   edits-then-approves.
4. Only on approval does the daemon publish the output through the
   appropriate integration (GitHub, Jira, etc.).

The approval gate defaults to on. A `--rude` flag disables it for
users who have calibrated DevDev to their preferences.

## MonitorPR, concretely

MonitorPR is the exemplar task. Its behaviour:

### On creation

1. Parse the PR reference (`owner/repo#number` or URL).
2. Fetch the current head SHA and the diff against the base.
3. Materialise the repository into a workspace at a conventional
   path.
4. Create an agent session scoped to that workspace.
5. Seed the session with preliminary context: the user's
   preference files, the PR metadata, the diff.
6. Emit a first review as a staged output.

### On poll

1. Consult the ledger: is the current head SHA the same one the
   task last reviewed?
2. If yes: return. Do nothing.
3. If no: fetch the new diff, re-drive the agent against the
   session with the updated diff, produce a revised review, stage
   it.
4. Record the new SHA in the ledger.

### On terminal transition

1. Detect via `get_pr_status` that the PR is closed or merged.
2. Destroy the agent session.
3. Flag any unpublished staged outputs as orphaned (for the user
   to decide what to do with).
4. Mark the task complete; the daemon will not poll it further.

### Not in scope for MonitorPR

- Creating or modifying PRs.
- Running CI.
- Responding to comment threads (a future task kind).
- Monitoring multiple PRs (multiple instances, one per PR).

## Events and tasks

Tasks are created in response to events; they are not created in
response to prompts. "Prompt a single question" is an interactive
session, not a task. "Watch this repo and act on new PRs" is a task.

Today, task creation is initiated by the user explicitly (via the
CLI). Future work wires external event sources — webhook receivers,
poll-based feeds, filesystem watchers — to task creation, so that
`monitor-pr` tasks appear automatically when a PR opens on a
repository the user has told DevDev to watch.

## Preferences

Every task that involves the agent draws on **preference files**:
markdown documents the user has written describing their style,
opinions, and boundaries. These are not configuration; they are
natural-language documents the agent reads.

The preference layer has two pieces:

1. **Storage.** A conventional directory structure of markdown
   files per user, per project, per context.
2. **Selection.** A lightweight LLM ("the Scout") runs before the
   main agent session and picks the subset of preference files
   that apply to this task's current invocation. The Heavy (the
   main agent) then gets only the relevant preferences.

Selection matters because preference files are meant to accumulate
over time. The user will write dozens, and sending all of them into
every session would blow the context budget. The Scout is the
relevance filter.

Today the preference layer and the Scout are roadmap, not shipping.
MonitorPR in its current form hardcodes a minimal instruction set;
preference-driven operation is the next feature it grows into.

## Checkpointing

Tasks are serialised on daemon shutdown and hydrated on startup.
The daemon checkpoints task state alongside the workspace blob that
the task owns.

This means, in the good case:

- A user stops DevDev mid-work (`devdev down`).
- The daemon walks active tasks, asks each to serialise itself,
  writes the blobs to local storage, destroys agent sessions,
  exits.
- Hours later, the user starts DevDev again (`devdev up`).
- The daemon reads back the blobs, re-creates each task, creates a
  fresh agent session per task, re-seeds each session with the
  task's accumulated context.
- Polling resumes. From the outside, no work was lost.

In the bad case (crash, signal, `kill -9`), the last-serialised
checkpoint is the recovery point. Tasks that had work in flight
since the last checkpoint re-do that work on next poll — which is
fine, because the ledger guarantees they do not publish duplicate
output.

## Roadmap from this layer's perspective

- **Second task kind.** MonitorPR is the exemplar. Others on the
  roadmap: `evaluate` (one-shot codebase review), `scout` (answer
  a question), `triage` (categorise an incoming ticket).
- **Preference-driven operation.** The Scout + preference file
  selection described above.
- **Automatic task creation from events.** Webhook and poll-feed
  integration so the user does not need to manually create a
  MonitorPR task for every PR.
- **Richer approval workflows.** Today approvals are accept/decline;
  future work adds edit-before-approve and batch-approve.
- **Task dependencies.** A task that can wait on another task's
  output. Not a near-term concern, but a natural extension.
