# Changelog

All notable changes to this project will be documented in this file.
Format inspired by [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [SemVer](https://semver.org/) once the first
release is cut.

## [Unreleased]

### Added
- **PR shepherding pipeline**: `devdev repo watch <owner>/<repo>` polls
  GitHub, hashes PR state, consults an append-only NDJSON idempotency
  ledger, and emits `PrOpened`/`PrUpdated`/`PrClosed` events on an
  internal `EventBus`. Per-PR `MonitorPrTask`s subscribe and re-prompt
  the agent on each update. Idempotent watch / unwatch via
  `repo/watch` + `repo/unwatch` IPC. New scenario S07 covers the
  user-surface plumbing.
- **`devdev_ask` MCP tool**: universal approval seam exposed to ACP
  agents. Takes `kind={post_review,post_comment,request_token,question}`
  and routes through `ApprovalGate`. On approval for the
  external-action kinds, the response includes a host-derived
  short-lived `GH_TOKEN` so the agent can run `gh` itself — no typed
  `post_review` adapter path.
- **Vibe Check**: `devdev init` runs a scribe session that writes
  `.devdev/*.md` preference files in the user's voice. `devdev
  preferences list` discovers preferences across repo, parents, and
  `~/.devdev/` with repo-wins precedence; `devdev preferences edit
  <name>` opens `$EDITOR`.
- `devdev-workspace`: standalone crate README covering the library
  entry points, minimal example, and platform matrix.
- `ROADMAP.md`: Today / Next / Aspirational breakdown.
- `SECURITY.md`, `CONTRIBUTING.md`: policy + contributor workflow.
- `rust-toolchain.toml`: pinned compiler.
- MIT `LICENSE`.

### Removed
- `placeholder_review_fn` agent-callback seam — superseded by the
  event-driven `MonitorPrTask` + `devdev_ask` flow described above.

### Changed
- Root `README.md` rewritten for the two-audience split
  (workspace-curious vs DevDev-hosting). Explicit non-claim on
  sandboxing.
- User-facing narrative consolidated into
  [`spirit/`](spirit/) (four files, ~300 lines each).
- Historical specs, the 21-file capability index, and ACP research
  moved to [`docs/internals/`](docs/internals/).
- Scenario catalog moved from `spirit/scenarios/` to
  `crates/devdev-scenarios/catalog/` (colocated with its test crate).

### Removed
- `Postmortem.md`, `clippy.out`, `testnorun.out`, and assorted
  scratch artifacts.

## [0.1.0] - unreleased

Initial public release — pending first tag. See `[Unreleased]`.
