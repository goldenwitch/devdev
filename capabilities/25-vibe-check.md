---
id: vibe-check
title: "Vibe Check — Preference Authoring"
status: not-started
type: composition
phase: 5
crate: devdev-cli
priority: P1
depends-on: [session-router]
effort: L
---

# P5-01 — Vibe Check (Preference Authoring)

The first of the two pillars from `spirit/outline.md` §1 ("The Vibe Check"). DevDev interviews the user in natural language and writes their technical preferences out as plain Markdown files in a `.devdev/` directory at the workspace root. No YAML schemas, no DSLs — file-scoped Markdown documents that the Scout (P5-02) and the Heavy (existing ACP path) can read directly.

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

- Should `.devdev/` live in the repo (tracked, shareable) or in the user's home (`~/.devdev/`, private)? Outline says "personal", suggesting home; PR review use cases suggest repo. **Resolution path:** support both — repo-local takes precedence over home, like `.gitconfig`.
- What's the scribe's prompt? Probably a small canned system prompt that says "you are interviewing the user about their coding preferences; write one Markdown file per distinct topic; keep titles short and prose conversational."
- What does the scribe do when the user says something contradictory to an existing file? Append a "Revision (date)" section, not silently overwrite.

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
