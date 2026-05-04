#!/usr/bin/env pwsh
<#
.SYNOPSIS
  Seed the live-tests CI environments with GitHub App credentials.

.DESCRIPTION
  Idempotent. Reads from the local DevDev SecretStore vault and pushes
  to two GitHub Actions Environments on the workflow repo:

    live-tests-admin
      var    DEVDEV_GH_APP_ADMIN_ID
      var    DEVDEV_GH_FIXTURE_OWNER
      var    DEVDEV_GH_FIXTURE_REPO
      var    DEVDEV_LIVE_ADO_ENABLED   = "0"  (until ADO is wired)
      secret DEVDEV_GH_APP_ADMIN_PEM

    live-tests-consumer
      var    DEVDEV_GH_APP_CONSUMER_ID
      var    DEVDEV_GH_FIXTURE_OWNER
      var    DEVDEV_GH_FIXTURE_REPO
      secret DEVDEV_GH_APP_CONSUMER_PEM

  Re-running this script overwrites existing values; safe to run after
  rotating PEMs.

.PARAMETER WorkflowRepo
  The repo where live-tests.yml runs. Default: goldenwitch/devdev.

.PARAMETER FixtureOwner
  GitHub account that owns the fixture repo. Default: goldenwitch.

.PARAMETER FixtureRepo
  Fixture repo name. Default: devdev-test-environment.

.NOTES
  Requires `gh` CLI signed in with at least `repo` scope on
  $WorkflowRepo. Personal-account repos work with the default scopes
  obtained via `gh auth login`.
#>
[CmdletBinding()]
param(
    [string]$WorkflowRepo  = 'goldenwitch/devdev',
    [string]$FixtureOwner  = 'goldenwitch',
    [string]$FixtureRepo   = 'devdev-test-environment'
)

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/devdev-secrets.ps1"

function Ensure-Environment {
    param([string]$Repo, [string]$Env)
    Write-Host "  ensuring environment $Env exists on $Repo..."
    $owner, $name = $Repo -split '/', 2
    gh api --method PUT "repos/$owner/$name/environments/$Env" --silent | Out-Null
}

function Set-EnvVariable {
    param([string]$Repo, [string]$Env, [string]$Name, [string]$Value)
    Write-Host "  var $Env/$Name"
    # `gh variable set` upserts; --env scopes to environment.
    # NOTE: `--body -` is interpreted as the literal value `-`, not
    # as a stdin marker. Pass the value directly via --body.
    & gh variable set $Name --env $Env --repo $Repo --body $Value | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "gh variable set $Name failed (exit $LASTEXITCODE)" }
}

function Set-EnvSecret {
    param([string]$Repo, [string]$Env, [string]$Name, [string]$Value)
    Write-Host "  secret $Env/$Name (length=$($Value.Length))"
    # `gh secret set` upserts; same caveat as variables (`-` is literal).
    # Value is encrypted client-side by gh before transmission.
    & gh secret set $Name --env $Env --repo $Repo --body $Value | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "gh secret set $Name failed (exit $LASTEXITCODE)" }
}

Write-Host "Seeding CI secrets/vars on $WorkflowRepo" -ForegroundColor Cyan
Write-Host "  fixture: $FixtureOwner/$FixtureRepo"

# ---- admin -----------------------------------------------------------
$adminEnv = 'live-tests-admin'
Ensure-Environment -Repo $WorkflowRepo -Env $adminEnv
$adminId  = Get-DevDevSecret -Name 'gh-app-admin-id'
$adminPem = Get-DevDevSecret -Name 'gh-app-admin-pem'
Set-EnvVariable -Repo $WorkflowRepo -Env $adminEnv -Name 'DEVDEV_GH_APP_ADMIN_ID' -Value $adminId
Set-EnvVariable -Repo $WorkflowRepo -Env $adminEnv -Name 'DEVDEV_GH_FIXTURE_OWNER' -Value $FixtureOwner
Set-EnvVariable -Repo $WorkflowRepo -Env $adminEnv -Name 'DEVDEV_GH_FIXTURE_REPO'  -Value $FixtureRepo
Set-EnvVariable -Repo $WorkflowRepo -Env $adminEnv -Name 'DEVDEV_LIVE_ADO_ENABLED' -Value '0'
Set-EnvSecret   -Repo $WorkflowRepo -Env $adminEnv -Name 'DEVDEV_GH_APP_ADMIN_PEM' -Value $adminPem

# ---- consumer --------------------------------------------------------
$consumerEnv = 'live-tests-consumer'
Ensure-Environment -Repo $WorkflowRepo -Env $consumerEnv
$consumerId  = Get-DevDevSecret -Name 'gh-app-consumer-id'
$consumerPem = Get-DevDevSecret -Name 'gh-app-consumer-pem'
Set-EnvVariable -Repo $WorkflowRepo -Env $consumerEnv -Name 'DEVDEV_GH_APP_CONSUMER_ID' -Value $consumerId
Set-EnvVariable -Repo $WorkflowRepo -Env $consumerEnv -Name 'DEVDEV_GH_FIXTURE_OWNER'   -Value $FixtureOwner
Set-EnvVariable -Repo $WorkflowRepo -Env $consumerEnv -Name 'DEVDEV_GH_FIXTURE_REPO'    -Value $FixtureRepo
Set-EnvSecret   -Repo $WorkflowRepo -Env $consumerEnv -Name 'DEVDEV_GH_APP_CONSUMER_PEM' -Value $consumerPem

Write-Host "DONE" -ForegroundColor Green
