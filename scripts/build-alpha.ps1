<# .SYNOPSIS
  Build the Okkhor portable alpha artifact (repeatable, no network).

.DESCRIPTION
  Builds okkhor-daemon in release, runs `okkhor self-check`, and stages
  dist/Okkhor-<Version>-win64/ plus dist/Okkhor-<Version>-win64-portable.zip
  with SHA256SUMS.txt. Fonts are COPIED from assets/fonts/ (staged earlier
  with scripts/fetch-fonts.ps1); this script never downloads. Missing fonts
  warn but do not fail; a missing exe or README-ALPHA.txt fails closed.

  Run from the repository root:  .\scripts\build-alpha.ps1 -Version 0.5.0-alpha.1
#>
[CmdletBinding()]
param(
  [string]$Version = '0.5.0-alpha.1',
  [switch]$SkipFonts
)

$ErrorActionPreference = 'Stop'

# Allowlist: version is interpolated into directory/zip names, so reject
# anything that could escape the dist folder or inject commands. No
# Invoke-Expression or string-executed commands are used anywhere below.
if ($Version -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,40}$') {
  throw "Invalid -Version '$Version'. Use letters, digits, dot, dash, underscore only (max 41 chars)."
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$DistRoot = Join-Path $RepoRoot 'dist'
$StageDir = Join-Path $DistRoot "Okkhor-$Version-win64"
$ZipPath  = Join-Path $DistRoot "Okkhor-$Version-win64-portable.zip"
$SumsPath = Join-Path $DistRoot 'SHA256SUMS.txt'

$cargo = Join-Path $HOME '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }

Write-Host "==> cargo build --release --locked -p okkhor-daemon" -ForegroundColor Cyan
& $cargo build --release --locked -p okkhor-daemon
if ($LASTEXITCODE -ne 0) { throw "Release build failed (exit $LASTEXITCODE)." }

$Exe = Join-Path $RepoRoot 'target\release\okkhor.exe'
if (-not (Test-Path $Exe)) { throw "Build reported success but $Exe is missing." }

Write-Host '==> okkhor self-check' -ForegroundColor Cyan
& $Exe self-check
if ($LASTEXITCODE -ne 0) { throw "self-check failed (exit $LASTEXITCODE)." }

$Readme = Join-Path $RepoRoot 'README-ALPHA.txt'
if (-not (Test-Path $Readme)) { throw "README-ALPHA.txt is missing at $Readme." }

if (Test-Path $StageDir) { Remove-Item -Recurse -Force $StageDir }
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null
$FontsDir = Join-Path $StageDir 'fonts'
New-Item -ItemType Directory -Force -Path $FontsDir | Out-Null

Copy-Item -Force $Exe (Join-Path $StageDir 'okkhor.exe')

# Fonts: copy what is staged, never download. Absence warns, never fails.
$stagedFonts = @()
if (-not $SkipFonts) {
  $AssetsFonts = Join-Path $RepoRoot 'assets\fonts'
  if (Test-Path $AssetsFonts) {
    $stagedFonts = Get-ChildItem -Path $AssetsFonts -Filter '*.ttf' -File -ErrorAction SilentlyContinue
  }
  foreach ($font in $stagedFonts) {
    Copy-Item -Force $font.FullName (Join-Path $FontsDir $font.Name)
  }
}
if ($SkipFonts) {
  Write-Warning 'Font staging skipped (-SkipFonts); shipping without fonts.'
} elseif ($stagedFonts.Count -eq 0) {
  Write-Warning 'No staged fonts (*.ttf) found; shipping without fonts. Run scripts/fetch-fonts.ps1 first for a full artifact.'
} else {
  Write-Host "Staged $($stagedFonts.Count) font(s)." -ForegroundColor Green
}

# Icon (cosmetic): warn, do not fail.
$Icon = Join-Path $RepoRoot 'assets\branding\okkhor.ico'
if (Test-Path $Icon) {
  Copy-Item -Force $Icon (Join-Path $StageDir 'okkhor.ico')
} else {
  Write-Warning "Icon not found at assets/branding/okkhor.ico; skipping."
}

# Licenses: required for redistribution, but only warn here so a license-text
# gap is visible without masking the exe/README gate below.
$Licenses = Join-Path $RepoRoot 'THIRD_PARTY_LICENSES.md'
if (Test-Path $Licenses) {
  Copy-Item -Force $Licenses (Join-Path $StageDir 'THIRD_PARTY_LICENSES.md')
} else {
  Write-Warning 'THIRD_PARTY_LICENSES.md is missing; shipping without it.'
}
$oflCopied = $false
$AssetsFontsDir = Join-Path $RepoRoot 'assets\fonts'
if (Test-Path $AssetsFontsDir) {
  foreach ($ofl in (Get-ChildItem -Path $AssetsFontsDir -Filter 'OFL-*.txt' -File -ErrorAction SilentlyContinue)) {
    Copy-Item -Force $ofl.FullName (Join-Path $StageDir $ofl.Name)
    $oflCopied = $true
  }
}
if (-not $oflCopied) { Write-Warning 'No OFL-*.txt license texts found under assets/fonts/.' }

Copy-Item -Force $Readme (Join-Path $StageDir 'README-ALPHA.txt')

# Build identity: version, commit, UTC date, toolchain, target.
$commit = 'unknown'
try {
  $gitOut = & git -C $RepoRoot rev-parse HEAD 2>$null
  if ($LASTEXITCODE -eq 0 -and $gitOut) { $commit = "$gitOut".Trim() }
} catch { $commit = 'unknown' }
$rustc = 'unknown'
try {
  $rustcOut = & rustc --version 2>$null
  if ($LASTEXITCODE -eq 0 -and $rustcOut) { $rustc = "$rustcOut".Trim() }
} catch { $rustc = 'unknown' }
$buildInfo = [ordered]@{
  version = $Version
  commit  = $commit
  date_utc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
  rustc   = $rustc
  target  = 'x86_64-pc-windows-msvc'
}
($buildInfo | ConvertTo-Json) | Set-Content -Path (Join-Path $StageDir 'BUILD-INFO.json') -Encoding utf8

if (-not (Test-Path (Join-Path $StageDir 'okkhor.exe'))) { throw 'Staged okkhor.exe is missing; aborting.' }
if (-not (Test-Path (Join-Path $StageDir 'README-ALPHA.txt'))) { throw 'Staged README-ALPHA.txt is missing; aborting.' }

if (Test-Path $ZipPath) { Remove-Item -Force $ZipPath }
Compress-Archive -Path (Join-Path $StageDir '*') -DestinationPath $ZipPath -Force

$hash = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash.ToLower()
$zipName = Split-Path $ZipPath -Leaf
"$hash  $zipName" | Set-Content -Path $SumsPath -Encoding ascii

$zipBytes = (Get-Item $ZipPath).Length
Write-Host "Artifact: $ZipPath ($zipBytes bytes)" -ForegroundColor Green
Write-Host "Staged:   $StageDir" -ForegroundColor Green
Write-Host "SHA256:   $hash" -ForegroundColor Green
