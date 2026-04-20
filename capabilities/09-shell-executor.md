---
id: shell-executor
title: "Pipeline Executor & Command Dispatch"
status: done
type: composition
phase: 3
crate: devdev-shell
priority: P0
depends-on: [shell-parser, shell-builtins, tool-registry, virtual-git-commands, vfs-core]
effort: L
---

# 09 — Pipeline Executor & Command Dispatch

The central composition layer. Takes a parsed AST, expands variables and globs, dispatches commands to the right engine (builtin / WASM tool / virtual git), and orchestrates pipeline data flow. This is where the shell "comes alive."

## Scope

**In:**
- Variable expansion: resolve `$VAR`, `${VAR}`, `$?` from shell state
- Glob expansion: expand `*.rs` against VFS
- Pipeline execution: sequential buffer-and-pass (stage N stdout → stage N+1 stdin)
- Redirect handling: `>`, `>>`, `<`, `2>`, `2>>`, `2>&1` against VFS
- Command dispatch: builtins → `try_builtin()`, `git` → `VirtualGit`, else → `ToolEngine`
- Operator semantics: `&&` (short-circuit on failure), `||` (short-circuit on success), `;` (always continue)
- Exit code tracking: `$?` updated after every command
- `ShellSession`: the stateful public API that ACP hooks call

**Out:**
- Parsing (that's `07-shell-parser`)
- Individual tool execution (that's `04-tool-registry` and `06-virtual-git-commands`)
- ACP message handling (that's `12-acp-hooks`)

## Interface

```rust
pub struct ShellSession {
    state: ShellState,                    // cwd, env, $?
    vfs: Arc<RwLock<dyn VirtualFilesystem>>,
    tools: Arc<dyn ToolEngine>,
    git: Arc<dyn VirtualGit>,
}

impl ShellSession {
    pub fn new(
        vfs: Arc<RwLock<dyn VirtualFilesystem>>,
        tools: Arc<dyn ToolEngine>,
        git: Arc<dyn VirtualGit>,
    ) -> Self;
    
    /// Execute a command string. The main entry point.
    pub fn execute(&mut self, command: &str) -> ShellResult;
    
    pub fn cwd(&self) -> &Path;
    pub fn env(&self) -> &HashMap<String, String>;
    pub fn last_exit_code(&self) -> i32;
}

pub struct ShellResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    pub session_ended: bool,  // true if `exit` was called
}
```

## Execution Flow

```
command string
    │
    ▼
┌─────────┐
│  Parse   │  → CommandList AST (from 07-shell-parser)
└────┬─────┘
     │
     ▼
┌──────────────┐
│ For each     │  Iterate (Pipeline, Operator) pairs
│ pipeline:    │
│              │
│  ┌─────────────────┐
│  │ For each stage: │
│  │                 │
│  │  1. Expand vars │  $VAR → value from ShellState.env
│  │  2. Expand globs│  *.rs → VFS glob results
│  │  3. Dispatch:   │
│  │     builtin?────│──► try_builtin(name, args, state, vfs)
│  │     "git"? ─────│──► VirtualGit.execute(args, cwd, vfs)
│  │     else ───────│──► ToolEngine.execute(name, args, stdin, env, cwd, vfs)
│  │  4. Wire stdin  │  Previous stage stdout → this stage stdin
│  │  5. Apply redir │  > file → write stdout to VFS path
│  └─────────────────┘
│              │
│  Pipeline exit code = last stage exit code
│  Update $?
│              │
│  Check operator:
│    && and exit!=0 → skip rest
│    || and exit==0 → skip rest
│    ;             → continue
└──────────────┘
     │
     ▼
  ShellResult
```

## Pipeline Details

**Sequential buffer-and-pass** (v1 strategy):

```
cat file.txt | grep foo | wc -l
```

1. Run `cat file.txt` → capture stdout as `Vec<u8>`.
2. Run `grep foo` with stdin = cat's stdout → capture stdout.
3. Run `wc -l` with stdin = grep's stdout → capture stdout = final result.

Each stage runs to completion before the next starts. Simple, debuggable, correct.

**Redirect handling:**
- `> file` — write final stdout to VFS path `file` (overwrite). Clear stdout from result.
- `>> file` — append final stdout to VFS path `file`. Clear stdout from result.
- `< file` — read VFS path `file` as stdin for the command.
- `2> file` — write stderr to VFS path `file`.
- `2>&1` — stderr merges into stdout.

Redirects on intermediate pipeline stages are legal but rare. Handle them per-stage.

## Variable Expansion

Walk the AST's `Word` parts:
- `WordPart::Literal(s)` → literal string
- `WordPart::Variable(name)` → look up `state.env[name]`, or empty string if missing
- `WordPart::LastExitCode` → `state.last_exit_code.to_string()`
- `WordPart::GlobPattern(pattern)` → expand against VFS. If no matches, keep the literal pattern (matching bash behavior with `nullglob` off).

Expansion happens **after** parsing, **before** dispatch. The dispatched command receives fully resolved string arguments.

## Dispatch Priority

1. **Builtin** — if `try_builtin()` returns anything other than `NotBuiltin`, use it.
2. **Git** — if command name is `"git"`, delegate to `VirtualGit`.
3. **Tool** — delegate to `ToolEngine`. If unknown, ToolEngine returns exit 127.

## Error Handling

- Parse error → `ShellResult` with stderr = parse error message, exit_code = 2, stdout empty
- VFS errors during redirect → stderr = error message, exit_code = 1
- Tool engine returns result (even errors) — executor doesn't interpret exit codes except for `&&`/`||`/`;`

## Files

```
crates/devdev-shell/src/executor.rs    — pipeline execution, redirect handling, operator sequencing
crates/devdev-shell/src/expand.rs      — variable + glob expansion
crates/devdev-shell/src/dispatch.rs    — builtin → git → tool priority chain (+ DispatchCtx)
crates/devdev-shell/src/session.rs     — ShellSession public API
```

Implementation notes:
- `ShellSession` holds `Arc<Mutex<MemFs>>` + `Arc<dyn ToolEngine>` + `Arc<Mutex<dyn VirtualGit>>`. The git mutex is required because `VirtualGit` is intentionally not `Sync` (wraps a raw libgit2 pointer).
- Pipelines use sequential buffer-and-pass as specified.
- Redirects go through `MemFs::read` / `write` / `append`; absolute targets are resolved via `devdev_vfs::path::resolve` + `normalize`.
- Note: the P0 tool registry does not yet preopen VFS paths into WASM sandboxes (see `run_wasm` comment in `crates/devdev-wasm/src/registry.rs`). Shell-visible redirects and builtins work regardless; VFS-aware WASM file args are tracked as a P1 follow-up to light up the `cat file.txt` → WASM path end-to-end.

## Acceptance Criteria

- [ ] `execute("echo hello")` → stdout `"hello\n"`, exit 0
- [ ] `execute("cat file.txt | grep pattern | wc -l")` → correct count
- [ ] `execute("grep foo > out.txt")` → VFS contains `out.txt` with results, stdout empty
- [ ] `execute("echo $HOME")` with env `HOME=/sandbox` → stdout `"/sandbox\n"`
- [ ] `execute("echo $?")` after a failed command → prints previous exit code
- [ ] `execute("echo *.md")` with VFS containing `a.md`, `b.md` → stdout `"a.md b.md\n"`
- [ ] `execute("false && echo nope")` → echo not executed, exit code 1
- [ ] `execute("false || echo yep")` → echo executed, stdout `"yep\n"`
- [ ] `execute("cmd1 ; cmd2")` → both run regardless of exit codes
- [ ] `execute("git log --oneline -3")` → dispatches to VirtualGit, output returned
- [ ] `execute("cd /tmp && pwd")` → stdout `"/tmp\n"`, cwd changed
- [ ] `execute("exit 42")` → `session_ended: true`, exit_code 42
- [ ] `execute("$(bad)")` → parse error in stderr, exit code 2
- [ ] `execute("FOO=bar env")` → `env` sees `FOO=bar` in its environment
