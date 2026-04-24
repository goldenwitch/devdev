# 01 — The Concept

## The one-sentence description

**DevDev is a local daemon that drives a coding agent against a
virtual workspace on your behalf, so that long-running, event-driven
developer tasks can be automated without leaving your machine.**

That sentence names two things the rest of this document unpacks:

- A **virtual workspace** — a bounded, inspectable, serializable
  place where an agent does work. This is the lower layer.
- A **daemon + task model** — the thing that decides when to invoke
  the agent, what context to give it, and what to do with its output.
  This is the upper layer.

The two layers are separable by design. Library consumers can take
the workspace and ignore the rest.

## Two audiences

DevDev is written for two concentric circles of users, and the
documentation reflects that.

### Outer circle: the workspace-curious

People interested in the idea of giving an agent a workspace it can
poke at freely — files, processes, a `git` binary, a `cargo`
binary — without that poking bleeding into unrelated parts of the
host. The workspace crate stands alone and is usable without the rest
of DevDev.

This circle is where collaboration is most likely today. The
workspace layer's contracts are stable enough to build on and
narrow enough to reason about.

### Inner circle: the DevDev-hosting

People who want to run the full DevDev product on their machine and
have it monitor developer artefacts (PRs, tickets, CI outputs) and
act on their behalf. The monitoring loop, task model, and agent
orchestration target this circle.

This circle is under active development. Core lifecycle and the
MonitorPR exemplar are in place; several pieces documented here are
roadmap rather than today.

## Why a workspace at all

Coding agents are most useful when they can *try things* — edit
files, run commands, observe output, revise. Most general-purpose
agent harnesses solve this by either:

1. Running the agent directly against your real working directory,
   trusting it not to break anything important.
2. Containerizing each run, paying the container cost and losing
   the fidelity of your local toolchain.

Neither is satisfying for long-running, repeated, iterative work.
(1) is unsafe and unrepeatable; (2) is slow and forces an
environment-parity problem.

DevDev's workspace is a middle path: **an in-memory filesystem
mounted as a real directory on your host.** The agent sees a normal
directory and uses the normal tools you already have installed. The
files it edits exist only in the process's memory until you ask for a
snapshot. When the work is done, the mount goes away.

This is not a sandbox. Process containment is a separate problem on
the roadmap. The workspace is about giving the agent a *place* to
work that is cheap to spin up, snapshot, and discard.

## Why a daemon

The target workflows are event-driven, not conversational: "a PR is
opened, go review it"; "CI failed on main, investigate"; "a ticket
transitioned to in-progress, draft a starting plan." These are
long-lived processes, not single prompts.

A daemon:

- Persists across terminal sessions, so it can be the thing
  receiving a webhook or polling a feed.
- Can drive a single agent subprocess across many logical sessions,
  amortising startup cost.
- Owns the workspace lifecycle and can snapshot / resume work units
  on its own schedule.
- Provides a stable IPC surface that other tools (a TUI, a CLI,
  external webhooks, a web UI) can all talk to.

The CLI `devdev up` / `devdev send` / `devdev down` is today's
interface to that daemon. The daemon is the durable thing; the CLI
is a client.

## Why a coding agent

DevDev is deliberately opinionated: the agent is **the
Copilot CLI** running in ACP mode, not "any LLM with a tool call
loop." This is a concession to reality.

Coding agents are a moving target — capabilities, cost, latency,
and tool-use quality change month to month. DevDev picks one credible
agent and builds around its contract rather than trying to abstract
over a space that isn't yet settled. If and when a better one
appears, the ACP-shaped seam is narrow and can be re-pointed.

What this buys:

- **A real tool surface.** The agent can shell out, edit files,
  use `git`, use `cargo`, without DevDev having to re-implement those
  tools as WASM or as hand-written adapters.
- **A real authentication story.** The agent reuses the user's
  existing `gh auth login`; DevDev stays out of the credential
  business.
- **A predictable protocol** (ACP for agent work, MCP for
  DevDev-specific context). These are documented in
  [03-agent-loop.md](03-agent-loop.md).

## What DevDev is *not*

- Not a sandbox. The workspace is a mount; processes launched from
  it run with your user's privileges against your real network, your
  real `$HOME`, and your real system tools. Treat a DevDev
  subprocess the way you would treat any other local process.
- Not a model. DevDev assumes an agent exists and speaks ACP. It
  does not provide one.
- Not a CI system. DevDev can observe CI output but does not run
  your tests for you.
- Not a Git host. DevDev talks to GitHub (today) as a client; it
  does not replace your forge.
- Not a collaboration platform. Outputs (PR comments, ticket
  updates) go out under the user's identity and are gated on the
  user's approval by default.

## What the pieces add up to

If everything in this document works, a developer can:

- Configure DevDev with a handful of markdown files describing how
  they like code to look (the "vibe check" — preferences as
  natural-language markdown, not YAML).
- Point it at sources of events: a GitHub repo, a Jira board, a
  folder of tickets.
- Let it run. When something interesting happens it opens a
  workspace, surfaces an agent in that workspace, and lets the agent build a solution with the tools available.

That's the product on the inner circle.

Separately, a library consumer can:

- Import the workspace crate, hand it files via its in-memory
  filesystem, mount it, run a subprocess against it, snapshot the
  result.

That's the product on the outer circle.

The rest of this directory specifies each layer's contracts in
detail.
