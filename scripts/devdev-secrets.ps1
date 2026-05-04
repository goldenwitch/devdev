# Helper functions for reading DevDev secrets from the local SecretStore vault.
# Dot-source this in other scripts:  . "$PSScriptRoot/devdev-secrets.ps1"
#
# Local vault layout (Microsoft.PowerShell.SecretStore, password-less DPAPI):
#   gh-app-admin-pem            RSA private key (PEM, RSA PKCS#1)
#   gh-app-admin-id             GitHub App numeric ID
#   gh-app-admin-client-id      GitHub App client ID (Iv23...)
#   gh-app-consumer-pem
#   gh-app-consumer-id
#   gh-app-consumer-client-id
#   ado-sp-admin-client-id      (added later)
#   ado-sp-admin-tenant-id
#   ado-sp-consumer-client-id
#   ado-sp-consumer-tenant-id

Set-StrictMode -Version Latest

function Get-DevDevSecret {
    param([Parameter(Mandatory)][string]$Name)
    Import-Module Microsoft.PowerShell.SecretManagement -ErrorAction Stop
    Get-Secret -Name $Name -Vault DevDev -AsPlainText -ErrorAction Stop
}

function New-GitHubAppJwt {
    param(
        [Parameter(Mandatory)][string]$AppId,
        [Parameter(Mandatory)][string]$Pem
    )
    $rsa = [System.Security.Cryptography.RSA]::Create()
    $rsa.ImportFromPem($Pem)
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    function ToB64Url([byte[]]$b) {
        [Convert]::ToBase64String($b).TrimEnd('=').Replace('+','-').Replace('/','_')
    }
    $h = ToB64Url ([Text.Encoding]::UTF8.GetBytes('{"alg":"RS256","typ":"JWT"}'))
    $p = ToB64Url ([Text.Encoding]::UTF8.GetBytes("{`"iat`":$($now-30),`"exp`":$($now+540),`"iss`":`"$AppId`"}"))
    $signingInput = "$h.$p"
    $sig = $rsa.SignData(
        [Text.Encoding]::UTF8.GetBytes($signingInput),
        [Security.Cryptography.HashAlgorithmName]::SHA256,
        [Security.Cryptography.RSASignaturePadding]::Pkcs1)
    return "$signingInput." + (ToB64Url $sig)
}

function Get-GitHubAppJwt {
    # Convenience: name = 'admin' or 'consumer'
    param([Parameter(Mandatory)][ValidateSet('admin','consumer')][string]$App)
    $pem = Get-DevDevSecret -Name "gh-app-$App-pem"
    $id  = Get-DevDevSecret -Name "gh-app-$App-id"
    New-GitHubAppJwt -AppId $id -Pem $pem
}
