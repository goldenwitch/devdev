---
id: test-harness
title: "End-to-End Test Harness & CLI"
status: done
type: composition
phase: 5
crate: devdev-cli
priority: P0
depends-on: [sandbox-integration]
effort: M
---

# 14 — End-to-End Test Harness & CLI

A minimal `devdev eval` binary that exercises the full pipeline from
cap 13 against real repositories. Not the final daemon — just enough
to prove the sandbox works end-to-end, to iterate on prompt shape, and
to give humans a way to run the system by hand.

## Scope

**In:**
- CLI binary: `devdev eval --repo <path> --task "..."`.
- clap argument parsing with a small, closed option surface.
- Two output modes: human-readable (stdout) and `--json`.
- `tracing-subscriber` wiring driven by `--verbose` and `--trace-file`.
- A **deterministic** acceptance suite that never touches the network:
  clap parse cases, JSON schema snapshot, workspace-limit error path,
  human-output formatting.
- An `#[ignore]`'d E2E suite gated on `DEVDEV_E2E=1` that runs against
  real Copilot. Skipped by default in CI.

**Out:**
- Daemon mode, polling, idempotency ledger.
- Notification system, approval UX.
- Scout / router.
- Fixture repos committed to the DevDev repo — tests seed tempdirs
  programmatically instead.
- Production-quality help text (clap's defaults are fine).

## Local tooling prerequisites

The deterministic suite needs nothing beyond the workspace. E2E tests
additionally require:

- Node.js ≥ 20 on `PATH`.
- `@github/copilot` ≥ 1.0.26 installed globally or under `npx`
  (tests shell out to whichever `copilot` resolves first).
- A GitHub credential for Copilot. Any of the following works:
  - `GH_TOKEN` / `GITHUB_TOKEN` env var containing a **fine-grained** PAT with Copilot scope, or
  - a `gh auth login` OAuth token (set `GH_TOKEN=$(gh auth token)` before running E2E),
  - or a pre-existing `gh auth login` session on the machine — the Copilot CLI reuses gh-CLI credentials transparently. (Validated 2026-04-22 via the P2-06 PoC using a `gho_*` OAuth token from `gh auth token`.)
  - Classic PATs are rejected by the CLI.
- `DEVDEV_E2E=1` in the environment to opt in.

No new tooling scripts are added by this capability. (The
`tools/build-tools.{ps1,sh}` WASM build scripts that this section
originally referenced were removed in the 2026-04-22 Phase 3
consolidation.)

## CLI interface

```
devdev eval [OPTIONS]

Options:
  --repo <PATH>              Path to local repository (required)
  --task <TEXT>              Evaluation task description (required)
  --diff <FILE>              Path to a diff file to include as context
  --preferences <DIR>        Directory of .md preference files
  --workspace-limit <BYTES>  VFS memory cap (default: 2 GiB)
  --timeout <SECONDS>        Session wall-clock timeout (default: 600)
  --json                     Output result as JSON
  --verbose                  Enable DEBUG tracing on stderr
  --trace-file <PATH>        Write TRACE output to file
```

Exit codes:

| Code | Meaning |
|------|---------|
| 0 | Evaluation completed; verdict printed |
| 1 | Evaluation failed (see stderr) |
| 2 | Invalid CLI arguments (clap) |

## Output

### Human-readable (default)

```
Loading repo: /path/to/project (1247 files, 48 MB)
Connecting to Copilot CLI...
Session created: sess_abc123

Agent is evaluating...
  [tool] grep -rn "unwrap()" src/  (exit 0, 0.8s)
  [tool] git blame src/auth.rs     (exit 0, 0.3s)
  [tool] cat src/auth.rs           (exit 0, 0.1s)
  [agent] I found 3 issues...

─── Verdict ───
1. src/auth.rs:42 — Using unwrap() on user input.
2. src/api.rs:15 — SQL query via string concatenation.
3. src/config.rs:8 — Hardcoded API key.

Evaluation complete (12 tool calls, 34.2s)
```

The human output is considered unstable for scripting — `--json` is the
stable contract.

### JSON

```json
{
  "verdict": "3 issues found...",
  "stop_reason": "end_turn",
  "tool_calls": [
    { "command": "grep -rn \"unwrap()\" src/", "exit_code": 0, "duration_ms": 823 },
    { "command": "git blame src/auth.rs", "exit_code": 0, "duration_ms": 312 }
  ],
  "duration_ms": 34200,
  "is_git_repo": true,
  "repo_stats": { "files": 1247, "bytes": 50331648 }
}
```

The shape is pinned by a snapshot test in the deterministic suite.

## Tracing

`tracing-subscriber::fmt` configured per flags:

- default → `WARN` to stderr.
- `--verbose` → `DEBUG` to stderr.
- `--trace-file <path>` → `TRACE` to the file, regardless of `--verbose`.

Expected event shapes:

```
[TRACE] vfs::load files=1247 bytes=50331648 duration_ms=1200
[TRACE] acp::init protocol_version=1 agent="copilot-cli/1.0.26"
[TRACE] acp::session id="sess_abc123"
[DEBUG] acp::update type="agent_message_chunk"
[DEBUG] acp::update type="tool_call" tool="grep" status="pending"
[TRACE] acp::terminal_create id="term_001" command="grep -rn unwrap() src/"
[TRACE] shell::execute command="grep -rn unwrap() src/" exit_code=0 duration_ms=823
[TRACE] acp::prompt_complete stop_reason="end_turn"
```

## Test split

### Deterministic (always run)

These build on cap 13's scripted fake-agent harness — no real Copilot.

| Test | What it proves |
|------|----------------|
| `clap_parses_minimum_args` | `--repo` + `--task` only → valid config |
| `clap_rejects_missing_required` | missing `--repo` → exit 2 |
| `clap_rejects_invalid_workspace_limit` | non-numeric → exit 2 |
| `workspace_limit_prints_clean_error` | tiny limit → "repo too large" on stderr, exit 1, no subprocess spawned |
| `json_output_matches_snapshot` | fake-agent run with `--json` → output parses and matches a pinned schema |
| `human_output_lists_each_tool_call` | fake-agent run without `--json` → every tool call from `EvalResult.tool_calls` appears in stdout, in order |
| `verbose_flag_enables_debug_tracing` | `--verbose` + fake agent → stderr contains at least one `DEBUG` record |
| `trace_file_written` | `--trace-file` → file exists, non-empty, contains `acp::init` |

### E2E (opt-in, `#[ignore]`)

Gated on `DEVDEV_E2E=1` + `GH_TOKEN` + `copilot` on `PATH`. These
exercise the real agent and are slow.

| Test | What it proves |
|------|----------------|
| `e2e_simple_eval` | Tempdir seeded with 3 files + trivial task → non-empty verdict, `stop_reason == "end_turn"` |
| `e2e_tool_execution` | Verdict mentions file content the agent could only have seen via `terminal/create` |
| `e2e_file_modification` | Agent is prompted to write `/NOTES.md`; VFS snapshot after shows the file |
| `e2e_git_operations` | Tempdir with a git repo; agent runs `git log`, verdict quotes a real commit subject |
| `e2e_timeout_graceful` | `--timeout 5` on a long task → exits 1 with `Timeout`, no orphaned child process |

E2E tests seed their repos programmatically using `tempfile::tempdir`
+ `git2` (already a workspace dep). Nothing is committed under
`tests/fixtures/`.

## Files

```
crates/devdev-cli/src/main.rs       — clap, dispatch, exit codes
crates/devdev-cli/src/output.rs     — human + JSON formatters
crates/devdev-cli/src/tracing.rs    — subscriber setup
crates/devdev-cli/tests/acceptance_cli.rs   — deterministic suite
crates/devdev-cli/tests/e2e.rs              — #[ignore]'d E2E suite
```

No new directories under `tests/` at the workspace root; everything
lives next to the crate it covers.

## Acceptance Criteria

Deterministic (must pass in CI with no env vars):

- [ ] **AC-01** `clap_parses_minimum_args` — `devdev eval --repo . --task "x"` yields a valid config.
- [ ] **AC-02** `clap_rejects_missing_required` — missing `--repo` exits 2 with usage on stderr.
- [ ] **AC-03** `workspace_limit_prints_clean_error` — `--workspace-limit 16` on a tempdir with >16 bytes of content exits 1 with a one-line "repo too large" message; no subprocess was spawned.
- [ ] **AC-04** `json_output_matches_snapshot` — fake-agent run with `--json` parses as JSON and every documented field is present with the expected type.
- [ ] **AC-05** `human_output_lists_each_tool_call` — fake-agent run without `--json` prints each tool command in order and a final "Evaluation complete" line.
- [ ] **AC-06** `verbose_flag_enables_debug_tracing` — `--verbose` produces at least one `DEBUG` line on stderr.
- [ ] **AC-07** `trace_file_written` — `--trace-file` produces a non-empty file containing an `acp::init` record.

E2E (opt-in, `#[ignore]`):

- [ ] **AC-E1** `e2e_simple_eval` passes against real Copilot.
- [ ] **AC-E2** `e2e_tool_execution` — verdict reflects tool-observed content.
- [ ] **AC-E3** `e2e_file_modification` — VFS contains the written file.
- [ ] **AC-E4** `e2e_git_operations` — verdict references real commit data.
- [ ] **AC-E5** `e2e_timeout_graceful` — short timeout returns cleanly, no zombies.
