<#
.SYNOPSIS
  Sign Okkhor executables with Authenticode and verify the result.

.DESCRIPTION
  Reusable signer for the portable beta. Signs one or more files with
  signtool.exe using either a PFX file or a certificate thumbprint from the
  current user's certificate store, applies an RFC 3161 timestamp, and verifies
  every signature with `signtool verify /pa /all`.

  The PFX password is never printed or written to a file. Supply it with
  -PfxPassword (SecureString) or the OKKHOR_SIGN_PFX_PASSWORD environment
  variable. signtool exposes `/p` on its command line while it runs, so prefer a
  hardware token or a certificate-store thumbprint for a real release.

  This file is dot-sourced by scripts/build-beta.ps1. When dot-sourced it only
  defines functions and signs nothing.

.EXAMPLE
  .\scripts\sign-artifact.ps1 `
    -Path dist\Okkhor-0.8.0-win64\Okkhor.exe `
    -CertificateThumbprint 0123456789ABCDEF0123456789ABCDEF01234567

.EXAMPLE
  $env:OKKHOR_SIGN_PFX_PASSWORD = '<from your secret store>'
  .\scripts\sign-artifact.ps1 -Path a.exe,b.exe -PfxPath C:\keys\okkhor.pfx
#>
[CmdletBinding()]
param(
  [Parameter(Position = 0)]
  [string[]]$Path,
  [string]$PfxPath,
  [System.Security.SecureString]$PfxPassword,
  [string]$CertificateThumbprint,
  [string]$TimestampUrl = 'http://timestamp.digicert.com',
  [string]$SignToolPath,
  [switch]$SkipVerify
)

# Shared helpers (also used by build-beta.ps1 through dot-sourcing).

function Resolve-SignToolPath {
  [CmdletBinding()]
  param(
    [string]$Path,
    [string]$Architecture = 'x64'
  )

  if ($Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
      throw "signtool.exe was not found at '$Path'."
    }
    return (Resolve-Path -LiteralPath $Path).Path
  }

  $onPath = Get-Command 'signtool.exe' -ErrorAction SilentlyContinue
  if ($onPath) { return $onPath.Source }

  $roots = @()
  foreach ($base in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
    if (-not $base) { continue }
    $kits = Join-Path $base 'Windows Kits\10\bin'
    if (Test-Path -LiteralPath $kits) { $roots += $kits }
  }

  foreach ($root in $roots) {
    $sdkVersions = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
      Where-Object { $_.Name -match '^10\.\d+' } |
      Sort-Object { [version]$_.Name } -Descending
    foreach ($sdk in $sdkVersions) {
      $candidate = Join-Path $sdk.FullName "$Architecture\signtool.exe"
      if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
  }

  throw ('signtool.exe was not found on PATH or in the installed Windows 10/11 SDK. ' +
    'A signature was requested, so signing stopped instead of producing an unsigned artifact. ' +
    'Install the Windows SDK signing tools or pass -SignToolPath.')
}

function ConvertFrom-SecureStringPlain {
  param(
    [Parameter(Mandatory = $true)]
    [System.Security.SecureString]$Secure
  )

  $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($Secure)
  try {
    [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
  } finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
  }
}

function Invoke-AuthenticodeSign {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path,
    [string]$PfxPath,
    [System.Security.SecureString]$PfxPassword,
    [string]$CertificateThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com',
    [string]$SignToolPath,
    [switch]$SkipVerify
  )

  $files = @()
  foreach ($item in $Path) {
    if (-not (Test-Path -LiteralPath $item -PathType Leaf)) {
      throw "Cannot sign a missing file: $item"
    }
    $files += (Resolve-Path -LiteralPath $item).Path
  }
  if ($files.Count -eq 0) { throw 'No files to sign.' }

  if ($PfxPath -and $CertificateThumbprint) {
    throw 'Pass either -PfxPath or -CertificateThumbprint, not both.'
  }
  if (-not $PfxPath -and -not $CertificateThumbprint) {
    throw 'No certificate selected: pass -PfxPath or -CertificateThumbprint.'
  }
  if (-not $TimestampUrl) {
    throw '-TimestampUrl is required. An untimestamped signature stops validating when the certificate expires.'
  }

  $signTool = Resolve-SignToolPath -Path $SignToolPath

  $password = $PfxPassword
  if ($PfxPath -and -not $password -and $env:OKKHOR_SIGN_PFX_PASSWORD) {
    $password = ConvertTo-SecureString -String $env:OKKHOR_SIGN_PFX_PASSWORD -AsPlainText -Force
  }

  $signArgs = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/tr', $TimestampUrl)
  if ($PfxPath) {
    if (-not (Test-Path -LiteralPath $PfxPath -PathType Leaf)) {
      throw "PFX file not found: $PfxPath"
    }
    $signArgs += @('/f', (Resolve-Path -LiteralPath $PfxPath).Path)
    if ($password) {
      $plain = ConvertFrom-SecureStringPlain -Secure $password
      try {
        $signArgs += @('/p', $plain)
      } finally {
        $plain = $null
      }
    } else {
      Write-Warning 'No PFX password supplied (-PfxPassword or OKKHOR_SIGN_PFX_PASSWORD); signtool will try the PFX with no password.'
    }
    $signerLabel = 'a PFX file'
  } else {
    $thumbprint = ($CertificateThumbprint -replace '\s', '').ToUpperInvariant()
    if ($thumbprint -notmatch '^([0-9A-F]{40}|[0-9A-F]{64})$') {
      throw 'Invalid certificate thumbprint. Expected 40 (SHA-1) or 64 (SHA-256) hex characters.'
    }
    $signArgs += @('/sha1', $thumbprint)
    $signerLabel = 'a certificate thumbprint'
  }

  $signArgs += $files
  Write-Host "signtool: signing $($files.Count) file(s) with $signerLabel." -ForegroundColor Cyan
  & $signTool @signArgs
  $signExit = $LASTEXITCODE
  $signArgs = $null
  if ($signExit -ne 0) {
    throw "signtool sign failed (exit $signExit). No password or secret is printed by this script."
  }

  if ($SkipVerify) {
    Write-Warning 'Signature verification skipped (-SkipVerify). Do not publish an unverified signature.'
    return
  }

  foreach ($file in $files) {
    $output = (& $signTool verify /pa /all $file 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
      throw ("Signature verification failed for '$file':`n$output`n" +
        'A self-signed certificate fails /pa verification on machines that do not trust its root. ' +
        'Use a publicly trusted Authenticode certificate for a release.')
    }
    if ($output -notmatch 'timestamp') {
      Write-Warning "The signature for '$file' does not report a timestamp; check -TimestampUrl."
    }
  }

  Write-Host "signtool: verified $($files.Count) signature(s)." -ForegroundColor Green
}

# Dot-sourced (build-beta.ps1): library only.
if ($MyInvocation.InvocationName -eq '.') { return }

if (-not $Path -or $Path.Count -eq 0) {
  throw 'No files to sign. Pass -Path <file>[,<file>...].'
}

Invoke-AuthenticodeSign `
  -Path $Path `
  -PfxPath $PfxPath `
  -PfxPassword $PfxPassword `
  -CertificateThumbprint $CertificateThumbprint `
  -TimestampUrl $TimestampUrl `
  -SignToolPath $SignToolPath `
  -SkipVerify:$SkipVerify
