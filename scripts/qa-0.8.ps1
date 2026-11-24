#requires -Version 5.1
<#
.SYNOPSIS
    Automated 0.8 QA gates for Okkhor.

.DESCRIPTION
    Runs formatting, lint, Rust tests, daemon self-check, suggestion-index
    verification, UI build/tests, the legacy import report, and the portable
    artifact build. It does not run real-key QA: the keyboard hook intentionally
    ignores injected keys, so the owner rows in docs/qa/0.8-checklist.md are
    still required.

.PARAMETER Version
    Portable artifact version. Defaults to 0.8.0.

.PARAMETER SkipUi
    Skip npm ci, UI build, and UI tests.

.PARAMETER SkipBuild
    Skip the portable artifact build and SHA256 print.

.PARAMETER LegacyHome
    Optional legacy install for the read-only import report. When omitted, the
    import report step is skipped.
#>
[CmdletBinding()]
param(
    [string]$Version = '0.8.0',
    [switch]$SkipUi,
    [switch]$SkipBuild,
    [string]$LegacyHome = ''
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

# 0.8 ships the MSVC build. The MSVC toolchain is installed for this user;
# make the selection explicit so the release and the tests agree.
$env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-msvc'
$env:PATH = "$HOME\.cargo\bin;$env:PATH"

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

if ($LegacyHome) {
    $reportRoot = Join-Path $env:TEMP 'okkhor-0.8-import-report'
    New-Item -ItemType Directory -Force -Path $reportRoot | Out-Null
    Invoke-Step 'legacy import report' {
        & (Join-Path $RepoRoot 'scripts\import-legacy.ps1') `
            -LegacyHome $LegacyHome `
            -DataHome $reportRoot `
            -Apply
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
Write-Host 'Automated 0.8 QA gates passed.' -ForegroundColor Green
Write-Host 'Still required: docs/qa/0.8-checklist.md owner real-key pass.'
