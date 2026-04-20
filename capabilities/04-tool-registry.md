---
id: tool-registry
title: "Tool Registry & Dispatch"
status: done
type: composition
phase: 2
crate: devdev-wasm
priority: P0
depends-on: [wasm-engine, wasm-toolchain]
effort: M
---

# 04 — Tool Registry & Dispatch

Map command names to backends (WASM module or native Rust) and handle tool-specific quirks. This is the public face of the tool engine — the shell executor calls `ToolEngine::execute("grep", args, ...)` and this layer figures out which backend to invoke and how.

## Scope

**In:**
- `ToolEngine` trait consumed by `09-shell-executor`
- Command → backend dispatch table with two kinds of backends:
  1. **WASM** — the `.wasm` binaries built by `02-wasm-toolchain` and embedded via `include_bytes!`
  2. **Native** — pure-Rust implementations for tools without a viable WASM source (`grep`, `find`, `diff`; `awk` is P2)
- GNU `sed` → `sd` flag shim
- "command not found" handling (exit 127) for truly unknown commands
- A single public `WasmToolRegistry::new()` constructor that wires everything up

**Out:**
- WASM execution mechanics (that's `03-wasm-engine`)
- WASM binary production (that's `02-wasm-toolchain`)
- Shell parsing or pipeline orchestration (that's `09-shell-executor`)

## Interface

```rust
/// The interface the shell executor calls.
pub trait ToolEngine: Send + Sync {
    fn execute(
        &self,
        command: &str,
        args: &[String],
        stdin: &[u8],
        env: &HashMap<String, String>,
        cwd: &str,
        fs: &dyn VirtualFilesystem,
    ) -> ToolResult;

    fn available_tools(&self) -> Vec<&str>;
    fn has_tool(&self, name: &str) -> bool;
}

pub struct ToolResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}
```

The trait is intentionally backend-agnostic — callers can't tell WASM from native from the result type.

## Dispatch Model

Every incoming `execute(name, …)` call walks a fixed priority chain:

```
execute(name, args, …)
    │
    ├─► 1. Shim? (e.g. name == "sed") → translate args, recurse as the target tool
    │
    ├─► 2. Native backend registered for `name`? → call the Rust impl
    │
    ├─► 3. WASM backend registered for `name`? → dispatch through WasmEngine
    │
    └─► 4. Otherwise → ToolResult { stderr: "command not found: <name>\n", exit_code: 127 }
```

Rationale for putting native **before** WASM: it lets us override a WASM tool with a native fallback if a `.wasm` artifact is ever dropped without a rebuild, and it's the only sensible order for `grep`/`find`/`diff` since they don't have WASM bodies today.

## Registry Construction

```rust
pub struct WasmToolRegistry {
    wasm: WasmEngine,                                    // from 03-wasm-engine
    wasm_modules: HashSet<&'static str>,                 // names of loaded WASM tools
    native: HashMap<&'static str, Arc<dyn NativeTool>>,  // native fallbacks
    shims: HashMap<&'static str, &'static str>,          // e.g. "sed" → "sd"
}

impl WasmToolRegistry {
    pub fn new() -> Result<Self, RegistryError> {
        let mut wasm = WasmEngine::new()?;

        // Embed every WASM tool at compile time. The build script guarantees
        // these paths exist; a missing file is a compile error, not a runtime one.
        wasm.load_module("cat",   include_bytes!("../../../tools/wasm/cat.wasm"))?;
        wasm.load_module("ls",    include_bytes!("../../../tools/wasm/ls.wasm"))?;
        wasm.load_module("head",  include_bytes!("../../../tools/wasm/head.wasm"))?;
        wasm.load_module("tail",  include_bytes!("../../../tools/wasm/tail.wasm"))?;
        wasm.load_module("wc",    include_bytes!("../../../tools/wasm/wc.wasm"))?;
        wasm.load_module("echo",  include_bytes!("../../../tools/wasm/echo.wasm"))?;
        wasm.load_module("mkdir", include_bytes!("../../../tools/wasm/mkdir.wasm"))?;
        wasm.load_module("rm",    include_bytes!("../../../tools/wasm/rm.wasm"))?;
        wasm.load_module("cp",    include_bytes!("../../../tools/wasm/cp.wasm"))?;
        wasm.load_module("mv",    include_bytes!("../../../tools/wasm/mv.wasm"))?;
        wasm.load_module("touch", include_bytes!("../../../tools/wasm/touch.wasm"))?;
        wasm.load_module("sort",  include_bytes!("../../../tools/wasm/sort.wasm"))?;
        wasm.load_module("uniq",  include_bytes!("../../../tools/wasm/uniq.wasm"))?;
        wasm.load_module("sd",    include_bytes!("../../../tools/wasm/sd.wasm"))?;

        let wasm_modules = /* collect above names */;

        // Native fallbacks — tools that can't reasonably be WASM-built today.
        let mut native: HashMap<_, Arc<dyn NativeTool>> = HashMap::new();
        native.insert("grep", Arc::new(native::Grep));
        native.insert("find", Arc::new(native::Find));
        native.insert("diff", Arc::new(native::Diff));

        let mut shims = HashMap::new();
        shims.insert("sed", "sd");

        Ok(Self { wasm, wasm_modules, native, shims })
    }
}
```

### `NativeTool` trait

Internal, not exposed to shell callers. Mirrors the `ToolEngine::execute` signature.

```rust
trait NativeTool: Send + Sync {
    fn execute(
        &self,
        args: &[String],
        stdin: &[u8],
        env: &HashMap<String, String>,
        cwd: &str,
        fs: &dyn VirtualFilesystem,
    ) -> ToolResult;
}
```

## Native Fallback Implementations

Each native tool implements just the flag surface the agent actually uses. If the agent passes an unsupported flag, return exit code 2 with a clear stderr — do not silently ignore.

### `grep` (P0)

Backend: the `regex` crate + `VirtualFilesystem::walk` for recursive mode.

| Flag | Behavior |
|------|----------|
| *(positional)* pattern, then paths | Match pattern against each line of each file |
| `-r` / `-R` | Recurse into directories |
| `-n` | Prefix each match with `line:` |
| `-i` | Case-insensitive |
| `-l` | Print only file names with matches |
| `-v` | Invert match |
| `-F` | Fixed string, not regex |
| `-w` | Whole-word match |
| `-c` | Count matches per file |

Out of scope for P0: `-A/-B/-C` context, `-P` PCRE, `--include/--exclude`, binary detection, colorized output. Exit codes: `0` = matches found, `1` = no matches, `2` = error (bad regex, missing file).

### `find` (P0)

Backend: `globset` + `VirtualFilesystem::walk`.

| Flag | Behavior |
|------|----------|
| *(positional path)* | Starting directory |
| `-name <glob>` | Match basename against glob |
| `-iname <glob>` | Case-insensitive `-name` |
| `-type f` / `-type d` | Filter by file / directory |
| `-maxdepth <N>` / `-mindepth <N>` | Depth limits |
| `-path <glob>` | Match full relative path |

Out of scope for P0: `-exec`, `-print0`, `-newer`, `-size`, `-perm`, `-prune`. Output: one path per line. Exit `0` on success, `1` on error.

### `diff` (P0)

Backend: the `similar` crate.

| Flag | Behavior |
|------|----------|
| *(positional)* two paths | Compare file contents, emit unified diff |
| `-u` / `--unified[=N]` | Unified format (default; N context lines, default 3) |
| `-r` / `--recursive` | Recurse; diff file-by-file |
| `-N` / `--new-file` | Treat missing files as empty |
| `-q` / `--brief` | Only report whether files differ |
| `--no-color` | (Accepted and ignored — we never color) |

Exit `0` = identical, `1` = differ, `2` = error. Header format matches GNU `diff` (`--- a/path\tTIMESTAMP` / `+++ b/path\tTIMESTAMP`) — the agent parses this.

### `awk` (P2, deferred)

Punted. Not in the P0 dispatch table. If the agent invokes `awk`, it falls through to the 127 path, which is correct behavior for the P0 release.

## `sed` → `sd` Shim

The agent emits GNU sed syntax (`sed 's/old/new/g' file`). The `sd` tool uses a different flag syntax (`sd 'old' 'new' file`). The shim translates before dispatch:

| GNU sed invocation | `sd` equivalent |
|-------------------|---------------|
| `sed 's/old/new/' file` | `sd 'old' 'new' file` |
| `sed 's/old/new/g' file` | `sd 'old' 'new' file` (`sd` is global by default) |
| `sed -i 's/old/new/g' file` | `sd -i 'old' 'new' file` |
| `sed -n '/pattern/p' file` | Not shimmed — return error with suggestion |
| `sed -e 's/a/b/' -e 's/c/d/'` | Chain: `sd 'a' 'b' file` then `sd 'c' 'd' file` |

For sed expressions that can't be translated, return:
`devdev: sed expression not supported. Try: sd 'pattern' 'replacement' file`
with exit code 2. This is clearer than silently producing the wrong output.

## Files

```
crates/devdev-wasm/src/registry.rs           — WasmToolRegistry, ToolEngine impl, dispatch chain, shim table + translate_shim()
crates/devdev-wasm/src/native/mod.rs         — NativeTool trait + module exports
crates/devdev-wasm/src/native/grep.rs        — regex + tree walk
crates/devdev-wasm/src/native/find.rs        — globset + tree walk
crates/devdev-wasm/src/native/diff.rs        — similar-based unified diff
```

Shim logic is inlined in `registry.rs` rather than split into a `shims/` directory. The shim table is currently empty (no `sd.wasm` available); registering `("sed", "sd")` in `WasmToolRegistry::new()` will light up the GNU-sed shim transparently once the binary lands.

## Acceptance Criteria

- [ ] `execute("cat", &["file.txt"], …)` reads from VFS and writes contents to stdout (WASM path)
- [ ] `execute("grep", &["-rn", "TODO", "src/"], …)` recursively searches the VFS tree (**native** path)
- [ ] `execute("find", &[".", "-name", "*.rs"], …)` lists matching files (**native** path)
- [ ] `execute("diff", &["a.txt", "b.txt"], …)` produces a unified diff (**native** path)
- [ ] `execute("sed", &["s/old/new/g", "file.txt"], …)` → shim rewrites args → dispatches to `sd` WASM → correct result
- [ ] `execute("awk", …)` → exit 127, stderr `command not found: awk\n` (deferred to P2)
- [ ] `execute("nonexistent", …)` → exit 127, stderr `command not found: nonexistent\n`
- [ ] `available_tools()` returns the union of WASM module names, native tool names, and shim source names (so `"sed"` is listed even though the binary is `sd`)
- [ ] `has_tool("grep")` → true, `has_tool("vim")` → false
- [ ] Native `grep -n foo file.txt` output format (`path:line:content`) matches GNU grep on fixture data
- [ ] Native `find . -name '*.rs' -type f` output matches GNU find (lexicographic order, one per line) on fixture data
- [ ] Native `diff a b` unified output parses as a valid patch (use `patch --dry-run` in the test, or parse with the `similar` round-trip)
- [ ] All P0 tools exercised against VFS fixtures in `crates/devdev-wasm/tests/`

## Related

- [capabilities/02-wasm-toolchain.md](02-wasm-toolchain.md) — builds the `.wasm` binaries this crate embeds
- [capabilities/03-wasm-engine.md](03-wasm-engine.md) — the WASI runtime the WASM branch dispatches through
- [capabilities/09-shell-executor.md](09-shell-executor.md) — the primary consumer of `ToolEngine`
