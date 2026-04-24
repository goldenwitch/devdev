# Contributing to DevDev

Thanks for your interest. DevDev is early; the most valuable kinds of
contribution right now are:

- Bug reports for the workspace library on Linux or Windows.
- Scenarios that fail — either new ones you think should pass, or
  existing ones that regress.
- Feedback on the [roadmap](ROADMAP.md): is the Next list the right
  next list?
- Containment work (see ROADMAP's aspirational section — we'd love
  help there).

## Prerequisites

- Rust toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml).
  `rustup` will pick it up automatically.
- **Linux:** FUSE is standard; nothing extra.
- **Windows:** install [WinFSP](https://github.com/winfsp/winfsp).
  Runtime is enough for `cargo test` (mount-heavy tests are gated
  `#[ignore]` so they're off by default anyway).
- **Live Copilot CLI tests** (also `#[ignore]`): require a logged-in
  [GitHub Copilot CLI](https://github.com/github/copilot-cli). Run
  `copilot --version` to verify.

## The standard loop

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must pass before you open a PR. CI runs the same three on
Linux + Windows x86_64.

To include the gated tests locally:

```
cargo test --workspace -- --ignored
```

Those will fail on machines without WinFSP or a logged-in Copilot CLI
— that's expected.

## Scenario harness

User-surface scenarios live in
[`crates/devdev-scenarios/`](crates/devdev-scenarios/). They drive
only the `devdev` binary + its IPC protocol + checkpoint files +
documented env vars — never engine internals. See
[`crates/devdev-scenarios/catalog/README.md`](crates/devdev-scenarios/catalog/README.md)
for the contract and
[`docs/internals/testing.md`](docs/internals/testing.md) for the
layered testing philosophy.

If you're adding behavior that a user would notice, please add or
update a scenario.

## Commit conventions

- **Subject:** `area: short imperative summary` (e.g.
  `workspace: fix O_APPEND offset on rename`).
- **Body:** what changed and why. Wrap at ~72 cols.
- **Trailer:** if Copilot co-authored, add
  `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>`.

## Design docs

- [`spirit/`](spirit/) — user-facing architecture narrative. Four
  files, each under 300 lines, implementation-agnostic.
- [`docs/internals/`](docs/internals/) — contributor-only history:
  the 21-file capability index, phase specs, ACP research.

If your change alters a behavior described in `spirit/`, update
`spirit/` in the same PR. Internals docs drift is fine; we'll clean
it up periodically.

## Questions

Open a discussion or a draft issue. We'd rather talk about shape
before code lands.
