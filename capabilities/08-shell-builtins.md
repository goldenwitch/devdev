---
id: shell-builtins
title: "Shell Builtins"
status: done
type: leaf
phase: 2
crate: devdev-shell
priority: P0
depends-on: [vfs-core]
effort: S
---

# 08 — Shell Builtins

Implement shell builtins that operate directly on session state — no WASM module, no subprocess. These modify the shell environment (cwd, env vars) or produce simple output.

## Scope

**In:**
- `cd <path>` — change working directory (validated against VFS)
- `pwd` — print working directory
- `export VAR=value` — set environment variable
- `unset VAR` — remove environment variable
- `echo <args>` — print arguments to stdout (fast path, also available as WASM tool)
- `exit [code]` — signal session end with optional exit code

**Out:**
- Parsing (that's `07-shell-parser`)
- Pipeline execution (that's `09-shell-executor`)
- Anything that runs as a WASM module

## Interface

```rust
pub struct ShellState {
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub last_exit_code: i32,
}

pub enum BuiltinResult {
    /// Command produced output and/or modified state.
    Ok {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        exit_code: i32,
    },
    /// Session should end.
    Exit(i32),
    /// Not a builtin — fall through to tool dispatch.
    NotBuiltin,
}

pub fn try_builtin(
    name: &str,
    args: &[String],
    state: &mut ShellState,
    vfs: &dyn VirtualFilesystem,
) -> BuiltinResult;
```

## Command Details

### `cd`

```
cd <path>     → resolve path relative to cwd, verify it's a directory in VFS, update cwd
cd            → cd to "/" (no $HOME in sandbox)
cd -          → cd to previous directory (track $OLDPWD)
cd ..         → parent directory
```

Errors: `cd nonexistent` → `stderr: "cd: no such file or directory: nonexistent"`, exit 1.

### `pwd`

Print `state.cwd` to stdout, followed by newline. Always exit 0.

### `export`

```
export FOO=bar     → set env["FOO"] = "bar"
export FOO         → mark FOO as exported (no-op in our model, all vars are visible)
export             → list all env vars (format: "declare -x FOO=bar")
```

### `unset`

```
unset FOO          → remove env["FOO"]
unset FOO BAR      → remove multiple
```

Always exit 0 (even if variable didn't exist — matches bash).

### `echo`

```
echo hello world   → "hello world\n" to stdout
echo -n hello      → "hello" (no trailing newline)
echo -e "a\tb"     → "a\tb" (interpret escapes)
echo               → "\n" (empty line)
```

The echo builtin is a fast path. If the agent uses `echo` in a pipeline, the executor may still use it as a builtin.

### `exit`

```
exit         → exit with code 0
exit 1       → exit with code 1
```

Returns `BuiltinResult::Exit(code)` — the shell executor interprets this to end the session.

## Files

```
crates/devdev-shell/src/builtins.rs    — try_builtin() and all builtin implementations
crates/devdev-shell/src/state.rs       — ShellState struct
```

## Acceptance Criteria

- [ ] `cd /some/path` then `pwd` → prints `/some/path`
- [ ] `cd` (no args) → cwd is `/`
- [ ] `cd nonexistent` → error message, exit code 1, cwd unchanged
- [ ] `export FOO=bar` then env contains `FOO=bar`
- [ ] `unset FOO` then env no longer contains `FOO`
- [ ] `echo hello world` → stdout is `"hello world\n"`
- [ ] `echo -n hello` → stdout is `"hello"` (no newline)
- [ ] `exit 42` returns `BuiltinResult::Exit(42)`
- [ ] `try_builtin("grep", ...)` returns `NotBuiltin`
