#!/usr/bin/env bash
# build-tools.sh — Compile Unix coreutils (and extras) to WASM for DevDev.
#
# Prerequisites:
#   rustup target add wasm32-wasip1
#
# Usage:
#   ./tools/build-tools.sh          # build all P0 + P1 tools
#   ./tools/build-tools.sh cat ls   # build only specified tools

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WASM_OUT="${SCRIPT_DIR}/wasm"
BUILD_DIR="${TMPDIR:-/tmp}/devdev-wasm-build"

# ── Pinned versions ──────────────────────────────────────────────
UUTILS_VERSION="0.8.0"
SD_VERSION="1.0.0"

# ── Tool → source mapping ───────────────────────────────────────
# uutils tools: installed as uu_<name> from crates.io
UUTILS_P0=(cat ls head tail wc echo mkdir rm cp mv touch sort uniq)
UUTILS_P1=(tr cut tee basename dirname)
UUTILS_P2=(xargs readlink realpath env printf true false)

# Non-uutils tools with separate crate names
declare -A EXTRA_TOOLS=(
    [sd]="sd:${SD_VERSION}"
)

# Tools known NOT to have trivial WASM builds (tracked for future work):
#   grep  → ripgrep depends on PCRE/system regex; needs investigation
#   find  → fd-find uses OS-specific directory walking
#   diff  → no established pure-Rust WASM-compatible diff binary
#   awk   → no established pure-Rust WASM-compatible awk binary

# ── Functions ────────────────────────────────────────────────────

build_uutils_tool() {
    local tool="$1"
    local pkg="uu_${tool}"
    echo "  Building ${tool} (${pkg} v${UUTILS_VERSION})..."
    cargo install "${pkg}" \
        --version "${UUTILS_VERSION}" \
        --target wasm32-wasip1 \
        --root "${BUILD_DIR}" \
        --force \
        --quiet 2>&1 || {
        echo "  ⚠ FAILED: ${tool}" >&2
        return 1
    }
    # uu_ binaries are named uu_<tool> or <tool>; find whichever exists
    local src
    for candidate in "${BUILD_DIR}/bin/${tool}.wasm" "${BUILD_DIR}/bin/${pkg}.wasm"; do
        if [ -f "$candidate" ]; then
            src="$candidate"
            break
        fi
    done
    if [ -z "${src:-}" ]; then
        echo "  ⚠ FAILED: ${tool} — binary not found in ${BUILD_DIR}/bin/" >&2
        return 1
    fi
    cp "$src" "${WASM_OUT}/${tool}.wasm"
    echo "  ✓ ${tool}.wasm ($(du -h "${WASM_OUT}/${tool}.wasm" | cut -f1))"
}

build_extra_tool() {
    local tool="$1"
    local spec="${EXTRA_TOOLS[$tool]}"
    local pkg="${spec%%:*}"
    local ver="${spec##*:}"
    echo "  Building ${tool} (${pkg} v${ver})..."
    cargo install "${pkg}" \
        --version "${ver}" \
        --target wasm32-wasip1 \
        --root "${BUILD_DIR}" \
        --force \
        --quiet 2>&1 || {
        echo "  ⚠ FAILED: ${tool}" >&2
        return 1
    }
    local src="${BUILD_DIR}/bin/${tool}.wasm"
    if [ ! -f "$src" ]; then
        echo "  ⚠ FAILED: ${tool} — binary not found" >&2
        return 1
    fi
    cp "$src" "${WASM_OUT}/${tool}.wasm"
    echo "  ✓ ${tool}.wasm ($(du -h "${WASM_OUT}/${tool}.wasm" | cut -f1))"
}

# ── Main ─────────────────────────────────────────────────────────

mkdir -p "$WASM_OUT" "$BUILD_DIR"

# If specific tools requested, build only those
if [ $# -gt 0 ]; then
    REQUESTED=("$@")
else
    REQUESTED=("${UUTILS_P0[@]}" "${UUTILS_P1[@]}" "${!EXTRA_TOOLS[@]}")
fi

echo "Building ${#REQUESTED[@]} WASM tools → ${WASM_OUT}"
echo ""

SUCCEEDED=0
FAILED=0

for tool in "${REQUESTED[@]}"; do
    if [[ -v "EXTRA_TOOLS[$tool]" ]]; then
        build_extra_tool "$tool" && ((SUCCEEDED++)) || ((FAILED++))
    else
        build_uutils_tool "$tool" && ((SUCCEEDED++)) || ((FAILED++))
    fi
done

echo ""
echo "Done: ${SUCCEEDED} succeeded, ${FAILED} failed"
echo "Output: ${WASM_OUT}/"
ls -lh "${WASM_OUT}"/*.wasm 2>/dev/null | awk '{print $5, $9}'
TOTAL=$(du -sh "${WASM_OUT}" | cut -f1)
echo "Total bundle size: ${TOTAL}"
