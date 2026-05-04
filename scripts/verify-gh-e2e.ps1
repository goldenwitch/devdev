#!/usr/bin/env pwsh
# End-to-end exercise of GitHub App credentials against the fixture repo.
# Proves: token mint -> read -> branch create -> PR open -> comment -> comment delete.
# Idempotent: tolerates pre-existing branch / PR by reusing them.
$ErrorActionPreference = "Stop"
. "$PSScriptRoot/devdev-secrets.ps1"

$Owner = 'goldenwitch'
$Repo  = 'devdev-test-environment'
$Base  = 'main'
$Branch = 'fixture/canonical'
$PrTitle = 'Canonical fixture PR — DO NOT MERGE'
$TagPrefix = '[devdev-live-test]'

function New-InstallationToken {
    param([string]$App)
    $jwt = Get-GitHubAppJwt -App $App
    $h = @{ Authorization = "Bearer $jwt"; Accept = 'application/vnd.github+json' }
    $installs = Invoke-RestMethod -Uri 'https://api.github.com/app/installations' -Headers $h
    if ($installs.Count -ne 1) { throw "expected exactly one install for $App, got $($installs.Count)" }
    (Invoke-RestMethod -Method Post -Uri "https://api.github.com/app/installations/$($installs[0].id)/access_tokens" -Headers $h).token
}

function GhHeaders([string]$Token) {
    @{ Authorization = "token $Token"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28' }
}

Write-Host "[1/7] Minting installation tokens..." -ForegroundColor Cyan
$adminTok = New-InstallationToken -App admin
$consumerTok = New-InstallationToken -App consumer
$adminH = GhHeaders $adminTok
$consumerH = GhHeaders $consumerTok
Write-Host "  admin:    minted ($($adminTok.Length) chars) [redacted]"
Write-Host "  consumer: minted ($($consumerTok.Length) chars) [redacted]"

Write-Host "[2/7] Consumer reads repo metadata..." -ForegroundColor Cyan
$repoInfo = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo" -Headers $consumerH
Write-Host "  $($repoInfo.full_name)  default_branch=$($repoInfo.default_branch)  visibility=$($repoInfo.visibility)"

Write-Host "[3/7] Admin ensures '$Branch' branch exists..." -ForegroundColor Cyan
try {
    $existing = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/git/refs/heads/$Branch" -Headers $adminH
    Write-Host "  branch already exists at $($existing.object.sha)"
} catch {
    $sc = $_.Exception.Response.StatusCode.value__
    Write-Host "  branch lookup returned $sc, falling back to base lookup"
    if ($sc -ne 404) { throw }
    $baseUrl = "https://api.github.com/repos/$Owner/$Repo/git/refs/heads/$Base"
    Write-Host "  GET $baseUrl"
    $baseRef = Invoke-RestMethod -Uri $baseUrl -Headers $adminH
    Write-Host "  base $Base sha: $($baseRef.object.sha)"
    $body = @{ ref = "refs/heads/$Branch"; sha = $baseRef.object.sha } | ConvertTo-Json
    $created = Invoke-RestMethod -Method Post -Uri "https://api.github.com/repos/$Owner/$Repo/git/refs" -Headers $adminH -Body $body -ContentType 'application/json'
    Write-Host "  created branch at $($created.object.sha)"
}

Write-Host "[4/7] Admin ensures FIXTURE.md exists on '$Branch'..." -ForegroundColor Cyan
try {
    $cur = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/contents/FIXTURE.md?ref=$Branch" -Headers $adminH
    Write-Host "  FIXTURE.md already at sha $($cur.sha)"
} catch {
    if ($_.Exception.Response.StatusCode.value__ -ne 404) { throw }
    $contents = "# DevDev fixture branch`n`nOwned by devdev-test-env. Reset on each apply.`n"
    $b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($contents))
    $body = @{ message = 'seed FIXTURE.md'; content = $b64; branch = $Branch } | ConvertTo-Json
    $put = Invoke-RestMethod -Method Put -Uri "https://api.github.com/repos/$Owner/$Repo/contents/FIXTURE.md" -Headers $adminH -Body $body -ContentType 'application/json'
    Write-Host "  created FIXTURE.md commit $($put.commit.sha)"
}

Write-Host "[5/7] Admin ensures canonical PR exists..." -ForegroundColor Cyan
$prList = Invoke-RestMethod -Uri "https://api.github.com/repos/$Owner/$Repo/pulls?state=open&head=${Owner}:${Branch}&base=$Base" -Headers $adminH
if ($prList.Count -gt 0) {
    $pr = $prList[0]
    Write-Host "  PR #$($pr.number) already open: $($pr.title)"
} else {
    $body = @{ title = $PrTitle; head = $Branch; base = $Base; body = "Provisioned by devdev-test-env. Do not merge." } | ConvertTo-Json
    $pr = Invoke-RestMethod -Method Post -Uri "https://api.github.com/repos/$Owner/$Repo/pulls" -Headers $adminH -Body $body -ContentType 'application/json'
    Write-Host "  opened PR #$($pr.number)"
}

Write-Host "[6/7] Consumer posts a tagged comment on PR #$($pr.number)..." -ForegroundColor Cyan
$nonce = [Guid]::NewGuid().ToString('N').Substring(0,8)
$commentBody = "$TagPrefix`:e2e-verify:$nonce`:hello from installation token"
$commentReq = @{ body = $commentBody } | ConvertTo-Json
$comment = Invoke-RestMethod -Method Post -Uri "https://api.github.com/repos/$Owner/$Repo/issues/$($pr.number)/comments" -Headers $consumerH -Body $commentReq -ContentType 'application/json'
Write-Host "  comment id $($comment.id) by $($comment.user.login)"

Write-Host "[7/7] Consumer sweeps the tagged comment..." -ForegroundColor Cyan
Invoke-RestMethod -Method Delete -Uri "https://api.github.com/repos/$Owner/$Repo/issues/comments/$($comment.id)" -Headers $consumerH | Out-Null
Write-Host "  deleted"

Write-Host "`nALL CHECKS PASSED" -ForegroundColor Green
