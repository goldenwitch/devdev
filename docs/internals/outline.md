# DevDev: Architecture & Experience Specification

**Values:** Empiricism. Brevity. Wit.
**Mission:** A portable, daemonized agent for the developer's brain. DevDev silently monitors workflows and uses a headless Copilot CLI sandbox to enforce personal technical boundaries, intervening only when necessary.

---

## 🔒 Locked Decisions (The "100% Clear" List)

### 1. The Vibe Check (Configuration)
* **Format:** Preferences are stored as standard Markdown files (`.md`), not rigid YAML or custom DSLs. 
* **Creation:** Initialized via a natural language conversation. The system acts as a scribe, translating the developer's "vibes" into distinct, file-scoped preference documents.

### 2. The Two-Stage Router (Lazy Loading Context)
* **The Scout (Lightweight LLM):** Evaluates an incoming event (e.g., a PR diff) and generates a list of file pointers (paths) referencing the specific preference files that apply to that context.
* **The Heavy (Copilot CLI via ACP):** The Copilot CLI is spawned in **ACP (Agent Communication Protocol) mode** — a structured JSON-based RPC protocol over stdio. DevDev sends the event context and preference file pointers to the CLI, which reasons about them using its native tool-calling. Tool calls are intercepted via ACP hooks and routed through the virtual execution engine. This doesn't mean it can't access other files and preferences if it thinks they are relevant.

### 3. The Sandbox (Execution Engine)
> **Architecture note (2026-04-22, post-Phase-3):** The four bullets below describe the original design — a pure-in-memory VFS with WASM-compiled coreutils and an in-memory libgit2. That architecture shipped in Phases 1-2, then was **consolidated** in Phase 3 into a single `devdev-workspace` crate that mounts the in-memory `Fs` as a real OS filesystem (FUSE on Linux, WinFSP on Windows). The agent now runs native host tools (`grep`, `sed`, `git`, …) against the mount, not WASM re-implementations. The user-visible contract — "agent operates in a bounded workspace that is discarded on completion" — is preserved; the mechanism changed. The deleted specs are retained under `spirit/spec-virtual-*.md` and `spec-{wasm,shell}-*.md` with historical banners.
* **Pure Virtual Workspace:** The agent operates inside a fully virtualized environment — a pure in-memory filesystem with its own execution engine. No host filesystem interaction. The target repo is loaded into memory at evaluation start and discarded on completion.
* **In-Memory Filesystem:** All file operations (read, write, stat, list, glob) are served from a pure in-memory filesystem. The repo snapshot is loaded into this space. The agent can freely create, modify, and delete files without any risk to the host or real repository.
* **Virtual Shell & Toolchain:** The agent's shell commands (the bash-like commands it was trained with) are intercepted and executed by a built-in shell parser and tool execution engine. Core utilities (grep, find, cat, ls, sed, etc.) are compiled to portable WebAssembly modules and run against the in-memory filesystem. Pipes, redirects, globs, and environment variables work as the agent expects.
* **Git as a First-Class Virtual Tool:** Git operations (diff, log, status, blame, etc.) are provided as a built-in virtual command backed by a native git library operating directly on the in-memory filesystem. The agent can inspect repository history and structure without shelling out to a real `git` binary.
* **Workspace Size Limit:** The in-memory workspace defaults to a **2 GB** cap. Users can override this with a `--workspace-limit` flag for larger monorepos.
* **Copilot CLI Integration (ACP):** DevDev spawns the GitHub Copilot CLI as a subprocess in ACP (Agent Communication Protocol) mode — a structured, versioned RPC protocol over stdio. The prod invocation is `copilot --acp --allow-all-tools`: the CLI runs its own tool bundle (shell, fs, web) directly against the mounted workspace, and DevDev observes work via ACP session updates rather than intercepting every tool call. (Per-tool interception via ACP client capabilities — `terminal/*`, `fs/*` — is still implemented and available behind a hypothetical `--strict-sandbox` profile; see [capability 12](capabilities/12-acp-hooks.md).) DevDev-specific tools (task queries, ledger lookups, preference file inventory) are surfaced via MCP rather than ACP hooks; see [capability 28](capabilities/28-mcp-tool-injection.md). No PTY hacking, no terminal escape sequence parsing — clean JSON in, clean JSON out. Authentication in daemon mode: an existing `gh auth login` session is typically sufficient (the Copilot CLI reuses gh-CLI credentials transparently); `GH_TOKEN` / `GITHUB_TOKEN` env vars are also honoured and can hold either a fine-grained PAT or a gh-CLI OAuth token.
* **Language Runtimes (Future):** Running language-specific tools (linters, type-checkers, test runners) inside the virtual workspace is a high-value goal under active exploration. The architecture is designed to accommodate additional execution backends over time.

### 4. The Silent Watcher (Daemon & UX)
* **Idempotency:** A background polling loop checks for state changes (PRs, Jira tickets). A local ledger ensures DevDev never evaluates or complains about the same exact commit or ticket state twice.
* **Private Intervention:** DevDev never acts as a dictator to the team. Rule violations result in a private, asynchronous notification to the user (e.g., a drafted PR comment) requiring explicit 1-click approval to deploy. Users can run with a --rude flag to disable approval.

---