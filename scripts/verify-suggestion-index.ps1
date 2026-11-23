# Verifies that the committed suggestion index matches its manifest.
# Usage: .\scripts\verify-suggestion-index.ps1
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$dir = Join-Path $repo 'crates\okkhor-engine\assets\suggest'
$manifestPath = Join-Path $dir 'manifest.json'
$indexPath = Join-Path $dir 'suggest-v1.bin'

if (-not (Test-Path $manifestPath)) { throw "Missing manifest: $manifestPath" }
if (-not (Test-Path $indexPath)) { throw "Missing index: $indexPath" }

$manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
$actual = (Get-FileHash $indexPath -Algorithm SHA256).Hash.ToLowerInvariant()
$expected = $manifest.sha256.ToLowerInvariant()
$size = (Get-Item $indexPath).Length

Write-Host "index:    $indexPath"
Write-Host "bytes:    $size (manifest $($manifest.bytes))"
Write-Host "sha256:   $actual"
Write-Host "expected: $expected"

if ($size -ne [int64]$manifest.bytes) { throw 'Index size does not match the manifest' }
if ($actual -ne $expected) { throw 'Index SHA256 does not match the manifest' }
Write-Host 'suggestion index OK'
