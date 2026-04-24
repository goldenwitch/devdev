---
id: mcp-tool-injection
title: "MCP Tool Injection (DevDev-specific tools)"
status: done
type: leaf
phase: 5
crate: devdev-daemon
priority: P1
depends-on: [session-router]
effort: L
---

# P5-03 — MCP Tool Injection

Surface DevDev's internal state (tasks, idempotency ledger, cross-repo navigation, PR status) to the Copilot CLI as callable tools via the **Model Context Protocol** (MCP). Without this, DevDev-specific context only reaches the agent through prompt text — which is unreliable for state the agent needs to *query* (e.g. "is this PR already in the ledger?" or "list other tasks monitoring this repo").

## Why this is its own capability

The P2-06 PoC (2026-04-22) surfaced that the Copilot CLI runs in `--allow-all-tools` mode on the prod path. In that mode Copilot runs its own tool bundle (shell, fs, web, etc.) directly against the mounted workspace — our `AcpHandler` tool/fs/permission hooks never fire. The `initialize` response advertised `mcpCapabilities: { http, sse }` as the agent's tool-injection surface.

So: if DevDev wants to teach the agent about *DevDev-specific* things, the path is MCP, not ACP hooks, and not `AcpHandler`.

## Scope

**In:**
- In-process MCP server (HTTP or stdio — whichever Copilot's `mcpServers` config in `session/new` params accepts first).
- `NewSessionParams.mcp_servers` populated from `AcpSessionBackend` when creating a session so Copilot sees the DevDev tool bundle.
- Initial tool set (small — keep it focused):
  - `devdev_tasks_list` — active tasks for this daemon.
  - `devdev_ledger_seen` — has DevDev already evaluated this `(adapter, resource, state_hash)`?
  - `devdev_prefs_list` — inventory of `.devdev/*.md` preference files (read-only; Copilot fetches file bytes via its own fs tools against the mount).
- MCP auth: local only, single-user. No network exposure. Bearer-token on loopback HTTP, or a pipe path for stdio.
- Tool output schema: small JSON objects; prefer arrays of `{id, summary}` over free-form text.

**Out:**
- Arbitrary user-defined tools. Keep the surface closed until the need is concrete.
- Remote MCP servers. Local only.
- Tool-level permission prompts. Everything in the bundle is read-only first draft.
- Streaming tool output.

## PoC Requirements (Spec Rule 2)

Before implementing the server, two open questions had to resolve:

1. **Transport fidelity:** Does the Copilot CLI, when launched with `mcpServers` in `session/new`, reliably connect to a local HTTP MCP server? Or does it only support stdio MCP?
2. **Advertising vs. calling:** Does the Copilot CLI automatically expose MCP tools to the agent, or does it require the prompt to mention them?

If either failed, fall back to injecting the same data as prompt preamble via `SessionContext.prior_observations` — lossy but always works.

**PoC Result (2026-04-22): PASS on both.** Scripts in `target/tmp/poc-mcp/` (`server.mjs`, `run_final.mjs`).

- **Transport:** Native **Streamable HTTP** (single-endpoint POST + GET SSE handshake). Copilot's `initialize` against the MCP server declared `protocolVersion=2025-11-25` — newer than public spec 2025-06-18. Using the official `@modelcontextprotocol/sdk` v1.29 server, the pair converged cleanly. Legacy HTTP+SSE split-endpoint mode was not needed. Stdio works too but was not exercised.
- **mcpServers schema:** Copilot rejected flat `{name, url}` with a Zod validation error. The accepted shape is a **discriminated union on `type`**:
  ```json
  { "name": "devdev", "type": "http",
    "url": "http://127.0.0.1:PORT/mcp",
    "headers": [ { "name": "Authorization", "value": "Bearer <tok>" } ] }
  ```
  `headers` is **array-of-`{name,value}`**, not a map. Stdio variant uses `{name, command, args, env}` (env same shape as headers); `sse` mirrors `http`.
- **Auto-discovery:** Once the server is reachable, Copilot calls `tools/list` on its own and invokes tools in response to natural-language prompts — no tool-name hint required. Prompt "list the DevDev tasks currently running" triggered `devdev_tasks_list` unbidden and the reply paraphrased the tool's JSON output.
- **Bearer auth:** Copilot faithfully forwards `headers` on every MCP request. Wrong-bearer test yielded 401 → Copilot probed `/.well-known/oauth-protected-resource*` and OAuth discovery endpoints (≈6 strays) before giving up gracefully and falling back to its built-in tools; session did not crash. Recommend stubbing those well-known endpoints to return 404 fast.
- **Additional findings:** user-level config at `~/.copilot/mcp-config.json` uses a different object-form (`mcpServers: { <name>: { type, url, headers: {k:v}, tools: [...] } }`) — do NOT confuse with the array-form ACP passes at `session/new`. CLI also has `--additional-mcp-config @path` and `--disable-builtin-mcps` flags; per-session injection is strictly better for our multi-tenant daemon.
- **Type surgery landed:** [crates/devdev-acp/src/types.rs](../crates/devdev-acp/src/types.rs) `McpServerConfig` is now a `#[serde(tag = "type")]` enum over `Http | Sse | Stdio` with an `McpHeader { name, value }` helper. `{name, url}` flat struct is gone.

## Interface

```rust
// crates/devdev-daemon/src/mcp/mod.rs
pub struct McpServer {
    addr: SocketAddr,           // loopback only (127.0.0.1 + ephemeral port)
    bearer_token: String,       // per-daemon-run, 32+ random bytes
    tools: Arc<dyn McpToolProvider>,
}

#[async_trait]
pub trait McpToolProvider: Send + Sync {
    async fn list_tools(&self) -> Vec<ToolDefinition>;
    async fn call_tool(&self, name: &str, args: serde_json::Value)
        -> Result<serde_json::Value, McpError>;
}

pub struct DaemonToolProvider {
    tasks: Arc<Mutex<TaskRegistry>>,
    ledger: Arc<dyn IdempotencyLedger>,
    workspace: Arc<dyn Workspace>,  // for prefs list
}
```

Recommended implementation: **`rmcp` crate** (official Rust MCP SDK, `modelcontextprotocol/rust-sdk`) which ships a Streamable HTTP server transport. Fall back to a hand-rolled `axum` handler only if `rmcp` drags unwanted deps.

The `AcpSessionBackend` then populates `NewSessionParams.mcp_servers` with one `McpServerConfig::Http { name, url, headers: [Authorization: Bearer …] }` entry pointing at the daemon's MCP endpoint.

## Open Design Questions

- **Token lifecycle:** Regenerate bearer on each `devdev up`, or persist across restarts? Lean toward regenerate — shorter blast radius if the token leaks.
- **Tool versioning:** When we add a tool in a later release, does Copilot cache the old tool list from a checkpoint-resumed session? If yes, we need to force-rebuild the MCP config on `session/new`. Mitigated partly by always regenerating the server config per-session.
- **Does this replace prompt preamble entirely?** Probably not — MCP tools are pull-driven (agent decides to call). Some context is useful even when the agent didn't know to ask. Keep both paths; prompt preamble carries "what am I doing right now", MCP carries "what can I ask DevDev about".

## Spec Requirements

Sourced from 2026-04-22 post-PoC alignment review — no spec section yet; this capability is *adding* a missing pillar.

| Req | Description |
|-----|-------------|
| MCP-1 | DevDev runs a local MCP server while the daemon is up |
| MCP-2 | Every Copilot session launched by `AcpSessionBackend` receives the MCP endpoint via `NewSessionParams.mcp_servers` |
| MCP-3 | Initial tool surface: `devdev_tasks_list`, `devdev_ledger_seen`, `devdev_prefs_list` |
| MCP-4 | MCP server binds loopback only, bearer-auth'd |
| MCP-5 | Adding a new tool does not break sessions already in flight |

## Acceptance Tests

- [ ] `mcp_server_starts_on_daemon_up` — bind loopback, bearer set, tools discoverable
- [ ] `mcp_tool_list_matches_provider` — `/tools` endpoint returns exactly the provider's definitions
- [ ] `mcp_tool_call_roundtrip` — call `devdev_tasks_list`, receive array of known task ids
- [ ] `mcp_auth_rejects_missing_bearer` — 401 without the right header
- [ ] `acp_session_gets_mcp_config` — `AcpSessionBackend::create_session` sends `mcp_servers` in `session/new`
- [ ] `mcp_tool_appears_in_live_session` (E2E, gated) — live Copilot session can list and invoke a DevDev tool

## Files

```
crates/devdev-daemon/src/mcp/mod.rs        — McpServer, McpToolProvider trait
crates/devdev-daemon/src/mcp/http.rs       — axum/hyper HTTP transport (pending PoC)
crates/devdev-daemon/src/mcp/tools.rs      — DaemonToolProvider impl
crates/devdev-cli/src/acp_backend.rs       — populate NewSessionParams.mcp_servers
```
