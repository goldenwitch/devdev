# Spec: Multi-host repo support (GitHub.com, GHE, Azure DevOps)

Status: living document; reflects the state shipped on the
`feature/ado-ghe-support` branch (Phases 1–7).

## Why

DevDev was originally built around a single hard-coded GitHub.com
adapter. Customers on GitHub Enterprise (GHE) and Azure DevOps
(ADO) need first-class support without forking the daemon. This
document captures the abstractions that let one running daemon
serve several hosts simultaneously without any of them shadowing
or impersonating the others.

## The four seams

```
                 PR URL or "owner/repo#N"
                          │
                          ▼
                ┌─────────────────────┐
                │      PrRef          │  carries (host_id, owner, repo, number)
                └──────────┬──────────┘
                           │
              ┌────────────┴────────────┐
              ▼                         ▼
   ┌────────────────────┐   ┌──────────────────────┐
   │  RepoHostId        │   │  CredentialStore     │
   │  (kind/api/host)   │   │  keyed by host_id    │
   └─────────┬──────────┘   └──────────┬───────────┘
             │                         │
             ▼                         ▼
   ┌────────────────────┐   ┌──────────────────────┐
   │ RepoHostRegistry   │   │  AskRequest.host →   │
   │ host_id → adapter  │   │  Credential lookup   │
   └─────────┬──────────┘   └──────────────────────┘
             │
             ▼
   ┌────────────────────┐
   │ RepoHostAdapter    │  github / ghe / azure_devops
   └────────────────────┘
```

1. **`RepoHostId`** (`devdev-integrations::host`) — the routing key.
   `{kind, api_base, host}`. Constructed via `github_com()`,
   `ghe(host)`, `azure_devops()`, or by classifying a browser host
   string with `from_browse_host(host)`. Serialises to a stable
   `<kind>:<host>` ledger key (`github:github.com`,
   `github:ghe.acme.io`, `ado:dev.azure.com`).

2. **`RepoHostAdapter`** (`devdev-integrations`) — the API surface.
   One implementation per forge family. `host_id()` returns its
   `RepoHostId`; the daemon never assumes which one until it asks.

3. **`RepoHostRegistry`** (`devdev-daemon::host_registry`) — the
   adapter lookup table. `for_host(&host_id)` and `for_url(url)`.
   Built once at `devdev up`; immutable thereafter (the same
   lifecycle invariant that protects `CredentialStore`).

4. **`CredentialStore`** (`devdev-daemon::credentials`) — frozen
   token snapshot keyed by `RepoHostId`. Sampled once at boot via
   `CredentialProvider`s (env vars, `gh auth token`,
   `az account get-access-token`). The agent never sees a token
   except via approved `devdev_ask` round-trips, and the response
   only releases the token bound to the requested `host`.

## Key invariants

- **Identity disambiguation.** `(owner, repo)` and
  `(owner, repo, number)` are *not* unique on their own. Every map
  in dispatch (`repo_watch_ids`, `monitor_pr_ids`), every event in
  `DaemonEvent`, and every ledger key includes the `RepoHostId`.
  Cross-host collisions are tested in `events.rs`'s
  `pr_target_disambiguates_by_host` and in scenario S08.
- **Snapshot-once credentials.** `CredentialStore` clones into an
  `Arc<HashMap>` at construction. Mutating the source environment
  after `devdev up` cannot leak into the daemon's auth state. See
  `store_is_immutable_after_snapshot_env_mutation`.
- **Hard rejection on unknown hosts.** Both `repo/watch` and
  `devdev_ask` reject unknown host strings (`-32602` IPC error or
  `AskResponse::Rejected`) rather than silently routing to
  github.com. Silent fallback would be a credential-leakage
  footgun.
- **Default host is github.com.** Clients that don't supply a
  `host` field — including legacy single-host MCP clients —
  resolve to `RepoHostId::github_com()`. This keeps the entire
  pre-multi-host surface wire-compatible.

## URL parsing

`PrRef::parse` accepts:

| Shape                                                        | host_id                  |
| ------------------------------------------------------------ | ------------------------ |
| `owner/repo#N`                                               | `github_com()`           |
| `https://github.com/owner/repo/pull/N[/files]`               | `github_com()`           |
| `https://ghe.example.com/owner/repo/pull/N`                  | `ghe("ghe.example.com")` |
| `https://dev.azure.com/{org}/{project}/_git/{repo}/pullrequest/{id}` | `azure_devops()`         |
| `https://{org}.visualstudio.com/{project}/_git/{repo}/pullrequest/{id}` | `azure_devops()`         |

ADO's three-level identity (`org`/`project`/`repo`) is encoded as
`owner = "{org}/{project}"`, `repo = "{repo}"` to fit the existing
`(owner, repo, number)` adapter surface. The `AzureDevOpsAdapter`
re-splits this on the way out to its REST API.

## Wire surface

### `repo/watch` and `repo/unwatch`

```jsonc
{
  "method": "repo/watch",
  "params": {
    "owner": "platform",
    "repo":  "api",
    "host":  "ghe.acme.io"  // optional; default "github.com"
  }
}
```

The `host` field is the browser-shaped host (no scheme, no path).
Unknown hosts → `-32602`.

### `devdev_ask` (MCP tool)

```jsonc
{
  "kind":    "post_review",
  "summary": "approve PR #42 on the GHE mirror",
  "host":    "ghe.acme.io",   // optional; default "github.com"
  "payload": { ... }
}
```

When the request is approved AND the kind requires a token
(`post_review`, `post_comment`, `request_token`), the response's
`token` field is the credential bound to the requested host —
*never* a different host's token. If no credential is stored for
that host, `token: null` is returned (the agent must surface a
helpful error, not retry on a different host).

## Test landmarks

| Concern                         | Test                                                                                                  |
| ------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `RepoHostId` classification     | `crates/devdev-integrations/src/host.rs` unit tests                                                   |
| `PrRef` URL parsing (all forms) | `crates/devdev-tasks/src/pr_ref.rs` unit tests (16)                                                   |
| Credential snapshot lifecycle   | `crates/devdev-daemon/src/credentials.rs::store_is_immutable_after_snapshot_env_mutation`             |
| Registry routing                | `crates/devdev-daemon/src/host_registry.rs` (8 tests)                                                 |
| Event host-id disambiguation    | `crates/devdev-tasks/src/events.rs::pr_target_disambiguates_by_host`                                  |
| Ask host selector + rejection   | `crates/devdev-daemon/src/mcp/provider.rs::ask_routes_token_by_host_selector`, `ask_unknown_host_is_rejected` |
| End-to-end IPC                  | `crates/devdev-scenarios/tests/scenarios.rs::s08_multi_host_registry_routes_by_host`                  |

## Open follow-ups

- **Preferences-driven registry seeding.** Today `devdev up` seeds
  the registry with one entry: github.com → the default adapter.
  A `[[repo]]` block in `.devdev/preferences.toml` should let
  users register additional GHE/ADO hosts at boot, each with its
  own credential provider chain.
- **Per-host credential provider chains.** `EnvVarProvider` and
  `GhCliProvider` are wired up for github.com only; GHE/ADO hosts
  need their own `EnvVarProvider` (e.g. `GHE_TOKEN_<host>`,
  `AZURE_DEVOPS_PAT`) and `AzCliProvider` instances appended to
  the snapshot at boot.
- **Event coordinator routing.** `ensure_monitor_pr_task` accepts
  a `&RepoHostId` but the only event source today
  (`RepoWatchTask`) only fires for the host its own watch is
  bound to. A future webhook receiver will need to set the
  correct `host_id` on events it publishes.
