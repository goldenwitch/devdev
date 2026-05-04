# DevDev

**A virtual workspace for agents, and an agent host that uses it.**

DevDev is two things in one repo:

1. **[`devdev-workspace`](crates/devdev-workspace/README.md)** — a
   virtual, in-memory, POSIX-ish filesystem that you can mount at a
   real host path and run real host binaries against. A scratch
   environment for agents that looks and feels like a normal
   directory.
2. **`devdev`** — a daemon + CLI that hosts an AI coding agent
   (GitHub Copilot CLI over ACP) inside that workspace, with DevDev
   tools surfaced via an injected MCP server.

Most of this README covers (1), because that is the part that stands
on its own today. (2) is actively being built and honestly described
in [ROADMAP.md](ROADMAP.md).

> ⚠️ **No sandboxing claim.** The workspace is a virtual path, not a
> jail. Processes launched via `Workspace::exec` run as your user, on
> your network, and can reach host files outside the mount if they
> try. Real containment (namespaces, seccomp, job objects) is on the
> roadmap — see [ROADMAP.md](ROADMAP.md).

## Who this is for

- **Workspace-curious.** You're building something with agents and
  want a snapshot-able, throwaway scratch directory that real tools
  (`cargo`, `git`, `rg`, language servers) can operate in. Start at
  [`crates/devdev-workspace/README.md`](crates/devdev-workspace/README.md).
- **DevDev-hosting.** You want to run the full agent product locally
  (PR shepherding, preferences-as-Markdown, approval gates). The
  end-to-end loop — `devdev init` → `devdev repo watch` → agent
  reviews PRs as they appear — works against the mock GitHub
  adapter today; live `gh` posting is gated behind `devdev_ask`
  approvals. See [ROADMAP.md](ROADMAP.md) for what's shipped vs.
  in flight.

## Quickstart: the workspace library

```toml
[dependencies]
devdev-workspace = { git = "https://github.com/goldenwitch/devdev" }
```

```rust
use devdev_workspace::Workspace;
use std::ffi::OsStr;

let mut ws = Workspace::new();
let _mount = ws.mount().expect("mount");

let mut out = Vec::new();
let code = ws.exec(
    OsStr::new("git"),
    &[OsStr::new("init")],
    b"/",
    &mut out,
).expect("exec");
assert_eq!(code, 0);
```

See [`crates/devdev-workspace/README.md`](crates/devdev-workspace/README.md)
for the full tour, platform matrix, and caveats.

## Quickstart: the agent host

Prebuilt binaries for Linux and Windows x86_64 are available on the
[Releases page](https://github.com/goldenwitch/devdev/releases) (once
the first release is cut — see ROADMAP).

From source:

```
cargo install --git https://github.com/goldenwitch/devdev devdev-cli
devdev up                              # starts the daemon
devdev init                            # interview yourself; writes .devdev/*.md
devdev repo watch owner/name           # poll GitHub for PR events
devdev preferences list                # show discovered .devdev/*.md
devdev down                            # stops the daemon
```

DevDev expects a logged-in [GitHub Copilot CLI](https://github.com/github/copilot-cli)
(`copilot --acp` must work) and, for GitHub adapters, either a
`gh auth login` session or a `GH_TOKEN` / `GITHUB_TOKEN` env var.
When the agent wants to post a review or comment it calls the
`devdev_ask` MCP tool; the daemon prompts you for approval and, on
“yes”, hands the agent a short-lived `gh` token to act with.

## Platform matrix

| Platform | Workspace library | Agent host |
|----------|-------------------|------------|
| Linux x86_64 | ✓ (FUSE, standard kernel) | ✓ |
| Windows x86_64 | ✓ (requires [WinFSP](https://github.com/winfsp/winfsp)) | ✓ |
| macOS | — | — |

macOS is not supported in this pass; it needs a third FS driver plus
a containment story. Tracked in ROADMAP.

## Design

Implementation-agnostic narrative under [`spirit/`](spirit/):

- [`spirit/01-concept.md`](spirit/01-concept.md) — why a virtual workspace at all.
- [`spirit/02-workspace-contract.md`](spirit/02-workspace-contract.md) — the `Fs`, mount, exec, and serialization contract.
- [`spirit/03-agent-loop.md`](spirit/03-agent-loop.md) — how ACP, MCP, and the daemon fit together.
- [`spirit/04-tasks.md`](spirit/04-tasks.md) — the task model and the `monitor-pr` exemplar.

Contributor-only history (capability index, phase specs, ACP research)
lives under [`docs/internals/`](docs/internals/).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Windows contributors need [WinFSP](https://github.com/winfsp/winfsp)
installed to run the mount-heavy tests (`cargo test -- --ignored`).
Live Copilot CLI tests are also `#[ignore]` and require a logged-in
Copilot CLI.

Live multi-host integration tests (against real github.com and
Azure DevOps fixtures) are gated behind the
[`live-tests` workflow](.github/workflows/live-tests.yml). The
fixture environment is fully reproducible from
[`test-env/manifest.json`](test-env/manifest.json) — see
[`docs/internals/live-test-fixtures.md`](docs/internals/live-test-fixtures.md)
for bootstrap, principals, and cost. GHE is intentionally absent;
[`docs/internals/ghe-gap.md`](docs/internals/ghe-gap.md) explains
why and how to close it.

## License

MIT. See [LICENSE](LICENSE).
