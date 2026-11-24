#requires -Version 5.1
<#
.SYNOPSIS
    Lists or restores an Okkhor data-home backup made by import-legacy.ps1.

.DESCRIPTION
    `scripts/import-legacy.ps1 -Apply` snapshots the data home to
    `<DataHome>\backups\legacy-<yyyyMMdd-HHmmss>\` before writing its report.
    This script lists those snapshots and restores one.

    Safety rules:

    - The chosen backup must be a direct child of `<DataHome>\backups` and must
      match the generated naming pattern. Paths that resolve outside the data
      home are refused.
    - A restore refuses to overwrite existing files unless -Force is passed.
    - Every restore asks for explicit confirmation (type RESTORE) unless -Force
      is passed. There is no prompt when stdin is not interactive: use -Force
      for scripted restores.
    - Restore copies files back; it never deletes files that are not in the
      snapshot. `backup-manifest.json` is report metadata and is not restored.

.PARAMETER DataHome
    Okkhor data home. Defaults to `$env:OKKHOR_DATA_HOME`, then
    `%LOCALAPPDATA%\Okkhor`, then `%APPDATA%\Okkhor`.

.PARAMETER BackupName
    Backup folder name under `<DataHome>\backups`, for example
    `legacy-20261124-101500`. Required for a restore.

.PARAMETER List
    Print the available backups and exit.

.PARAMETER Force
    Skip the confirmation prompt and allow overwriting existing files.

.EXAMPLE
    .\scripts\rollback-legacy-import.ps1 -List

.EXAMPLE
    .\scripts\rollback-legacy-import.ps1 -BackupName legacy-20261124-101500

.NOTES
    Exit codes: 0 success or listing; 1 unavailable/aborted; 2 backup not found;
    3 refused to overwrite without -Force; 4 unsafe path or bad arguments.
#>
[CmdletBinding()]
param(
    [string]$DataHome = '',

    [string]$BackupName = '',

    [switch]$List,

    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$BackupPattern = '^legacy-\d{8}-\d{6}(-\d{2})?$'

function Resolve-FullPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    $expanded = [Environment]::ExpandEnvironmentVariables($Path)
    if (-not [IO.Path]::IsPathRooted($expanded)) {
        $expanded = Join-Path (Get-Location).ProviderPath $expanded
    }
    return [IO.Path]::GetFullPath($expanded)
}

function Test-PathUnder {
    param(
        [Parameter(Mandatory = $true)][string]$Child,
        [Parameter(Mandatory = $true)][string]$Parent
    )

    $childFull = ([IO.Path]::GetFullPath($Child)).TrimEnd('\')
    $parentFull = ([IO.Path]::GetFullPath($Parent)).TrimEnd('\')
    if ($childFull.Equals($parentFull, [StringComparison]::OrdinalIgnoreCase)) { return $true }
    $prefix = $parentFull + '\'
    return $childFull.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)
}

function Get-BackupSummary {
    param([Parameter(Mandatory = $true)][string]$Path)

    $files = 0
    $bytes = [int64]0
    foreach ($file in @(Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue)) {
        if ($file.Name -eq 'backup-manifest.json' -and $file.DirectoryName -eq $Path) { continue }
        $files++
        $bytes += [int64]$file.Length
    }

    $created = $null
    $manifestPath = Join-Path $Path 'backup-manifest.json'
    if (Test-Path -LiteralPath $manifestPath -PathType Leaf) {
        try {
            $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
            $created = $manifest.createdUtc
            if ($created -is [datetime]) {
                $created = $created.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
            }
        }
        catch {
            $created = $null
        }
    }

    return [pscustomobject]@{
        Name    = (Split-Path -Leaf $Path)
        Created = $created
        Files   = $files
        Bytes   = $bytes
    }
}

# ---------------------------------------------------------------- data home ---

if (-not $DataHome -or $DataHome.Trim().Length -eq 0) {
    $override = [Environment]::GetEnvironmentVariable('OKKHOR_DATA_HOME')
    if ($override -and $override.Trim().Length -gt 0) {
        $DataHome = $override
    }
    elseif ($env:LOCALAPPDATA) {
        $DataHome = Join-Path $env:LOCALAPPDATA 'Okkhor'
    }
    elseif ($env:APPDATA) {
        $DataHome = Join-Path $env:APPDATA 'Okkhor'
    }
    else {
        [Console]::Error.WriteLine('error: no data home; pass -DataHome or set OKKHOR_DATA_HOME.')
        exit 4
    }
}

$dataRoot = (Resolve-FullPath -Path $DataHome).TrimEnd('\')
if ($dataRoot -match '^[A-Za-z]:$') {
    [Console]::Error.WriteLine('error: refusing to use a drive root as the data home.')
    exit 4
}
if (-not (Test-Path -LiteralPath $dataRoot -PathType Container)) {
    [Console]::Error.WriteLine('error: data home does not exist; nothing to restore.')
    exit 1
}

$backupRoot = Join-Path $dataRoot 'backups'
if (-not (Test-Path -LiteralPath $backupRoot -PathType Container)) {
    if ($List -or -not $BackupName) {
        Write-Host 'No backups found under the data home.' -ForegroundColor Yellow
        exit 0
    }
    [Console]::Error.WriteLine('error: no backups directory under the data home.')
    exit 2
}

if ($List -or -not $BackupName) {
    $available = @(Get-ChildItem -LiteralPath $backupRoot -Directory -Force |
            Where-Object { $_.Name -match $BackupPattern } |
            Sort-Object Name -Descending |
            ForEach-Object { Get-BackupSummary -Path $_.FullName })

    if ($available.Count -eq 0) {
        Write-Host 'No legacy-import backups found under the data home.' -ForegroundColor Yellow
        exit 0
    }

    Write-Host ''
    Write-Host 'Legacy-import backups:' -ForegroundColor Cyan
    foreach ($backup in $available) {
        Write-Host ("  {0}  files={1}  bytes={2}  created={3}" -f `
                $backup.Name, $backup.Files, $backup.Bytes, $backup.Created)
    }
    Write-Host ''
    Write-Host 'Restore with: .\scripts\rollback-legacy-import.ps1 -BackupName <name>' -ForegroundColor DarkGray
    Write-Host ''
    exit 0
}

# -------------------------------------------------------------------- select ---

if ($BackupName -match '[\\/]' -or $BackupName -notmatch $BackupPattern) {
    [Console]::Error.WriteLine('error: invalid backup name.')
    [Console]::Error.WriteLine('       expected a name like legacy-20261124-101500 (no path separators).')
    exit 4
}

$backupPath = Join-Path $backupRoot $BackupName
if (-not (Test-PathUnder -Child $backupPath -Parent $backupRoot)) {
    [Console]::Error.WriteLine('error: backup path escapes the data home.')
    exit 4
}
if (-not (Test-Path -LiteralPath $backupPath -PathType Container)) {
    [Console]::Error.WriteLine("error: backup not found: $BackupName")
    exit 2
}

$entries = New-Object System.Collections.Generic.List[object]
foreach ($file in @(Get-ChildItem -LiteralPath $backupPath -Recurse -File -Force | Sort-Object FullName)) {
    $relative = $file.FullName.Substring($backupPath.Length).TrimStart('\')
    if ($relative -eq 'backup-manifest.json') { continue }
    if ($relative -match '(^|[\\/])\.\.([\\/]|$)') {
        [Console]::Error.WriteLine('error: backup contains an unsafe relative path.')
        exit 4
    }
    $destination = Join-Path $dataRoot $relative
    if (-not (Test-PathUnder -Child $destination -Parent $dataRoot)) {
        [Console]::Error.WriteLine('error: restore target escapes the data home.')
        exit 4
    }
    $entries.Add([pscustomobject]@{
            Relative    = $relative
            Source      = $file.FullName
            Destination = $destination
            Bytes       = [int64]$file.Length
            Conflict    = (Test-Path -LiteralPath $destination)
        })
}

$conflicts = @($entries | Where-Object { $_.Conflict })
if ($conflicts.Count -gt 0 -and -not $Force) {
    [Console]::Error.WriteLine("error: restore would overwrite $($conflicts.Count) existing file(s).")
    [Console]::Error.WriteLine('       re-run with -Force after reviewing the data home.')
    exit 3
}

$summary = Get-BackupSummary -Path $backupPath
Write-Host ''
Write-Host "Backup:  $($summary.Name)" -ForegroundColor Cyan
Write-Host "Created: $($summary.Created)"
Write-Host "Files:   $($summary.Files)"
Write-Host "Bytes:   $($summary.Bytes)"
Write-Host "Target:  data home"
if ($conflicts.Count -gt 0) {
    Write-Host "Overwrite: $($conflicts.Count) existing file(s) (-Force)" -ForegroundColor Yellow
}

if (-not $Force) {
    if (-not [Environment]::UserInteractive -or $Host.Name -eq 'ServerRemoteHost') {
        [Console]::Error.WriteLine('error: restore needs confirmation but the host is not interactive; use -Force.')
        exit 1
    }
    $answer = Read-Host -Prompt 'Type RESTORE to continue'
    if ($answer -cne 'RESTORE') {
        Write-Host 'Restore cancelled.' -ForegroundColor Yellow
        exit 1
    }
}

$restored = 0
foreach ($entry in $entries) {
    $parent = Split-Path -Parent $entry.Destination
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    Copy-Item -LiteralPath $entry.Source -Destination $entry.Destination -Force
    $restored++
}

Write-Host ''
Write-Host "Restored $restored file(s) from $($summary.Name)." -ForegroundColor Green
Write-Host 'Files that are not in the snapshot were left in place.' -ForegroundColor DarkGray
Write-Host ''

exit 0
