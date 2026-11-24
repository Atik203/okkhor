#requires -Version 5.1
<#
.SYNOPSIS
  Fresh-extraction QA for the Okkhor 0.8 portable artifact.

.DESCRIPTION
  Expands the portable zip into a fresh temp folder, checks the expected
  binaries/fonts, runs the engine version identity and self-check, starts
  Okkhor.exe with an isolated data home, and verifies that the Settings and
  Quick Bar windows appear. Stops every process it started.

  This is a smoke test, not the owner real-key pass.
#>
[CmdletBinding()]
param(
  [string]$Zip = '',
  [string]$Version = '0.8.0'
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Zip) {
  $Zip = Join-Path $RepoRoot "dist\Okkhor-$Version-win64-portable.zip"
}
if (-not (Test-Path -LiteralPath $Zip -PathType Leaf)) {
  throw "Artifact not found: $Zip"
}

$temp = Join-Path $env:TEMP "okkhor-$Version-artifact-qa"
if (Test-Path -LiteralPath $temp) {
  Remove-Item -LiteralPath $temp -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $temp | Out-Null
Expand-Archive -LiteralPath $Zip -DestinationPath $temp -Force

$app = Join-Path $temp 'Okkhor.exe'
$engine = Join-Path $temp 'bin\okkhor-engine.exe'
$worker = Join-Path $temp 'bin\okkhor-worker.exe'
$fonts = Join-Path $temp 'fonts'
foreach ($required in @($app, $engine, $worker)) {
  if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
    throw "Missing from artifact: $required"
  }
}
$fontCount = @(Get-ChildItem -LiteralPath $fonts -Filter '*.ttf' -File -ErrorAction SilentlyContinue).Count
if ($fontCount -lt 4) {
  throw "Expected at least 4 bundled fonts, found $fontCount."
}

$actual = (& $engine --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $actual -ne "okkhor $Version") {
  throw "Engine version mismatch: '$actual' (exit $LASTEXITCODE), expected 'okkhor $Version'."
}
& $engine self-check
if ($LASTEXITCODE -ne 0) {
  throw "Engine self-check failed (exit $LASTEXITCODE)."
}

$dataHome = Join-Path $temp 'data'
$env:OKKHOR_DATA_HOME = $dataHome
$started = @()
try {
  $started += Start-Process -FilePath $app -PassThru
  Start-Sleep -Seconds 9

  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class OkkhorArtifactWindows {
  public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  public static string Find() {
    var result = new StringBuilder();
    EnumWindows((hwnd, lparam) => {
      var text = new StringBuilder(256);
      GetWindowText(hwnd, text, 256);
      var title = text.ToString();
      if (title.Contains("Okkhor") && IsWindowVisible(hwnd)) {
        result.Append(title + ";");
      }
      return true;
    }, IntPtr.Zero);
    return result.ToString();
  }
}
'@

  $windows = [OkkhorArtifactWindows]::Find()
  if ($windows -notmatch 'Okkhor Settings') {
    throw "Settings window did not appear. Visible Okkhor windows: '$windows'"
  }
  if ($windows -notmatch 'Okkhor Quick Bar') {
    throw "Quick Bar window did not appear. Visible Okkhor windows: '$windows'"
  }
  Write-Host "Windows: $windows" -ForegroundColor Green
  Write-Host "Artifact QA passed: $Version, $fontCount fonts, engine/worker present." -ForegroundColor Green
}
finally {
  Get-Process okkhor-engine,Okkhor -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -like "$temp*" } |
    Stop-Process -Force -ErrorAction SilentlyContinue
  Remove-Item Env:OKKHOR_DATA_HOME -ErrorAction SilentlyContinue
}
