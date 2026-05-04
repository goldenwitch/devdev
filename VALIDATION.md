# Proving it works

This repository claims things. Some are subtle enough that a skeptic
can't just read the README and believe them. This file points at the
machinery that turns each claim into a reproducible test.

## What's claimed

See [`claims.toml`](claims.toml). Each `[[claim]]` row:

- names a claim by id,
- anchors it to a source file + line (what prose is being proven),
- lists the env vars and host tools required to run the test,
- names the exact `cargo test` invocation that validates it.

The rubric every row must satisfy lives in
[`spirit/05-validation.md`](spirit/05-validation.md): real path, no
tautologies, no motte-and-bailey.

## Running it

From the repo root:

```pwsh
pwsh -File scripts\validate.ps1
```

PowerShell is [cross-platform](https://learn.microsoft.com/en-us/powershell/scripting/install/installing-powershell);
the same script runs on Windows, Linux, and macOS. One runner,
one surface — adding a bash twin would double the maintenance on
every change.

Claims whose env prerequisites aren't set are **skipped with a
message naming the missing variable** — not silently passed. To run
the live-Copilot claim:

```pwsh
$env:DEVDEV_LIVE_COPILOT = '1'
pwsh -File scripts\validate.ps1
```

You must additionally have a signed-in `copilot` on `PATH`, and on
Windows, [WinFSP](https://github.com/winfsp/winfsp) installed. The
runner adds WinFSP's `bin\` to `PATH` on Windows automatically so
the DLL delay-load resolves.

## Current claims

| id | what it proves | gate |
|---|---|---|
| `AGENT-FS-WRITE` | A live Copilot session's tool calls update the mounted workspace Fs, verified through both the host mount and the Fs directly. | `DEVDEV_LIVE_COPILOT=1` |
| `DAEMON-AGENT-FS-WRITE` | A `devdev up` daemon routes a live Copilot session through an injected MCP tool to mutate daemon-owned Fs state. | `DEVDEV_LIVE_COPILOT=1` |
| `FIXTURE-MANIFEST-INTEGRITY` | The CI-resettable live-test fixture manifest enforces its structural invariants and the `reset-comments` keep/delete decisions are correct (deterministic side; the fixture-state-matches-manifest side runs in CI only). | none |
| `LIVE-HOST-PROBE-GH` | `GitHubAdapter` round-trips the canonical fixture PR through real github.com REST. | `DEVDEV_LIVE_HOSTS=1` + fixture env |
| `LIVE-HOST-PROBE-ADO` | `AzureDevOpsAdapter` round-trips the canonical fixture PR through real `dev.azure.com` REST. | `DEVDEV_LIVE_HOSTS=1` + fixture env |
| `LIVE-CREDENTIAL-CHAIN-GH` | `GhCliProvider` produces a non-empty token from a real signed-in `gh` CLI, stamped with `TokenSource::GhCli`. | `DEVDEV_LIVE_CRED_GH=1` |
| `LIVE-CREDENTIAL-CHAIN-ADO` | `AzCliProvider` produces a non-empty AAD token from a real signed-in `az` CLI for the ADO resource. | `DEVDEV_LIVE_CRED_AZ=1` |
| `LIVE-ADO-PR-WRITE` | `AzureDevOpsAdapter::post_comment` lands a tagged comment on the canonical PR; `list_pr_comments` sees it. Cleanup removes it. | `DEVDEV_LIVE_HOSTS=1`, `DEVDEV_LIVE_WRITE=1` + fixture env |

## Live tests in CI

The four-stage live-tests pipeline lives in
[`.github/workflows/live-tests.yml`](.github/workflows/live-tests.yml).
Manual `workflow_dispatch` + nightly cron + label-gated PRs. The
fixture environment it provisions is documented in
[`docs/internals/live-test-fixtures.md`](docs/internals/live-test-fixtures.md);
the deliberate GHE gap and how to close it is documented in
[`docs/internals/ghe-gap.md`](docs/internals/ghe-gap.md).

The list is deliberately short. Adding a claim means writing a real
test that clears the rubric — not padding the manifest.
