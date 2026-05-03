#!/usr/bin/env pwsh
# Verify GitHub App credentials from the local DevDev SecretStore vault.
# Usage: pwsh scripts/verify-gh-apps.ps1
$ErrorActionPreference = "Stop"
. "$PSScriptRoot/devdev-secrets.ps1"

foreach ($app in @('admin', 'consumer')) {
    Write-Host "=== $app ===" -ForegroundColor Cyan
    try {
        $jwt = Get-GitHubAppJwt -App $app
        $headers = @{
            Authorization          = "Bearer $jwt"
            Accept                 = "application/vnd.github+json"
            "X-GitHub-Api-Version" = "2022-11-28"
        }
        $info = Invoke-RestMethod -Uri "https://api.github.com/app" -Headers $headers
        Write-Host "  slug: $($info.slug)"
        Write-Host "  owner: $($info.owner.login)"
        Write-Host "  permissions: $($info.permissions | ConvertTo-Json -Compress)"
        $installs = Invoke-RestMethod -Uri "https://api.github.com/app/installations" -Headers $headers
        if (-not $installs -or $installs.Count -eq 0) {
            Write-Host "  installations: NONE" -ForegroundColor Yellow
        } else {
            foreach ($inst in $installs) {
                Write-Host "  install id=$($inst.id) account=$($inst.account.login) repo_selection=$($inst.repository_selection)"
            }
        }
    }
    catch {
        Write-Host "  ERROR: $_" -ForegroundColor Red
    }
}
