# Spec: WASM Tool Execution Engine

**Status:** Draft — Updated with research findings (April 2026)
**Depends on:** Virtual Filesystem (spec-virtual-filesystem.md)

---

## Purpose

Provide a portable execution engine that runs standard Unix command-line tools (grep, find, cat, ls, etc.) as WebAssembly modules against the in-memory virtual filesystem. This gives the agent a familiar bash-like tool surface without depending on the host operating system's installed binaries.

---

## Requirements

### Tool Surface

Tools are compiled individually to WebAssembly (WASI target). Each tool is a standalone `.wasm` binary.

**Priority 0 (must-have at launch):**
`cat`, `ls`, `grep`, `find`, `head`, `tail`, `wc`, `echo`, `mkdir`, `rm`, `cp`, `mv`, `touch`, `sort`, `uniq`

**Priority 1 (needed for real-world agent workflows):**
`sed`, `awk`, `tr`, `cut`, `tee`, `diff`, `basename`, `dirname`

**Priority 2 (useful, can be added incrementally):**
`xargs`, `chmod`, `readlink`, `realpath`, `env`, `printf`, `test`/`[`, `true`, `false`

### Tool Source

The primary source for WASM-compiled tools is **`uutils/coreutils`** — a Rust reimplementation of GNU coreutils with a permissive license.

**This is proven, not speculative.** As of uutils v0.8.0 (April 2025), **70+ utilities** compile cleanly to `wasm32-wasi` using the standard toolchain:
```
cargo build --release --target wasm32-wasi --features feat_wasm
```

Key adaptations already in the uutils codebase:
- `sort` — single-threaded path for WASI (no rayon/threading).
- `tail` — file watching (`notify`) disabled under WASI; static mode works.
- `ls` — graceful fallback when parent directory metadata is inaccessible.
- `cp`/`ln` — returns clear "Unsupported" error for symlinks on WASI.

A live browser demo exists at https://uutils.github.io/playground/ — proof that the WASM compilation is production-quality.

**For `sed` and `awk`** (not part of uutils by design):
- **`sed` → Use `sd`**, a Rust sed alternative. Pure Rust, zero C dependencies, PCRE-compatible regex, trivial WASM compilation. Benchmarked at ~12x faster than GNU sed on large files.
- **`awk` → Use the `awk` crate** (Rust, available on crates.io), a full awk interpreter. Or implement field-splitting with the `regex` crate for a minimal subset.
- Compiling GNU sed/awk to WASI via wasi-sdk is possible but untested in the wild and adds C dependency complexity. Not recommended.

### Execution Model

1. When a tool is invoked (e.g., `grep -r TODO src/`), the engine:
   a. Looks up the corresponding `.wasm` binary by command name.
   b. Instantiates a new WASM module instance.
   c. Configures the WASI layer: mount the VFS as the module's filesystem, set the working directory, inject environment variables, wire up stdin/stdout/stderr.
   d. Passes command-line arguments (e.g., `["-r", "TODO", "src/"]`).
   e. Runs the module to completion.
   f. Captures exit code, stdout bytes, and stderr bytes.
   g. Destroys the module instance.

2. Each tool invocation is **isolated**: no shared state between invocations except through the VFS. This matches how real shell commands work — they communicate through the filesystem and pipes, not shared memory.

3. **No fork/exec.** WASI does not support process spawning. This means:
   - The engine cannot execute `sh -c "..."` or subshells.
   - Tools that internally spawn processes (e.g., `xargs` running a subcommand) need special handling: either a custom implementation that calls back into the engine, or an explicit limitation communicated to the agent.

### WASM Runtime

The recommended WASM runtime is one that provides:
- **Built-in in-memory filesystem** — a `mem_fs` implementation that can back WASI preopens directly, so WASM modules see the VFS as their filesystem root with zero glue code.
- **Ahead-of-time compilation** — `.wasm` binaries pre-compiled to native code for fast startup.
- **Sub-10ms instance creation** — the agent may run dozens of commands per evaluation.
- **Cross-platform support** — Linux, macOS, Windows.

Runtimes with built-in virtual filesystem traits (e.g., `mem_fs`, `overlay_fs`, `mount_fs`) are strongly preferred over those requiring custom WASI filesystem shims.

### Performance

- **Module caching:** The compiled native code for each tool should be loaded once and reused across invocations. Only the instance (memory, state) is created fresh per invocation.
- **Startup target:** Tool invocation overhead should be under 10ms (achievable with AOT compilation and cached modules).

### Bundling

All `.wasm` tool binaries should be embedded directly in the DevDev binary (or distributed alongside it as a single archive). No runtime downloading, no dependency on host-installed tools.

### Known WASI Limitations

The following WASI constraints are known and accounted for:

| Limitation | Impact | Handling |
|-----------|--------|----------|
| No `fork`/`exec` | Tools can't spawn subprocesses | Pipeline orchestration is DevDev's shell parser responsibility |
| No signals (SIGPIPE, etc.) | `tail --follow` doesn't work; some edge cases in piping | Static file processing is unaffected; acceptable |
| No threading | `sort` on very large files is slower | uutils already has single-threaded WASI path |
| No pipes/FIFO (`mkfifo`) | `split --filter` unsupported | Rare in agent usage |
| No device/inode checks | `cp` can't detect self-copy | Acceptable in sandbox |
| No symlinks (creation) | `ln -s` returns error | Existing symlinks in VFS are readable; creation fails gracefully |
| UTF-8 argv required | Filenames must be valid UTF-8 | Non-issue — agent generates UTF-8 commands |

---

## Interface Contract

```
interface ToolEngine {
  // Execute a tool by name with the given arguments.
  // Returns stdout, stderr, and exit code.
  execute(
    command: string,         // e.g., "grep"
    args: [string],          // e.g., ["-r", "TODO", "src/"]
    stdin: byte_stream,      // piped input (may be empty)
    env: {string: string},   // environment variables
    cwd: string,             // working directory in VFS
    fs: VirtualFilesystem    // the VFS to operate on
  ) -> { stdout: bytes, stderr: bytes, exit_code: int }
  
  // List available tools.
  available_tools() -> [string]
  
  // Check if a specific tool is available.
  has_tool(name: string) -> bool
}
```

---

## Design Notes

- The engine is a **pure function** from (command, args, stdin, fs-state) → (stdout, stderr, exit-code, fs-mutations). The only side effect is VFS writes.
- The VFS is passed into each WASM module via the WASI preopens mechanism — the module sees `/` as its filesystem root, backed entirely by the in-memory VFS.
- Environment variables are synthetic. The engine maintains a virtual env that the shell parser can modify (`export FOO=bar`) and passes to each tool invocation.
- If a command is not found in the WASM tool set, the engine returns exit code 127 and stderr `"command not found: <name>"` — matching bash behavior.

---

## Extensibility

The tool set is designed to grow over time. Adding a new tool means:
1. Compile it to a `.wasm` binary targeting WASI.
2. Register it in the tool lookup table.
3. Bundle it with the distribution.

No changes to the shell parser, VFS, or PTY layer are required. This is deliberate — the tool surface should be the easiest thing to extend.

---

## Resolved Questions

1. **`sed` and `awk` source:** ✅ Resolved. Use `sd` (Rust) for sed, `awk` crate for awk. Both compile trivially to wasm32-wasi. No GNU C compilation needed.
2. **uutils WASM feasibility:** ✅ Resolved. 70+ utils compile cleanly. Battle-tested with CI integration tests via wasmtime. Live playground proves it.

## Open Questions

1. **`xargs` and process-spawning tools:** These fundamentally need callback into the engine. Should we implement a custom `xargs` that invokes the engine recursively? Or declare it unsupported?
2. **Error messaging:** When a tool doesn't exist or a WASI limitation is hit, how verbose should the error be? Should the agent get a hint about alternatives?
3. **`sd` CLI compatibility:** `sd` uses a different flag syntax than GNU sed (`sd 'pattern' 'replacement'` vs `sed 's/pattern/replacement/'`). Should we shim `sed` to translate common flags to `sd` invocations, or register `sd` as a separate tool and let the agent learn it?
