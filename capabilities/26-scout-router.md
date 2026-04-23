---
id: scout-router
title: "Scout — Two-Stage Preference Router"
status: not-started
type: composition
phase: 5
crate: devdev-tasks
priority: P1
depends-on: [vibe-check, session-router]
effort: L
---

# P5-02 — Scout (Two-Stage Preference Router)

The second pillar from `spirit/outline.md` §2. Before the Heavy (Copilot via ACP) sees an event, a lightweight LLM ("the Scout") inspects the event and emits a list of `.devdev/*.md` file pointers — only the preferences that apply. The Heavy then loads those files into its session context. Cheap routing → expensive reasoning, lazily loaded.

## Scope

**In:**
- `Scout` trait: `fn route(event: &Event, prefs: &PreferenceIndex) -> Vec<PathBuf>`.
- One concrete implementation: `CopilotScout` using a small/fast Copilot model (or whatever the cheapest available ACP-speakable model is — TBD by PoC).
- `PreferenceIndex`: lightweight scan of `.devdev/*.md` titles + first-paragraph summaries, fed to the Scout as part of its prompt.
- Wiring: `MonitorPrTask` (P2-07) calls Scout with the PR event before opening its session; passes returned file paths into the session context.
- `devdev send` / interactive chat: same flow — every user message gets routed through Scout if `.devdev/` exists.

**Out:**
- Local LLMs. Use Copilot for both Scout and Heavy initially. Cost optimization is Phase 6.
- Caching / memoization of Scout decisions. Idempotency-ledger (P5-03) handles "have we seen this event" — Scout always re-routes for fresh events.
- Bypassing Scout (always loading all preferences). If `.devdev/` is small (< N files), Scout could be skipped, but that's an optimization.

## PoC Requirement (Spec Rule 2)

Critical decisions to validate before locking the design:

1. **Which Copilot model is "lightweight"?** Run an A/B: routing accuracy vs latency vs cost across the available models. Pick the smallest that gets routing right ≥ 95% on a hand-labeled fixture set.
2. **Output schema:** structured JSON list of paths, or free-form text the Scout parses? Structured is cleaner; free-form might be all the small model can do reliably.
3. **Failure mode:** Scout returns nothing → load all preferences? Load none? Default conservative: load all.

## Open Questions

- Is the Scout a separate ACP session, or a single-shot prompt? Probably single-shot per event (no multi-turn), which means a different code path from the Heavy session.
- Latency budget? Outline doesn't say. Working assumption: < 1s p50.
- How does the user override? `devdev send --prefs all "..."` to skip Scout, `--prefs none` to skip preferences entirely. Scriptable for testing.

## Dependencies

- **P5-01 (vibe-check)** — Scout has nothing to route to without `.devdev/` files.
- **P2-06 (session-router)** — even though Scout is single-shot, it shares the ACP transport machinery.

## Acceptance Criteria

- Given a fixture `.devdev/` with 5 preference files and a fixture PR diff that should trigger 2 of them, Scout returns exactly those 2 paths (deterministic test using a mocked Scout, plus a live test gated on `DEVDEV_E2E`).
- Scout latency p95 ≤ chosen budget on a representative event mix.
- `devdev send --prefs all` and `--prefs none` correctly bypass Scout in both directions.
- `MonitorPrTask` integration test: Scout-routed prefs appear in the session prompt; non-routed ones don't.

## Why Now (or Not Yet)

This is what makes DevDev scale beyond toy preference sets. Without Scout, every Heavy invocation either drowns in irrelevant context or silently misses the relevant rule. Build after P5-01 (Scout needs files) and after P2-07 lands the first real task that benefits from routing.
