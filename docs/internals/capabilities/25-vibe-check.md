---
id: vibe-check
title: "Vibe Check — Preference Authoring"
status: shipped
type: composition
phase: 5
crate: devdev-cli
priority: P1
depends-on: [session-router]
effort: L
---

# P5-01 — Vibe Check (Preference Authoring)

**Status: shipped (Phase D).** Resolved questions captured below.

The first of the two pillars from `spirit/outline.md` §1 ("The Vibe Check"). DevDev interviews the user in natural language and writes their technical preferences out as plain Markdown files in a `.devdev/` directory at the workspace root. No YAML schemas, no DSLs — file-scoped Markdown documents that the Scout (P5-02) and the Heavy (existing ACP path) can read directly.

## Resolved

- **Location:** `.devdev/` is searched in the workdir, all ancestors, and `~/.devdev/`. Repo-wins, then parent, then home, dedup by title (earliest layer wins). See `crates/devdev-cli/src/preferences.rs`.
- **Scribe prompt:** lives at `crates/devdev-cli/src/vibe_check_prompt.md`, embedded into the binary via `include_str!`. `devdev init` ships it as the session preamble and runs a stdin REPL against the daemon's ACP session.
- **Revision behaviour:** prompt instructs the scribe to append `## Revision <date>` sections rather than overwrite — enforced socially via the prompt, not by the file layer.
- **Surfacing into PR review:** the per-PR `MonitorPrTask` carries a `Vec<PathBuf>` of preference paths (`with_preferences`); the prompt lists them so the agent reads them on demand via the existing MCP `fs/*` tools. Auto-injection from dispatch is deferred (would require relocating the loader to break a daemon→cli cycle).

## Scope

**In:**
- `devdev init` command: launch a scribe conversation that writes `.devdev/<topic>.md` files.
- File-per-topic convention: one Markdown file per topic area (e.g. `style.md`, `tests.md`, `dependencies.md`, `pr-review.md`). One title, free-form prose.
- The scribe agent uses the existing ACP session machinery (post-P2-06) — no new agent backend.
- `devdev preferences list` / `devdev preferences edit <topic>`: trivial CLI helpers, not a TUI.
- Re-run safety: if `.devdev/` already exists, `devdev init` resumes / appends, never overwrites silently.

**Out:**
- Schema validation. Markdown is the schema.
- Multi-user / org-wide preferences. User-scoped only for now.
- A web UI or VS Code extension for editing.
- Importing existing config (eslintrc, rustfmt.toml, etc.). Stretch goal for Phase 6.

## Open Questions

*(All previously open questions resolved — see Resolved section above.)*

## Dependencies

- **P2-06 (session-router)** — needed for the scribe's ACP session.
- Optionally **P2-03 (chat TUI)** if the scribe should run interactively in the TUI rather than a single-shot CLI flow.

## Acceptance Criteria

- `devdev init` in an empty workspace creates `.devdev/` with at least one preference file authored from a multi-turn conversation.
- Re-running `devdev init` preserves prior files and only adds/appends.
- `devdev preferences list` enumerates the files and their titles.
- A subsequent `devdev send "review this PR"` invocation can read the files (proven by the agent quoting from them in its response).

## Why Now (or Not Yet)

This is the first half of "what makes DevDev a product, not just a sandbox." Without it, the user has no way to express their vibes — and Scout (P5-02) has nothing to route to. Build it after P2-06 lands so the scribe has a real ACP session to talk to; build it before P5-02 because Scout is meaningless without files to point at.
