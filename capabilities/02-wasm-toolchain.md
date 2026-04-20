---
id: wasm-toolchain
title: "WASM Coreutils Build Pipeline"
status: done
type: build
phase: 2
priority: P0
depends-on: []
effort: M
---

# 02 — WASM Coreutils Build Pipeline

Compile standard Unix tools to WebAssembly. This is a build/infra task — no Rust application code. The output is a set of `.wasm` binaries under [tools/wasm/](../tools/wasm/) that get embedded into the DevDev binary at compile time (by `04-tool-registry`).

## Scope

**In:**
- Compile `uutils/coreutils` P0/P1 tools to `wasm32-wasip1`
- Compile `sd` (sed replacement) to `wasm32-wasip1`
- Cross-platform build scripts (bash + PowerShell) that reproducibly regenerate every `.wasm` binary from pinned upstream versions
- Track which standard tools do **not** have a trivial pure-Rust WASM source — these are handled natively in Rust by `04-tool-registry`, not here

**Out:**
- Runtime loading / embedding of WASM modules (that's `03-wasm-engine` + `04-tool-registry`)
- The sed → `sd` flag shim (that's `04-tool-registry`)
- Native-Rust fallback implementations of the missing tools (that's `04-tool-registry`)

## Target & Versions

| Knob | Value |
|------|-------|
| Rust target | `wasm32-wasip1` (WASI preview 1) |
| `uutils/coreutils` | `0.8.0` (pinned) |
| `sd` | `1.0.0` (pinned) |

Prerequisite: `rustup target add wasm32-wasip1`.

## Build Scripts

- [tools/build-tools.sh](../tools/build-tools.sh) — bash (Linux/macOS)
- [tools/build-tools.ps1](../tools/build-tools.ps1) — PowerShell (Windows)

Both scripts:
- Pull each tool via `cargo install <pkg> --target wasm32-wasip1 --root <tempdir>` (uutils ships each utility as an individual `uu_<name>` crate on crates.io, so no upstream checkout is needed).
- Copy the produced `.wasm` into [tools/wasm/](../tools/wasm/) with a normalized name (`cat.wasm`, not `uu_cat.wasm`).
- Accept a tool-list argument to rebuild a subset (`./build-tools.sh cat ls`).
- Print per-tool size and total bundle size when done.

## Tool Manifest

### Built to WASM (committed in [tools/wasm/](../tools/wasm/))

| Priority | Source | Tools |
|----------|--------|-------|
| P0 (uutils) | `uu_<name>` v0.8.0 | `cat`, `ls`, `head`, `tail`, `wc`, `echo`, `mkdir`, `rm`, `cp`, `mv`, `touch`, `sort`, `uniq` |
| P0 extras | `sd` v1.0.0 | `sd` (routed as `sed` via shim in 04) |

There is also a grandfathered `diff.wasm` artifact in [tools/wasm/](../tools/wasm/) that the current build scripts do not know how to rebuild — the authoritative plan is for `diff` to be a **native** tool (see below). If a future rebuild drops `diff.wasm`, that's expected and not a regression.

### P1/P2 still to add (uutils, nontrivial but not blocking P0)

`tr`, `cut`, `tee`, `basename`, `dirname` (P1) and `xargs`, `readlink`, `realpath`, `env`, `printf`, `true`, `false` (P2). The scripts know how to build them; they are simply not in the committed bundle yet. Regenerate via `./tools/build-tools.sh tr cut tee basename dirname` when needed.

### Deliberately **not** WASM-built (handled natively in `04-tool-registry`)

These are served by pure-Rust implementations running inside the `devdev-wasm` crate — no `.wasm` artifact, no WASI detour. The agent still invokes them as `grep`, `find`, `diff`, `awk`; the registry dispatches them to a Rust impl instead of the WASM engine.

| Tool | Why not WASM | Native fallback |
|------|--------------|-----------------|
| `grep` | `ripgrep` depends on PCRE / `regex` features that don't cleanly compile to `wasm32-wasip1` at the pinned version | `regex` crate + VFS tree walk (P0 — implement in 04) |
| `find` | `fd-find` uses OS-specific directory walking APIs | `globset` + `VirtualFilesystem::walk` (P0 — implement in 04) |
| `diff` | No established pure-Rust WASM-compatible binary; the current `diff.wasm` artifact is unmaintained | `similar` crate, unified-diff formatter (P0 — implement in 04) |
| `awk` | No pure-Rust WASM-compatible awk binary | Punted to P2; not blocking launch |

This split is deliberate: the WASM path stays pure (no feature-gated forks, no PCRE glue), and the native fallbacks are small, well-scoped Rust that also benefits from direct VFS access (no WASI preopen overhead on large tree walks).

## Output Layout

```
tools/wasm/
  cat.wasm    cp.wasm     echo.wasm   head.wasm   ls.wasm
  mkdir.wasm  mv.wasm     rm.wasm     sort.wasm   tail.wasm
  touch.wasm  uniq.wasm   wc.wasm     sd.wasm
  (diff.wasm — grandfathered; will be replaced by a native fallback)
  (grep.wasm — grandfathered; will be replaced by a native fallback)
```

Current bundle: ~15 `.wasm` files. Expected size once the P1 additions are built: ~15–25 MB total.

## Known Build Considerations

- `uu_sort`: single-threaded path for WASI (no rayon). Works as-is.
- `uu_tail`: file watching disabled under WASI. Static mode works.
- `uu_ls`: graceful fallback when parent dir metadata is inaccessible.
- `uu_cp` / `uu_ln`: return "Unsupported" for symlink creation on WASI (we don't virtualize symlinks).
- `sd`: pure Rust, zero C deps — trivial WASM compilation.
- The upstream uutils crates occasionally tighten their MSRV; if a rebuild fails, check `rust-toolchain` before chasing feature flags.

## Acceptance Criteria

- [x] `build-tools.sh` / `build-tools.ps1` run to completion and produce all P0 `.wasm` binaries in [tools/wasm/](../tools/wasm/)
- [x] Each bundled `.wasm` is a valid WASI module (`wasmtime compile <file>.wasm` succeeds — verified by `devdev-wasm` loading them in `03-wasm-engine`'s test suite)
- [x] `sd.wasm` is present and usable as the `sed` backend
- [x] Build is reproducible: same pinned versions → same bytes (modulo cargo's build hash)
- [x] Script runs on Linux/macOS (bash) and Windows (PowerShell)
- [x] Missing-tool rationale for `grep`/`find`/`diff`/`awk` is documented here **and** in both build scripts
- [ ] *(deferred to 04-tool-registry)* Native fallbacks for `grep`/`find`/`diff` cover the surface the agent expects

## Related

- [capabilities/03-wasm-engine.md](03-wasm-engine.md) — loads these binaries at runtime
- [capabilities/04-tool-registry.md](04-tool-registry.md) — embeds them, dispatches commands, and provides the native fallbacks listed above
---
id: wasm-toolchain
title: "WASM Coreutils Build Pipeline"
status: not-started
type: build
phase: 2
priority: P0
depends-on: []
effort: M
---

# 02 — WASM Coreutils Build Pipeline

Compile standard Unix tools to WebAssembly. This is a build/infra task — no Rust application code. The output is a set of `.wasm` binaries that get bundled into the DevDev binary.

## Scope

**In:**
- Compile uutils/coreutils to `wasm32-wasi` (P0 and P1 tools)
- Compile `sd` (sed replacement) to `wasm32-wasi`
- Compile `awk` crate to `wasm32-wasi`
- Build script that reproducibly generates all `.wasm` binaries
- CI integration to rebuild when upstream versions change

**Out:**
- Runtime loading of WASM modules (that's `03-wasm-engine`)
- The sed flag shim (that's `04-tool-registry`)

## Tool Manifest

### P0 — Must-have at launch (from uutils)

`cat`, `ls`, `grep`, `find`, `head`, `tail`, `wc`, `echo`, `mkdir`, `rm`, `cp`, `mv`, `touch`, `sort`, `uniq`

### P1 — Needed for real workflows

`sed` → built from `sd` (Rust, PCRE regex, zero C deps)
`awk` → built from `awk` crate
`tr`, `cut`, `tee`, `diff`, `basename`, `dirname` → from uutils

### P2 — Add incrementally

`xargs`, `chmod`, `readlink`, `realpath`, `env`, `printf`, `test`/`[`, `true`, `false` → from uutils

## Build Process

```bash
#!/usr/bin/env bash
# tools/build-tools.sh

set -euo pipefail

WASM_OUT="tools/wasm"
UUTILS_VERSION="0.8.0"  # pin version
SD_VERSION="1.0.0"
mkdir -p "$WASM_OUT"

# 1. uutils/coreutils
git clone --depth 1 --branch "$UUTILS_VERSION" \
    https://github.com/uutils/coreutils.git /tmp/uutils

# Build each P0 tool individually
TOOLS=(cat ls grep find head tail wc echo mkdir rm cp mv touch sort uniq
       tr cut tee diff basename dirname)

for tool in "${TOOLS[@]}"; do
    cargo build --manifest-path /tmp/uutils/Cargo.toml \
        --release --target wasm32-wasi \
        --features "feat_wasm" \
        -p "uu_${tool}"
    cp "/tmp/uutils/target/wasm32-wasi/release/uu_${tool}.wasm" \
       "${WASM_OUT}/${tool}.wasm"
done

# 2. sd (sed replacement)
cargo install sd --version "$SD_VERSION" --target wasm32-wasi \
    --root /tmp/sd-build
cp /tmp/sd-build/bin/sd.wasm "${WASM_OUT}/sd.wasm"

# 3. awk
# (specific build steps depend on chosen awk crate)

echo "Built $(ls -1 $WASM_OUT/*.wasm | wc -l) WASM tools"
```

**Note:** The exact cargo invocation for uutils may need adjustment — individual util builds use `uu_<name>` package names. The `feat_wasm` feature gates threading and OS-specific code.

## Known Build Considerations

- **uutils `sort`:** Has a single-threaded path for WASI (no rayon). Works as-is.
- **uutils `tail`:** File watching disabled under WASI. Static mode works.
- **uutils `ls`:** Graceful fallback when parent dir metadata is inaccessible.
- **uutils `cp`/`ln`:** Returns "Unsupported" for symlink creation on WASI.
- **`sd`:** Pure Rust, zero C dependencies — trivial WASM compilation.
- **`wasm32-wasi` target:** Must be installed via `rustup target add wasm32-wasi`.

## Output

```
tools/wasm/
  cat.wasm
  ls.wasm
  grep.wasm
  find.wasm
  head.wasm
  tail.wasm
  wc.wasm
  echo.wasm
  mkdir.wasm
  rm.wasm
  cp.wasm
  mv.wasm
  touch.wasm
  sort.wasm
  uniq.wasm
  tr.wasm
  cut.wasm
  tee.wasm
  diff.wasm
  basename.wasm
  dirname.wasm
  sd.wasm
  awk.wasm
```

## Acceptance Criteria

- [ ] `build-tools.sh` runs to completion, producing all P0 `.wasm` binaries
- [ ] Each `.wasm` binary is a valid WASI module (verify with `wasmtime compile <file>.wasm`)
- [ ] P1 binaries (sd, awk, tr, cut, etc.) also compile
- [ ] Total `.wasm` bundle size is documented (ballpark: ~10-20 MB for all tools)
- [ ] Build is reproducible — same inputs produce same outputs (pin uutils version)
- [ ] Build script runs on Linux, macOS, and Windows (or documents platform requirements)
