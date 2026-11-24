<#
.SYNOPSIS
  Build the Okkhor 0.7.x portable beta (daemon + worker + Tauri UI + fonts).

.DESCRIPTION
  Builds okkhor-daemon, okkhor-worker, and okkhor-ui in release, verifies the
  daemon version identity, and stages dist/Okkhor-<Version>-win64/ plus a
  portable zip and SHA256SUMS.txt. Fonts are copied from assets/fonts/ (stage
  them first with scripts/fetch-fonts.ps1); a missing font warns, a missing
  binary fails.

  Run from anywhere:  .\scripts\build-beta.ps1 -Version 0.7.0
#>
[CmdletBinding()]
param(
  [string]$Version = '0.7.0',
  [switch]$SkipFonts,
  [switch]$SkipUi
)

$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,40}$') {
  throw "Invalid -Version '$Version'. Use letters, digits, dot, dash, underscore only."
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$DistRoot = Join-Path $RepoRoot 'dist'
$StageDir = Join-Path $DistRoot "Okkhor-$Version-win64"
$ZipPath  = Join-Path $DistRoot "Okkhor-$Version-win64-portable.zip"
$SumsPath = Join-Path $DistRoot 'SHA256SUMS.txt'

$cargo = Join-Path $HOME '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }

Write-Host '==> daemon: cargo build --release --locked -p okkhor-daemon' -ForegroundColor Cyan
& $cargo build --manifest-path (Join-Path $RepoRoot 'Cargo.toml') --release --locked -p okkhor-daemon
if ($LASTEXITCODE -ne 0) { throw "Daemon release build failed (exit $LASTEXITCODE)." }
$DaemonExe = Join-Path $RepoRoot 'target\release\okkhor.exe'
if (-not (Test-Path $DaemonExe)) { throw "Build reported success but $DaemonExe is missing." }

Write-Host '==> daemon: version identity gate' -ForegroundColor Cyan
$actual = (& $DaemonExe --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { throw "okkhor --version failed (exit $LASTEXITCODE)." }
if ($actual -ne "okkhor $Version") {
  throw "Version mismatch: daemon reports '$actual', expected 'okkhor $Version'. refusing to stage."
}
Write-Host "Identity: $actual" -ForegroundColor Green

Write-Host '==> daemon: self-check' -ForegroundColor Cyan
& $DaemonExe self-check
if ($LASTEXITCODE -ne 0) { throw "self-check failed (exit $LASTEXITCODE)." }

Write-Host '==> worker: cargo build --release --locked -p okkhor-worker' -ForegroundColor Cyan
& $cargo build --manifest-path (Join-Path $RepoRoot 'Cargo.toml') --release --locked -p okkhor-worker
if ($LASTEXITCODE -ne 0) { throw "Worker release build failed (exit $LASTEXITCODE)." }
$WorkerExe = Join-Path $RepoRoot 'target\release\okkhor-worker.exe'
if (-not (Test-Path $WorkerExe)) { throw "Build reported success but $WorkerExe is missing." }

$UiExe = $null
if (-not $SkipUi) {
  $UiRoot = Join-Path $RepoRoot 'apps\okkhor-ui'
  Write-Host '==> ui: npm ci && npm run build' -ForegroundColor Cyan
  Push-Location $UiRoot
  try {
    & npm ci
    if ($LASTEXITCODE -ne 0) { throw "npm ci failed (exit $LASTEXITCODE)." }
    & npm run build
    if ($LASTEXITCODE -ne 0) { throw "npm run build failed (exit $LASTEXITCODE)." }
  } finally { Pop-Location }

  Write-Host '==> ui: cargo build --release --locked' -ForegroundColor Cyan
  & $cargo build --manifest-path (Join-Path $UiRoot 'src-tauri\Cargo.toml') --release --locked --features custom-protocol
  if ($LASTEXITCODE -ne 0) { throw "UI release build failed (exit $LASTEXITCODE)." }
  $UiExe = Join-Path $UiRoot 'src-tauri\target\release\okkhor-ui.exe'
  if (-not (Test-Path $UiExe)) { throw "UI build reported success but $UiExe is missing." }

  $conf = Get-Content (Join-Path $UiRoot 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
  if ($conf.version -ne $Version) {
    throw "UI version mismatch: tauri.conf.json reports '$($conf.version)', expected '$Version'."
  }
}

if (Test-Path $StageDir) { Remove-Item $StageDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

# Portable beta layout: one user-facing executable at the folder root
# (`Okkhor.exe`, the Tauri UI) and the resident engine hidden in `bin\`.
# The UI starts the engine itself (single entry point); the engine still finds
# its neighbors and fonts relative to the artifact root.
$BinDir = Join-Path $StageDir 'bin'
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
Copy-Item $DaemonExe (Join-Path $BinDir 'okkhor-engine.exe') -Force
Copy-Item $WorkerExe (Join-Path $BinDir 'okkhor-worker.exe') -Force
if ($UiExe) { Copy-Item $UiExe (Join-Path $StageDir 'Okkhor.exe') -Force }

$Icon = Join-Path $RepoRoot 'assets\branding\okkhor.ico'
if (Test-Path $Icon) { Copy-Item $Icon (Join-Path $StageDir 'okkhor.ico') -Force }

$FontDir = Join-Path $RepoRoot 'assets\fonts'
$FontsStage = Join-Path $StageDir 'fonts'
New-Item -ItemType Directory -Force -Path $FontsStage | Out-Null
if (-not $SkipFonts -and (Test-Path $FontDir)) {
  $fonts = Get-ChildItem $FontDir -Filter *.ttf -File
  foreach ($f in $fonts) { Copy-Item $f.FullName (Join-Path $FontsStage $f.Name) -Force }
  if ($fonts.Count -eq 0) { Write-Warning 'No .ttf files found in assets/fonts; artifact will have an empty fonts folder.' }
  else { Write-Host "Staged $($fonts.Count) font(s)." -ForegroundColor Green }
} else {
  Write-Warning 'Fonts skipped; the artifact will have an empty fonts folder.'
}

$Licenses = Join-Path $StageDir 'LICENSES'
New-Item -ItemType Directory -Force -Path $Licenses | Out-Null
$thirdParty = Join-Path $RepoRoot 'THIRD_PARTY_LICENSES.md'
if (Test-Path $thirdParty) { Copy-Item $thirdParty (Join-Path $Licenses 'THIRD_PARTY_LICENSES.md') -Force }
Get-ChildItem $FontDir -Filter 'OFL-*.txt' -File -ErrorAction SilentlyContinue |
  ForEach-Object { Copy-Item $_.FullName (Join-Path $Licenses $_.Name) -Force }

$Readme = Join-Path $RepoRoot 'README-BETA.txt'
if (-not (Test-Path $Readme)) { throw "README-BETA.txt is missing at $Readme." }
Copy-Item $Readme (Join-Path $StageDir 'README.txt') -Force

$hostTriple = (& rustc -vV 2>$null | Select-String '^host:\s*(.+)$').Matches.Groups[1].Value
if (-not $hostTriple) { $hostTriple = 'x86_64-pc-windows-msvc' }
$commit = (& git -C $RepoRoot rev-parse HEAD).Trim()
$uiSha = if ($UiExe) { (Get-FileHash $UiExe -Algorithm SHA256).Hash } else { $null }

$buildInfo = [ordered]@{
  product      = 'Okkhor'
  version      = $Version
  commit       = $commit
  date_utc     = (Get-Date).ToUniversalTime().ToString('o')
  rustc        = (& rustc --version)
  target       = $hostTriple
  daemon_sha256 = (Get-FileHash $DaemonExe -Algorithm SHA256).Hash
  worker_sha256 = (Get-FileHash $WorkerExe -Algorithm SHA256).Hash
  ui_sha256    = $uiSha
}
$buildInfo | ConvertTo-Json -Depth 3 | Set-Content (Join-Path $StageDir 'BUILD-INFO.json') -Encoding utf8

if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
Compress-Archive -Path (Join-Path $StageDir '*') -DestinationPath $ZipPath -CompressionLevel Optimal
$zipHash = (Get-FileHash $ZipPath -Algorithm SHA256).Hash
"$zipHash  $(Split-Path $ZipPath -Leaf)" | Set-Content $SumsPath -Encoding ascii

Write-Host "Artifact: $ZipPath" -ForegroundColor Green
Write-Host "Staged:   $StageDir" -ForegroundColor Green
Write-Host "SHA256:   $zipHash" -ForegroundColor Green
