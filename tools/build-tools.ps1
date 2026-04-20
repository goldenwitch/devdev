# build-tools.ps1 — Compile Unix coreutils (and extras) to WASM for DevDev.
#
# Prerequisites:
#   rustup target add wasm32-wasip1
#
# Usage:
#   .\tools\build-tools.ps1              # build all P0 + P1 tools
#   .\tools\build-tools.ps1 cat ls       # build only specified tools

param(
    [Parameter(ValueFromRemainingArguments)]
    [string[]]$Tools
)

$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot
$WasmOut = Join-Path $ScriptDir "wasm"
$BuildDir = Join-Path ([System.IO.Path]::GetTempPath()) "devdev-wasm-build"

# ── Pinned versions ──────────────────────────────────────────────
$UutilsVersion = "0.8.0"
$SdVersion = "1.0.0"

# ── Tool → source mapping ───────────────────────────────────────
$UutilsP0 = @("cat", "ls", "head", "tail", "wc", "echo", "mkdir", "rm", "cp", "mv", "touch", "sort", "uniq")
$UutilsP1 = @("tr", "cut", "tee", "basename", "dirname")
$UutilsP2 = @("xargs", "readlink", "realpath", "env", "printf", "true", "false")

$ExtraTools = @{
    "sd" = @{ Package = "sd"; Version = $SdVersion }
}

# Tools known NOT to have trivial WASM builds (tracked for future work):
#   grep  - ripgrep depends on PCRE/system regex; needs investigation
#   find  - fd-find uses OS-specific directory walking
#   diff  - no established pure-Rust WASM-compatible diff binary
#   awk   - no established pure-Rust WASM-compatible awk binary

# ── Functions ────────────────────────────────────────────────────

function Build-UutilsTool {
    param([string]$Tool)
    $pkg = "uu_$Tool"
    Write-Host "  Building $Tool ($pkg v$UutilsVersion)..."
    $output = cargo install $pkg `
        --version $UutilsVersion `
        --target wasm32-wasip1 `
        --root $BuildDir `
        --force `
        --quiet 2>&1

    if ($LASTEXITCODE -ne 0) {
        Write-Host "  ! FAILED: $Tool" -ForegroundColor Red
        Write-Host ($output | Out-String)
        return $false
    }

    # Find the output binary
    $candidates = @(
        (Join-Path $BuildDir "bin" "$Tool.wasm"),
        (Join-Path $BuildDir "bin" "$pkg.wasm")
    )
    $src = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1

    if (-not $src) {
        Write-Host "  ! FAILED: $Tool - binary not found in $BuildDir\bin\" -ForegroundColor Red
        return $false
    }

    Copy-Item $src (Join-Path $WasmOut "$Tool.wasm") -Force
    $size = (Get-Item (Join-Path $WasmOut "$Tool.wasm")).Length
    $sizeKB = [math]::Round($size / 1KB, 0)
    Write-Host "  + $Tool.wasm (${sizeKB} KB)" -ForegroundColor Green
    return $true
}

function Build-ExtraTool {
    param([string]$Tool)
    $spec = $ExtraTools[$Tool]
    $pkg = $spec.Package
    $ver = $spec.Version
    Write-Host "  Building $Tool ($pkg v$ver)..."
    $output = cargo install $pkg `
        --version $ver `
        --target wasm32-wasip1 `
        --root $BuildDir `
        --force `
        --quiet 2>&1

    if ($LASTEXITCODE -ne 0) {
        Write-Host "  ! FAILED: $Tool" -ForegroundColor Red
        Write-Host ($output | Out-String)
        return $false
    }

    $src = Join-Path $BuildDir "bin" "$Tool.wasm"
    if (-not (Test-Path $src)) {
        Write-Host "  ! FAILED: $Tool - binary not found" -ForegroundColor Red
        return $false
    }

    Copy-Item $src (Join-Path $WasmOut "$Tool.wasm") -Force
    $size = (Get-Item (Join-Path $WasmOut "$Tool.wasm")).Length
    $sizeKB = [math]::Round($size / 1KB, 0)
    Write-Host "  + $Tool.wasm (${sizeKB} KB)" -ForegroundColor Green
    return $true
}

# ── Main ─────────────────────────────────────────────────────────

New-Item -ItemType Directory -Path $WasmOut -Force | Out-Null
New-Item -ItemType Directory -Path $BuildDir -Force | Out-Null

if ($Tools.Count -gt 0) {
    $Requested = $Tools
} else {
    $Requested = $UutilsP0 + $UutilsP1 + @($ExtraTools.Keys)
}

Write-Host "Building $($Requested.Count) WASM tools -> $WasmOut"
Write-Host ""

$Succeeded = 0
$Failed = 0
$FailedTools = @()

foreach ($tool in $Requested) {
    if ($ExtraTools.ContainsKey($tool)) {
        if (Build-ExtraTool $tool) { $Succeeded++ } else { $Failed++; $FailedTools += $tool }
    } else {
        if (Build-UutilsTool $tool) { $Succeeded++ } else { $Failed++; $FailedTools += $tool }
    }
}

Write-Host ""
Write-Host "Done: $Succeeded succeeded, $Failed failed"
if ($FailedTools.Count -gt 0) {
    Write-Host "Failed tools: $($FailedTools -join ', ')" -ForegroundColor Yellow
}
Write-Host "Output: $WasmOut"

$wasmFiles = Get-ChildItem -Path $WasmOut -Filter "*.wasm" -ErrorAction SilentlyContinue
if ($wasmFiles) {
    $totalBytes = ($wasmFiles | Measure-Object -Property Length -Sum).Sum
    $totalMB = [math]::Round($totalBytes / 1MB, 1)
    foreach ($f in $wasmFiles | Sort-Object Name) {
        $sizeKB = [math]::Round($f.Length / 1KB, 0)
        Write-Host "  $($f.Name) (${sizeKB} KB)"
    }
    Write-Host "Total bundle size: ${totalMB} MB"
}
