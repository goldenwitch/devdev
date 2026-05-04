# GitHub Enterprise — the gap and how to close it

DevDev's `RepoHostId` plumbing, `RepoHostAdapter` trait, and
`RepoHostRegistry` were designed to support **GitHub Enterprise
Server** (GHE) on equal footing with GitHub.com and Azure DevOps.
The code paths are real:

- A single `GitHubAdapter` impl serves both github.com and GHE; the
  `RepoHostId::api_base` field switches between
  `https://api.github.com` and `https://<host>/api/v3`.
- `PrRef::parse` recognises GHE-shaped URLs and threads the right
  `RepoHostId` through to the dispatch maps and event bus.
- `RepoHostRegistry::for_url` classifies arbitrary GHE hosts via
  the `ghe.*` heuristic + explicit registration.
- `CredentialStore` keys on `RepoHostId`, so per-GHE-host tokens
  (`GHE_TOKEN_<host>` env vars) drop in cleanly.

What we **do not** have today is a live test against an actual GHE
instance. This document explains why, what we still rely on for
GHE confidence, and what it would take to close the gap.

## Why GHE isn't in CI today

Three options exist, and all three have costs we're choosing not
to absorb in the first cut:

1. **Self-hosted GHE VM in CI**
   - Highest fidelity (real on-prem product).
   - Requires a GHE license, a host with ~32 GB RAM, the operational
     burden of upgrades, backups, and TLS certificates.
   - Not justifiable for a project at our current scale.
2. **GHE.com / data residency**
   - A real GHE-classified host suffix (`*.ghe.com`) on
     GitHub-managed infrastructure.
   - Same wire protocol as on-prem.
   - Paid plan; pricing scales with seat count.
3. **Record/replay (VCR-style)**
   - Capture real GHE responses once; replay in CI.
   - Cheap, fast, but no longer a *live* test — adapter changes
     against newer GHE versions go undetected until the next
     re-record.
   - Violates the "real path" rule from
     [`spirit/05-validation.md`](../../spirit/05-validation.md).

We picked option 4: **explicitly skip GHE in CI**, document the
gap, and lean on adapter unit tests + URL-parsing tests +
`host_id`-plumbing identity proofs.

## What we still rely on for GHE confidence

| Coverage | Where |
|---|---|
| URL classification (`ghe.*` heuristic, `from_browse_host` round-trip) | `crates/devdev-integrations/src/host.rs` (8 unit tests) |
| `PrRef::parse` accepts GHE URLs and produces the right `RepoHostId` | `crates/devdev-tasks/src/pr_ref.rs` (16 unit tests) |
| `GitHubAdapter::ghe(host, token)` constructor sets `api_base` correctly | `crates/devdev-integrations/src/github.rs` (acceptance) |
| `RepoHostRegistry::for_url` routes GHE URLs to the right adapter | `crates/devdev-daemon/src/host_registry.rs` (8 unit tests) |
| Dispatch keys disambiguate `(owner, repo)` collisions across hosts | `crates/devdev-tasks/src/events.rs` (`pr_target_disambiguates_by_host`) |
| End-to-end IPC routing across hosts (mock GHE adapter) | `crates/devdev-scenarios/tests/scenarios.rs` (`s08_multi_host_registry_routes_by_host`) |

What this catches:

- Adapter boilerplate diverging between github.com and GHE codepaths.
- URL parsing regressions when someone refactors `RepoHostId`.
- Ledger-key collisions across hosts.

What this does **not** catch:

- A real GHE instance returning a slightly different response shape
  (rare, but historically possible during major-version rollouts).
- TLS chain issues against on-prem GHE installs with private CAs.
- Auth-header dialect shifts (e.g. if GHE adds a new required
  header in a future major release).

These are the risks the "live GHE" tests would close. They're real,
but bounded.

## What it would take to close the gap

If a contributor or sponsor wants this:

1. **Pick a GHE flavour** — GHE.com data-residency tier is the
   easiest provisioning path; a self-hosted VM is the most
   faithful.
2. **Provision a fixture there.** The `devdev-test-env` crate
   already abstracts the manifest; add a `ghe` block alongside
   `github` and `azure_devops` in
   [`test-env/manifest.json`](../../test-env/manifest.json).
   Reuse `GithubClient` (point it at the GHE API base) for `apply`.
3. **Wire CI.** Add a `live-tests-ghe` job to
   [`.github/workflows/live-tests.yml`](../../.github/workflows/live-tests.yml).
   Gate it on a `LIVE_GHE_HOST` repository variable so the same
   workflow handles "GHE configured" and "GHE not configured".
4. **Add a live test.** A GHE-flavoured twin of the planned
   `live_ado_pr` test (read-only PR fetch, host-id assertion,
   write-mode-gated comment round-trip).

Estimated work: a day for the IaC + workflow plumbing, plus
ongoing license / runtime cost for the GHE instance itself.

## How to volunteer

If your org runs a GHE instance and you would be willing to:

- Provision a small fixture org/repo on it, scoped to a
  long-lived bot account.
- Issue a fine-grained PAT for that bot, rotated on a 90-day
  cadence.
- Allow inbound traffic from GitHub Actions runners (or run a
  self-hosted runner inside your network).

…open an issue tagged `live-tests:ghe` and we'll work the rest
together. We'd rather have one real GHE in CI for one quarter
than recorded fixtures forever.

The same is true for any **other** repo host we don't currently
test live (Bitbucket, Gitea, Forgejo) — the abstraction is built
to extend, and a contributor sponsorship is the missing piece.
