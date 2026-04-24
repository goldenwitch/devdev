# Skeptic-facing validation runner (PowerShell).
#
# Parses claims.toml at repo root, reports which claims have their
# environment prerequisites satisfied, runs the `test` command for
# each runnable claim, and prints PASS/FAIL per claim id.
#
# Exit code: non-zero if any runnable claim failed.
#
# Dependencies: PowerShell 5.1+, cargo. No toml parser dep — the
# manifest format is constrained enough that a tiny regex pass
# handles it.

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$manifest = Join-Path $repoRoot 'claims.toml'

if (-not (Test-Path $manifest)) {
    Write-Error "claims.toml not found at $manifest"
    exit 2
}

Set-Location $repoRoot

# ── WinFSP PATH fix-up (Windows) ───────────────────────────────
#
# The workspace crate links winfsp-x64.dll with /DELAYLOAD, but the
# delay-load stub still needs to find the DLL when a FS op fires.
# WinFSP's installer does not always leave bin\ on the session PATH,
# so any test binary that ever *could* mount will fail to launch
# (STATUS_DLL_NOT_FOUND, 0xc0000135). Mirror build.rs's discovery
# logic: prefer WINFSP_PATH, fall back to the default install root,
# prepend bin\ if present.
if ($IsWindows -or $env:OS -eq 'Windows_NT') {
    $winfspRoot = if ($env:WINFSP_PATH) {
        $env:WINFSP_PATH
    } else {
        'C:\Program Files (x86)\WinFsp'
    }
    $winfspBin = Join-Path $winfspRoot 'bin'
    if (Test-Path $winfspBin) {
        if (-not ($env:PATH -split ';' | Where-Object { $_ -eq $winfspBin })) {
            $env:PATH = "$winfspBin;$env:PATH"
        }
    }
}

# ── Parse claims.toml ──────────────────────────────────────────
#
# Returns an array of PSCustomObject { Id, Test, RequiresEnv }.
# Assumes id/test are simple quoted strings on single lines and
# requires_env is a single-line array of quoted strings. Good
# enough for the current manifest; upgrade to a real parser when
# we actually need triple-quoted arrays.
function Get-Claims {
    param([string]$Path)

    $lines = Get-Content $Path
    $claims = @()
    $current = $null

    foreach ($line in $lines) {
        if ($line -match '^\s*\[\[claim\]\]\s*$') {
            if ($null -ne $current) { $claims += $current }
            $current = [PSCustomObject]@{
                Id = ''
                Test = ''
                RequiresEnv = @()
            }
            continue
        }
        if ($null -eq $current) { continue }
        if ($line -match '^\s*id\s*=\s*"(.*)"\s*$')   { $current.Id = $Matches[1]; continue }
        if ($line -match '^\s*test\s*=\s*"(.*)"\s*$') { $current.Test = $Matches[1]; continue }
        if ($line -match '^\s*requires_env\s*=\s*\[(.*)\]\s*$') {
            $inner = $Matches[1]
            $current.RequiresEnv = [regex]::Matches($inner, '"([^"]*)"') |
                ForEach-Object { $_.Groups[1].Value }
            continue
        }
    }
    if ($null -ne $current) { $claims += $current }
    return $claims
}

# ── Env gate check ─────────────────────────────────────────────
function Test-EnvSatisfied {
    param([string[]]$Requirements)
    foreach ($req in $Requirements) {
        $key, $want = $req -split '=', 2
        $have = [Environment]::GetEnvironmentVariable($key)
        if ($have -ne $want) { return $false }
    }
    return $true
}

# ── Run ────────────────────────────────────────────────────────
$pass = 0
$fail = 0
$skip = 0
$failIds = @()

foreach ($claim in Get-Claims -Path $manifest) {
    if (-not (Test-EnvSatisfied -Requirements $claim.RequiresEnv)) {
        $envStr = $claim.RequiresEnv -join ','
        "SKIP  {0,-24} (env not set: {1})" -f $claim.Id, $envStr | Write-Host
        $skip++
        continue
    }

    "RUN   {0,-24} {1}" -f $claim.Id, $claim.Test | Write-Host

    # cargo exit code is what we care about; let stderr/stdout
    # stream through unmodified so the skeptic sees the real output.
    & cmd.exe /c $claim.Test
    if ($LASTEXITCODE -eq 0) {
        "PASS  {0,-24}" -f $claim.Id | Write-Host
        $pass++
    }
    else {
        "FAIL  {0,-24}" -f $claim.Id | Write-Host
        $fail++
        $failIds += $claim.Id
    }
}

Write-Host ""
Write-Host "-- summary ----------------------------------"
Write-Host "  PASS: $pass"
Write-Host "  FAIL: $fail"
Write-Host "  SKIP: $skip"
if ($fail -gt 0) {
    Write-Host "  failed: $($failIds -join ', ')"
    exit 1
}
