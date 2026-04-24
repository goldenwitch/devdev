# Changelog

All notable changes to this project will be documented in this file.
Format inspired by [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [SemVer](https://semver.org/) once the first
release is cut.

## [Unreleased]

### Added
- `devdev-workspace`: standalone crate README covering the library
  entry points, minimal example, and platform matrix.
- `ROADMAP.md`: Today / Next / Aspirational breakdown.
- `SECURITY.md`, `CONTRIBUTING.md`: policy + contributor workflow.
- `rust-toolchain.toml`: pinned compiler.
- MIT `LICENSE`.

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
