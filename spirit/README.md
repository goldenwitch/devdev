# DevDev: Technical Narrative

This directory is the **canonical description of what DevDev is** — the
concepts, contracts, and boundaries — written independent of the Rust
code that currently implements them. If the implementation ever moves
to another language or restructures its crates, these documents should
still read true.

For code, cap docs, and phase history, see `docs/internals/`. For a
user-facing pitch, see the repository root `README.md`. For a
library-only entry into the workspace layer, see
`crates/devdev-workspace/README.md`.

## Reading order

1. **[01-concept.md](01-concept.md)** — the idea. What DevDev is, who it
   is for, and the distinction between the workspace layer (usable on
   its own) and the agent-host layer (the product being built on top).

2. **[02-workspace-contract.md](02-workspace-contract.md)** — the
   workspace layer in detail: the virtual filesystem, mount model,
   process-launch model, serialization, and snapshot story. This is the
   piece that stands alone.

3. **[03-agent-loop.md](03-agent-loop.md)** — how DevDev drives a coding
   agent against a workspace: session routing, the two protocols in
   play (ACP and MCP), and the daemon lifecycle.

4. **[04-tasks.md](04-tasks.md)** — the task model that sits above the
   agent loop: how external events become durable, resumable work
   units, and how MonitorPR exemplifies the shape.

5. **[05-validation.md](05-validation.md)** — the rubric we hold
   validation code against: no tautologies, no off-path stubs, no
   motte-and-bailey tests. Read before writing a test that claims
   to prove a README bullet.

## What these docs are not

- They are **not a roadmap**. See [`ROADMAP.md`](../ROADMAP.md) for
  what is shipped, next, and aspirational.
- They are **not a contributor guide**. See
  [`CONTRIBUTING.md`](../CONTRIBUTING.md) for how to build, test, and
  submit changes.
- They are **not API reference**. See per-crate `README.md` and the
  rustdoc output for types and methods.
- They do **not** claim process isolation or sandboxing. DevDev runs
  agent subprocesses against your real host filesystem through a
  mounted view; process-level containment is a roadmap item.
