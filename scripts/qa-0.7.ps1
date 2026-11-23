#requires -Version 5.1
<#
.SYNOPSIS
    Automated 0.7 QA gates for Okkhor.

.DESCRIPTION
    Runs formatting, lint, Rust tests, daemon self-check, suggestion-index
    verification, UI build/tests, and the portable artifact build. It does not
    run real-key QA: the keyboard hook intentionally ignores injected keys, so
    the owner checklist in docs/qa/0.7-checklist.md is still required.

.PARAMETER Version
    Portable artifact version. Defaults to 0.7.0.

.PARAMETER SkipUi
    Skip npm ci, UI build, and UI tests.

.PARAMETER SkipBuild
    Skip the portable artifact build and SHA256 print.
#>
[CmdletBinding()]
param(
    [string]$Version = '0.7.0',
    [switch]$SkipUi,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$env:PATH = "$HOME\.cargo\bin;C:\msys64\mingw64\bin;$env:PATH"

function Invoke-Step {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$Action
    )
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

Invoke-Step 'cargo fmt' { cargo fmt --all -- --check }
Invoke-Step 'cargo clippy' { cargo clippy --workspace --all-targets -- -D warnings }
Invoke-Step 'cargo test' { cargo test --workspace --locked }
Invoke-Step 'daemon self-check' { cargo run --package okkhor-daemon -- self-check }
Invoke-Step 'suggestion index checksum' { & (Join-Path $RepoRoot 'scripts\verify-suggestion-index.ps1') }

if (-not $SkipUi) {
    $UiRoot = Join-Path $RepoRoot 'apps\okkhor-ui'
    Push-Location $UiRoot
    try {
        Invoke-Step 'ui npm ci' { npm ci }
        Invoke-Step 'ui build' { npm run build }
        Invoke-Step 'ui tests' { npm test -- --run }
    }
    finally {
        Pop-Location
    }
}

if (-not $SkipBuild) {
    Invoke-Step "portable build $Version" {
        & (Join-Path $RepoRoot 'scripts\build-beta.ps1') -Version $Version
    }
    $sums = Join-Path $RepoRoot 'dist\SHA256SUMS.txt'
    if (-not (Test-Path $sums)) {
        throw "missing artifact checksum file: $sums"
    }
    Write-Host ''
    Write-Host 'Artifact checksums:' -ForegroundColor Green
    Get-Content $sums
}

Write-Host ''
Write-Host 'Automated 0.7 QA gates passed.' -ForegroundColor Green
Write-Host 'Still required: docs/qa/0.7-checklist.md owner real-key pass.'
