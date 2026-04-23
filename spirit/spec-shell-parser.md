# Spec: Shell Parser & Pipeline Engine

> **⚠️ HISTORICAL — describes the pre-Phase-3 architecture.** This spec describes a hand-rolled bash-subset parser that drove the WASM tool engine against the in-memory VFS. Both collaborators (the WASM engine and the in-memory VFS) were deleted during the Phase 3 consolidation (2026-04-22); the agent now uses a real host shell (via `Workspace::exec`) inside the FUSE/WinFSP mount. Retained for design-history context; **do not use as a spec for current or future work.**

**Status:** Historical — superseded by host-shell-in-mount.
**Original status:** Draft
**Depends on:** Virtual Filesystem (spec-virtual-filesystem.md), WASM Tool Engine (spec-wasm-tools.md)

---

## Purpose

Parse bash-like command strings issued by the agent and orchestrate their execution through the WASM tool engine. The agent was RL-trained on bash — this layer exists to honor that training by providing a familiar shell surface without running an actual shell on the host.

---

## Requirements

### Supported Syntax

The parser must handle the subset of bash syntax that a code-editing agent realistically emits:

**Commands & Arguments:**
- Simple commands: `grep -rn "TODO" src/`
- Quoting: single quotes `'literal'`, double quotes `"interpolated $VAR"`, backslash escapes `\n`, `\ `

**Pipelines:**
- Pipe operator: `cat file.txt | grep foo | wc -l`
- Each stage's stdout connects to the next stage's stdin via in-memory byte buffers.

**Redirects:**
- Output: `>` (overwrite), `>>` (append)
- Input: `<`
- Stderr: `2>`, `2>>`, `2>&1`
- All redirects target paths in the VFS.

**Operators:**
- Sequential: `;` (run next regardless)
- Conditional: `&&` (run next if previous succeeded), `||` (run next if previous failed)

**Glob Expansion:**
- `*`, `?`, `**`, `[abc]`, `[a-z]`
- Expanded against the VFS, not the host filesystem.

**Environment Variables:**
- Reference: `$VAR`, `${VAR}`, `$?` (last exit code)
- Inline assignment: `FOO=bar command args`
- The shell maintains a virtual environment that persists across commands within a session.

**Builtins** (executed directly, not via WASM):
- `cd <path>` — update VFS working directory
- `pwd` — return VFS working directory
- `export VAR=value` — set environment variable
- `unset VAR` — remove environment variable
- `echo <args>` — can be a builtin for speed (also available as WASM tool)
- `exit [code]` — signal session end

### Explicitly Unsupported

The following bash features are **out of scope**. If the agent emits them, the parser returns a clear error message (not a silent failure):

- Subshells: `$(command)`, `` `command` ``
- Process substitution: `<(command)`, `>(command)`
- Bash functions: `function foo() { ... }`
- Control flow: `if`, `for`, `while`, `case`, `select`
- Here-docs: `<<EOF ... EOF`
- Arrays: `arr=(a b c)`
- Arithmetic: `$(( ... ))`, `let`, `(( ... ))`
- Job control: `&`, `bg`, `fg`, `jobs`

Error format: `devdev: unsupported syntax: <description>. Try: <alternative suggestion>`

For example: `devdev: unsupported syntax: command substitution $(). Try: run the commands separately and pipe the output.`

---

## Pipeline Execution Model

A pipeline like `cat file.txt | grep foo | sort | uniq -c` executes as:

1. **Parse** the full command string into an AST: a sequence of pipeline stages, each with a command, args, and redirects.
2. **Expand** globs and variables against the VFS and virtual environment.
3. **Execute** each pipeline stage:
   a. Create an in-memory byte buffer for inter-stage communication.
   b. Invoke the WASM tool engine for each stage, wiring stdin/stdout to the appropriate buffers.
   c. Stages may execute **concurrently** (producer-consumer on the byte buffer) or **sequentially** (capture all stdout from stage N, then pass to stage N+1). Sequential is simpler and acceptable for v1.
4. **Collect** final stdout/stderr and exit code from the last stage.
5. **Apply** redirect targets: if the final stage has `> output.txt`, write stdout to that VFS path.

### Exit Code Propagation

- A pipeline's exit code is the exit code of its **last** command (matching bash default behavior).
- `&&` chains abort on first non-zero exit.
- `||` chains abort on first zero exit.
- `;` chains always continue.
- The virtual environment stores `$?` as the last command's exit code.

---

## Interface Contract

```
interface ShellSession {
  // Execute a command string. Returns the final result.
  execute(command_string: string) -> {
    stdout: bytes,
    stderr: bytes,
    exit_code: int
  }
  
  // Current working directory (virtual).
  cwd() -> string
  
  // Current environment variables.
  env() -> {string: string}
}
```

A `ShellSession` is stateful: it tracks cwd, env vars, and `$?` across commands within a single agent evaluation.

---

## Design Notes

- The parser should be a proper tokenizer + AST, not regex hacking. Shell quoting rules are subtle (`"$VAR"` vs `'$VAR'` vs `\$VAR`), and getting them wrong will confuse the agent.
- Glob expansion happens **before** the WASM tool sees the args. This matches real bash behavior — the shell expands globs, not the tool.
- Concurrent pipeline execution is a nice-to-have. For v1, sequential (buffer-and-pass) is fine and dramatically simpler to debug.
- The parser must handle multi-line commands if the agent emits them (e.g., a `grep` with a long pattern). Newlines within quotes or after `\` continuation are valid.

---

## Open Questions

1. **Parser implementation strategy:** Write from scratch, or use an existing shell-parsing library/grammar? A bash grammar is well-documented but complex. A subset parser custom-built for our supported syntax may be more reliable.
2. **Concurrent vs sequential pipelines:** Is there a case where streaming matters for correctness (e.g., very large files where buffering all of stdout is impractical given the 2 GB VFS)?
3. **Unsupported syntax recovery:** Should the system attempt to "downgrade" unsupported syntax automatically (e.g., expand `$(git rev-parse HEAD)` by running the inner command first), or always return an error?
