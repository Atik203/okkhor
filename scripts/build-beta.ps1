<#
.SYNOPSIS
  Build the Okkhor 0.8.x portable beta (daemon + worker + Tauri UI + fonts).

.DESCRIPTION
  Builds okkhor-daemon, okkhor-worker, and (unless -SkipUi) okkhor-ui in
  release for an explicit Rust target. MSVC is the default production
  toolchain; the script forces RUSTUP_TOOLCHAIN for MSVC targets because
  rust-toolchain.toml selects the GNU dev fallback in an interactive shell.

  MSVC builds link the C runtime statically (`-C target-feature=+crt-static`)
  so the portable zip does not need a separate VC++ Redistributable. The staged
  executables are checked against their PE import table; a VCRUNTIME140 or
  MSVCP140 import fails the build. `-UiDynamicCrt` is the fallback when a Tauri
  UI dependency cannot link the static CRT; it is documented and recorded in
  BUILD-INFO.json.

  It then verifies the daemon version identity and self-check, stages
  dist/Okkhor-<Version>-win64/ plus a portable zip and SHA256SUMS.txt, and
  optionally signs the staged executables with Authenticode.

  Signing is opt-in: pass -PfxPath (with -PfxPassword or
  OKKHOR_SIGN_PFX_PASSWORD) or -CertificateThumbprint. Without a certificate
  the artifact is unsigned and Windows SmartScreen will warn.

  Fonts are copied from assets/fonts/ (stage them first with
  scripts/fetch-fonts.ps1); a missing font warns, a missing binary fails.

.EXAMPLE
  .\scripts\build-beta.ps1 -Version 0.8.0

.EXAMPLE
  $env:OKKHOR_SIGN_PFX_PASSWORD = '<from your secret store>'
  .\scripts\build-beta.ps1 -Version 0.8.0 -PfxPath C:\keys\okkhor.pfx

.EXAMPLE
  .\scripts\build-beta.ps1 -Version 0.8.0 -CertificateThumbprint 0123456789ABCDEF0123456789ABCDEF01234567
#>
[CmdletBinding()]
param(
  [string]$Version = '0.8.0',
  [string]$Target = 'x86_64-pc-windows-msvc',
  [string]$OutputDir,
  [switch]$SkipFonts,
  [switch]$SkipUi,
  [switch]$UiDynamicCrt,
  [switch]$SkipSign,
  [string]$PfxPath,
  [System.Security.SecureString]$PfxPassword,
  [string]$CertificateThumbprint,
  [string]$TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,40}$') {
  throw "Invalid -Version '$Version'. Use letters, digits, dot, dash, underscore only."
}
if ($Target -notmatch '^[A-Za-z0-9_.-]+$') {
  throw "Invalid -Target '$Target'."
}
if ($PfxPath -and $CertificateThumbprint) {
  throw 'Pass either -PfxPath or -CertificateThumbprint, not both.'
}
if ($PfxPassword -and -not $PfxPath) {
  throw '-PfxPassword requires -PfxPath.'
}
if ($PfxPath -and -not (Test-Path -LiteralPath $PfxPath -PathType Leaf)) {
  throw "PFX file not found: $PfxPath"
}

# --- PE import inspection (no external tool required) -----------------------
# Reads the import directory so the build can prove the artifact does not need
# the Microsoft Visual C++ Redistributable. UCRT api-ms-win-crt-* imports come
# from Windows 10/11 itself and are not a redistributable dependency.

function Convert-PeRvaToOffset {
  param([object[]]$Sections, [uint32]$Rva)

  foreach ($section in $Sections) {
    $size = if ($section.VirtualSize -gt 0) { $section.VirtualSize } else { $section.RawSize }
    if ($Rva -ge $section.VirtualAddress -and $Rva -lt ($section.VirtualAddress + $size)) {
      return [int]($section.RawPointer + ($Rva - $section.VirtualAddress))
    }
  }
  return -1
}

function Get-PeImportNames {
  [CmdletBinding()]
  param([Parameter(Mandatory = $true)][string]$Path)

  $bytes = [System.IO.File]::ReadAllBytes((Get-Item -LiteralPath $Path).FullName)
  if ($bytes.Length -lt 0x40) { throw "Not a PE file: $Path" }

  $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
  if ($peOffset -le 0 -or $peOffset + 0x18 -gt $bytes.Length) { throw "Invalid PE header: $Path" }
  if ([BitConverter]::ToUInt32($bytes, $peOffset) -ne 0x00004550) { throw "Missing PE signature: $Path" }

  $coffOffset = $peOffset + 4
  $sectionCount = [BitConverter]::ToUInt16($bytes, $coffOffset + 2)
  $optionalSize = [BitConverter]::ToUInt16($bytes, $coffOffset + 16)
  $optionalOffset = $coffOffset + 20
  $magic = [BitConverter]::ToUInt16($bytes, $optionalOffset)
  $dataDirectoryOffset = switch ($magic) {
    0x20B { $optionalOffset + 0x70 }  # PE32+
    0x10B { $optionalOffset + 0x60 }  # PE32
    default { throw "Unsupported PE optional header in $Path" }
  }

  $importRva = [BitConverter]::ToUInt32($bytes, $dataDirectoryOffset + 8)
  if ($importRva -eq 0) { return @() }

  $sections = @()
  $sectionTable = $optionalOffset + $optionalSize
  for ($i = 0; $i -lt $sectionCount; $i++) {
    $entry = $sectionTable + (40 * $i)
    $sections += [pscustomobject]@{
      VirtualSize    = [BitConverter]::ToUInt32($bytes, $entry + 8)
      VirtualAddress = [BitConverter]::ToUInt32($bytes, $entry + 12)
      RawSize        = [BitConverter]::ToUInt32($bytes, $entry + 16)
      RawPointer     = [BitConverter]::ToUInt32($bytes, $entry + 20)
    }
  }

  $names = [System.Collections.Generic.List[string]]::new()
  $cursor = Convert-PeRvaToOffset -Sections $sections -Rva $importRva
  for ($guard = 0; $guard -lt 4096 -and $cursor -ge 0 -and ($cursor + 20) -le $bytes.Length; $guard++) {
    $originalFirstThunk = [BitConverter]::ToUInt32($bytes, $cursor)
    $nameRva = [BitConverter]::ToUInt32($bytes, $cursor + 12)
    $firstThunk = [BitConverter]::ToUInt32($bytes, $cursor + 16)
    if ($nameRva -eq 0 -and $firstThunk -eq 0 -and $originalFirstThunk -eq 0) { break }

    $nameOffset = Convert-PeRvaToOffset -Sections $sections -Rva $nameRva
    if ($nameOffset -ge 0 -and $nameOffset -lt $bytes.Length) {
      $end = $nameOffset
      while ($end -lt $bytes.Length -and $bytes[$end] -ne 0) { $end++ }
      $names.Add([System.Text.Encoding]::ASCII.GetString($bytes, $nameOffset, $end - $nameOffset))
    }
    $cursor += 20
  }

  return $names
}

function Assert-NoVcRedistributableImport {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $imports = Get-PeImportNames -Path $Path
  $redist = @($imports | Where-Object { $_ -match '^(vcruntime140|msvcp140|concrt140|vcomp140)' })
  if ($redist.Count -gt 0) {
    throw ("$Label imports $($redist -join ', '), so the portable zip would require the " +
      'Microsoft Visual C++ Redistributable. Rebuild with +crt-static (drop -UiDynamicCrt), ' +
      'or document the redistributable dependency before publishing.')
  }
  Write-Host "CRT: $Label has no VC++ Redistributable imports." -ForegroundColor Green
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$DistRoot = if ($OutputDir) {
  $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDir)
} else {
  Join-Path $RepoRoot 'dist'
}
$StageDir = Join-Path $DistRoot "Okkhor-$Version-win64"
$ZipPath  = Join-Path $DistRoot "Okkhor-$Version-win64-portable.zip"
$SumsPath = Join-Path $DistRoot 'SHA256SUMS.txt'

$cargo = Join-Path $HOME '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }

# MSVC is the production toolchain. rust-toolchain.toml pins the GNU dev
# fallback, so the release build must override it explicitly.
$Toolchain = $null
if ($Target -match '-msvc$') {
  $Toolchain = 'stable-x86_64-pc-windows-msvc'
  if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    throw 'rustup is required to select the MSVC toolchain but was not found on PATH.'
  }
  $installedTargets = & rustup target list --installed --toolchain $Toolchain 2>$null
  if ($LASTEXITCODE -ne 0) {
    throw "Rust toolchain '$Toolchain' is not installed. Run: rustup toolchain install $Toolchain"
  }
  if (@($installedTargets) -notcontains $Target) {
    throw "Target '$Target' is not installed for '$Toolchain'. Run: rustup target add $Target --toolchain $Toolchain"
  }
}

# Resolve the signer before the build so a missing signtool or password fails
# in seconds instead of after a full release compile.
$certConfigured = [bool]($PfxPath -or $CertificateThumbprint)
$signRequested = (-not $SkipSign) -and $certConfigured
$SignTool = $null
if ($signRequested) {
  . (Join-Path $PSScriptRoot 'sign-artifact.ps1')
  $SignTool = Resolve-SignToolPath
  if ($PfxPath -and -not $PfxPassword -and $env:OKKHOR_SIGN_PFX_PASSWORD) {
    $PfxPassword = ConvertTo-SecureString -String $env:OKKHOR_SIGN_PFX_PASSWORD -AsPlainText -Force
  }
}

$previousToolchain = $env:RUSTUP_TOOLCHAIN
$previousRustFlags = $env:RUSTFLAGS
$useStaticCrt = $Target -match '-msvc$'
$StaticCrtFlag = '-C target-feature=+crt-static'
if ($Toolchain) { $env:RUSTUP_TOOLCHAIN = $Toolchain }

try {
  $toolchainLabel = if ($Toolchain) { $Toolchain } else { 'ambient toolchain' }
  Write-Host "==> build: $toolchainLabel, target $Target" -ForegroundColor Cyan

  if ($useStaticCrt) {
    if ($previousRustFlags -notmatch 'crt-static') {
      $env:RUSTFLAGS = if ($previousRustFlags) { "$previousRustFlags $StaticCrtFlag" } else { $StaticCrtFlag }
    }
    Write-Host "==> crt: static ($StaticCrtFlag); the zip must not need the VC++ Redistributable" -ForegroundColor Cyan
  }

  Write-Host "==> daemon: cargo build --release --locked --target $Target -p okkhor-daemon" -ForegroundColor Cyan
  & $cargo build --manifest-path (Join-Path $RepoRoot 'Cargo.toml') --release --locked --target $Target -p okkhor-daemon
  if ($LASTEXITCODE -ne 0) { throw "Daemon release build failed (exit $LASTEXITCODE)." }
  $DaemonExe = Join-Path $RepoRoot "target\$Target\release\okkhor.exe"
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

  Write-Host "==> worker: cargo build --release --locked --target $Target -p okkhor-worker" -ForegroundColor Cyan
  & $cargo build --manifest-path (Join-Path $RepoRoot 'Cargo.toml') --release --locked --target $Target -p okkhor-worker
  if ($LASTEXITCODE -ne 0) { throw "Worker release build failed (exit $LASTEXITCODE)." }
  $WorkerExe = Join-Path $RepoRoot "target\$Target\release\okkhor-worker.exe"
  if (-not (Test-Path $WorkerExe)) { throw "Build reported success but $WorkerExe is missing." }

  $UiExe = $null
  if (-not $SkipUi) {
    if ($UiDynamicCrt -and $useStaticCrt) {
      Write-Warning 'UI built with the dynamic CRT (-UiDynamicCrt); Okkhor.exe will require the Microsoft Visual C++ Redistributable. The engine and worker stay self-contained.'
      $env:RUSTFLAGS = $previousRustFlags
    }

    $UiRoot = Join-Path $RepoRoot 'apps\okkhor-ui'
    Write-Host '==> ui: npm ci && npm run build' -ForegroundColor Cyan
    Push-Location $UiRoot
    try {
      & npm ci
      if ($LASTEXITCODE -ne 0) { throw "npm ci failed (exit $LASTEXITCODE)." }
      & npm run build
      if ($LASTEXITCODE -ne 0) { throw "npm run build failed (exit $LASTEXITCODE)." }
    } finally { Pop-Location }

    Write-Host "==> ui: cargo build --release --locked --target $Target" -ForegroundColor Cyan
    & $cargo build --manifest-path (Join-Path $UiRoot 'src-tauri\Cargo.toml') --release --locked --target $Target --features custom-protocol
    if ($LASTEXITCODE -ne 0) { throw "UI release build failed (exit $LASTEXITCODE)." }
    $UiExe = Join-Path $UiRoot "src-tauri\target\$Target\release\okkhor-ui.exe"
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
  $EngineStaged = Join-Path $BinDir 'okkhor-engine.exe'
  $WorkerStaged = Join-Path $BinDir 'okkhor-worker.exe'
  $UiStaged = Join-Path $StageDir 'Okkhor.exe'
  Copy-Item $DaemonExe $EngineStaged -Force
  Copy-Item $WorkerExe $WorkerStaged -Force
  if ($UiExe) { Copy-Item $UiExe $UiStaged -Force }

  # Portability gate: the portable zip must run without installing the VC++
  # Redistributable. This is checked on the staged (soon to be signed) bytes.
  $engineCrt = 'system'
  $workerCrt = 'system'
  $uiCrt = 'system'
  if ($useStaticCrt) {
    Assert-NoVcRedistributableImport -Path $EngineStaged -Label 'okkhor-engine.exe'
    Assert-NoVcRedistributableImport -Path $WorkerStaged -Label 'okkhor-worker.exe'
    $engineCrt = 'static'
    $workerCrt = 'static'

    if (Test-Path $UiStaged) {
      if ($UiDynamicCrt) {
        Write-Warning 'Okkhor.exe is built with the dynamic CRT and requires the Microsoft Visual C++ Redistributable (x64). This is recorded in BUILD-INFO.json.'
        $uiCrt = 'dynamic'
      } else {
        Assert-NoVcRedistributableImport -Path $UiStaged -Label 'Okkhor.exe'
        $uiCrt = 'static'
      }
    }
  }

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

  # Sign the staged copies, not the build outputs: the zip must contain the
  # signed bytes, and the hashes recorded below must match what users download.
  $signed = $false
  $signMethod = 'none'
  $signatureVerified = $false
  if ($signRequested) {
    $signTargets = @()
    if (Test-Path $UiStaged) {
      $signTargets += $UiStaged
    } else {
      Write-Warning 'Okkhor.exe was not staged (-SkipUi); only the engine and worker will be signed.'
    }
    $signTargets += $EngineStaged
    $signTargets += $WorkerStaged

    Write-Host "==> sign: $($signTargets.Count) executable(s)" -ForegroundColor Cyan
    Invoke-AuthenticodeSign `
      -Path $signTargets `
      -PfxPath $PfxPath `
      -PfxPassword $PfxPassword `
      -CertificateThumbprint $CertificateThumbprint `
      -TimestampUrl $TimestampUrl `
      -SignToolPath $SignTool
    $signed = $true
    $signMethod = if ($PfxPath) { 'pfx' } else { 'thumbprint' }
    $signatureVerified = $true
    Write-Host "Signature: $signMethod, timestamped via $TimestampUrl, verified." -ForegroundColor Green
  } else {
    if ($certConfigured -and $SkipSign) {
      Write-Warning 'Signature skipped by -SkipSign. The artifact is UNSIGNED even though a certificate was configured.'
    }
    Write-Warning 'Unsigned build: SmartScreen will warn; provide a trusted Authenticode certificate to remove it.'
  }

  $hostTriple = (& rustc -vV 2>$null | Select-String '^host:\s*(.+)$').Matches.Groups[1].Value
  if (-not $hostTriple) { $hostTriple = $Target }
  $toolchainId = if ($Toolchain) { $Toolchain } else { ((& rustup show active-toolchain 2>$null) -split '\s+')[0] }
  if (-not $toolchainId) { $toolchainId = "stable-$hostTriple" }
  $rustcVersion = (& rustc --version)
  $commit = (& git -C $RepoRoot rev-parse HEAD 2>$null | Out-String).Trim()
  if (-not $commit) { $commit = 'unknown' }

  $uiSha = if (Test-Path $UiStaged) { (Get-FileHash $UiStaged -Algorithm SHA256).Hash } else { $null }

  $buildInfo = [ordered]@{
    product       = 'Okkhor'
    version       = $Version
    commit        = $commit
    date_utc      = (Get-Date).ToUniversalTime().ToString('o')
    rustc         = $rustcVersion
    toolchain     = $toolchainId
    target        = $Target
    profile       = 'release'
    runtime       = [ordered]@{
      engine_crt                  = $engineCrt
      worker_crt                  = $workerCrt
      ui_crt                      = $uiCrt
      vc_redistributable_required = [bool]($uiCrt -eq 'dynamic')
    }
    signing       = [ordered]@{
      signed         = $signed
      method         = $signMethod
      verified       = $signatureVerified
      timestamp_url  = if ($signed) { $TimestampUrl } else { $null }
    }
    daemon_sha256 = (Get-FileHash $EngineStaged -Algorithm SHA256).Hash
    worker_sha256 = (Get-FileHash $WorkerStaged -Algorithm SHA256).Hash
    ui_sha256     = $uiSha
  }
  $buildInfo | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $StageDir 'BUILD-INFO.json') -Encoding utf8

  if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
  Compress-Archive -Path (Join-Path $StageDir '*') -DestinationPath $ZipPath -CompressionLevel Optimal
  $zipHash = (Get-FileHash $ZipPath -Algorithm SHA256).Hash
  "$zipHash  $(Split-Path $ZipPath -Leaf)" | Set-Content $SumsPath -Encoding ascii

  Write-Host "Artifact: $ZipPath" -ForegroundColor Green
  Write-Host "Staged:   $StageDir" -ForegroundColor Green
  Write-Host "SHA256:   $zipHash" -ForegroundColor Green
} finally {
  # Do not leak the overrides into the caller's shell.
  if ($null -ne $previousRustFlags) {
    $env:RUSTFLAGS = $previousRustFlags
  } else {
    Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
  }
  if ($null -ne $previousToolchain) {
    $env:RUSTUP_TOOLCHAIN = $previousToolchain
  } else {
    Remove-Item Env:RUSTUP_TOOLCHAIN -ErrorAction SilentlyContinue
  }
}
