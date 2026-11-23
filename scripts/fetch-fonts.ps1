<# .SYNOPSIS
  Download Okkhor's bundled fonts into assets/fonts/ and verify them.

.DESCRIPTION
  Fonts are vendored at build time, never committed. Each entry below is
  pinned by URL, expected byte length, and SHA256. Any mismatch (or any
  download failure) aborts with a non-zero exit code and deletes the
  partial file, so a release build can never silently ship a wrong font.

  Run from the repository root:  .\scripts\fetch-fonts.ps1
#>
[CmdletBinding()]
param(
  [string]$OutDir = (Join-Path $PSScriptRoot '..\assets\fonts')
)

$ErrorActionPreference = 'Stop'

$Fonts = @(
  @{
    Name   = 'kalpurush.ttf'
    Url    = 'https://www.omicronlab.com/download/fonts/kalpurush.ttf'
    Length = 314592
    Sha256 = '83613A03E53AAD8337F2BE9FB974409A23C1D99B3B4B494567BA9D156C492B01'
  },
  @{
    Name   = 'kalpurush ANSI.ttf'
    Url    = 'https://www.omicronlab.com/download/fonts/kalpurush%20ANSI.ttf'
    Length = 113300
    Sha256 = 'BFB06B2360A558424C61405A29C24B09C70487757204814C6DD04621A2A9EEBC'
  },
  @{
    Name   = 'NotoSansBengali.ttf'
    Url    = 'https://raw.githubusercontent.com/google/fonts/main/ofl/notosansbengali/NotoSansBengali%5Bwdth%2Cwght%5D.ttf'
    Length = 463668
    Sha256 = 'dcd42978094e584a849c84a51450eeac40c8826057d566ea6d4b9627a403a05a'
  }
)

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$resolved = (Resolve-Path $OutDir).Path

foreach ($font in $Fonts) {
  $dest = Join-Path $resolved $font.Name
  Write-Host "Fetching $($font.Name) ..."
  if (Test-Path $dest) { Remove-Item -Force $dest }
  try {
    Invoke-WebRequest -Uri $font.Url -OutFile $dest -UseBasicParsing
  } catch {
    if (Test-Path $dest) { Remove-Item -Force $dest }
    throw "Download failed for $($font.Name): $($_.Exception.Message)"
  }
  $actual = (Get-Item $dest).Length
  if ($actual -ne $font.Length) {
    Remove-Item -Force $dest
    throw "Length mismatch for $($font.Name): expected $($font.Length), got $actual"
  }
  $hash = (Get-FileHash -Path $dest -Algorithm SHA256).Hash
  if ($hash -ne $font.Sha256) {
    Remove-Item -Force $dest
    throw "SHA256 mismatch for $($font.Name): expected $($font.Sha256), got $hash"
  }
  Write-Host "OK $($font.Name) ($actual bytes, sha256 verified)"
}

# SutonnyMJ is staged from the owner's legacy installation, never from the
# network and never committed. Absence is a warning, not an error: the build
# continues with Kalpurush ANSI as the Bijoy fallback.
$SutonnyName   = 'SutonnyMJ Regular.ttf'
$SutonnyLength = 73656
$SutonnySha256 = 'D58D9A45311447301426649C641FE148010163594AE57A4094AC086B21D52BDC'
$SutonnyDest   = Join-Path $resolved $SutonnyName
$legacyHome = $env:OKKHOR_LEGACY_HOME
if ([string]::IsNullOrWhiteSpace($legacyHome)) {
  Write-Warning "OKKHOR_LEGACY_HOME is not set; skipping $SutonnyName (Kalpurush ANSI remains the Bijoy fallback)."
} else {
  $legacyFont = Join-Path $legacyHome 'Res\Fonts\SutonnyMJ Regular.ttf'
  if (-not (Test-Path $legacyFont)) {
    Write-Warning "Legacy font not found at $legacyFont; skipping $SutonnyName (Kalpurush ANSI remains the Bijoy fallback)."
  } else {
    Copy-Item -Force $legacyFont $SutonnyDest
    $actual = (Get-Item $SutonnyDest).Length
    if ($actual -ne $SutonnyLength) {
      Remove-Item -Force $SutonnyDest
      throw "Length mismatch for $SutonnyName - expected $SutonnyLength, got $actual"
    }
    $hash = (Get-FileHash -Path $SutonnyDest -Algorithm SHA256).Hash
    if ($hash -ne $SutonnySha256) {
      Remove-Item -Force $SutonnyDest
      throw "SHA256 mismatch for $SutonnyName - expected $SutonnySha256, got $hash"
    }
    Write-Host "OK $SutonnyName ($actual bytes, sha256 verified, staged from legacy installation)"
  }
}

Write-Host "All fonts verified in $resolved"
