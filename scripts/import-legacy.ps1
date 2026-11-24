#requires -Version 5.1
<#
.SYNOPSIS
    Read-only inspection and migration report for the legacy Kontho install.

.DESCRIPTION
    Inspects an owner-provided Kontho installation without modifying it and
    produces a machine-readable plus human report of what Okkhor 0.8 can and
    cannot reuse.

    Safety rules that this script enforces or documents:

    - The legacy folder is only read. No file, attribute, timestamp, or ACL is
      changed. The script refuses a DataHome that lives inside the legacy
      folder, and refuses a legacy folder inside the DataHome.
    - Python pickle data (`settings.pkl`, `*.nmsLayout`, `*.nmsEnco`,
      `SentenceHistory.pkl`) is measured and hashed only. It is never
      deserialized.
    - `*.bin` files are classified from their 4-byte magic. Unknown or
      recognized-but-unparsed formats are reported, never guessed at.
    - `CustomWords.json` is a JSON string array, not a roman -> Bangla mapping.
      Only the count and coarse category counts are written; entry text is
      never printed, logged, or written to the report.
    - Nothing proprietary is copied into this repository. The report is written
      under the Okkhor data home only when -Apply is set.

    The report is honest: in 0.8 nothing is imported automatically. The report
    records counts, SHA256s, supported/partial/unsupported status, and the next
    owner action.

.PARAMETER LegacyHome
    Path to the read-only Kontho installation (the folder containing
    `Kontho.exe` and `Dictionaries\`). Required.

.PARAMETER DataHome
    Okkhor data home that receives `import\legacy-import-report.{json,md}`.
    Defaults to `$env:OKKHOR_DATA_HOME`, then `%LOCALAPPDATA%\Okkhor` (the
    documented Okkhor data home), then `%APPDATA%\Okkhor`.

.PARAMETER Apply
    Write the reports under `<DataHome>\import\` and, when backup is enabled,
    snapshot the existing data home first. Without -Apply the script prints the
    same summary and writes no report.

.PARAMETER Backup
    On by default in -Apply mode. Creates
    `<DataHome>\backups\legacy-<yyyyMMdd-HHmmss>\` from the current data home
    before writing. Pass `-Backup:$false` to skip the snapshot.

.EXAMPLE
    .\scripts\import-legacy.ps1 -LegacyHome 'D:\Kontho'

.EXAMPLE
    .\scripts\import-legacy.ps1 -LegacyHome 'D:\Kontho' -DataHome 'C:\Temp\okkhor' -Apply

.NOTES
    Exit codes: 0 report produced (even with unsupported formats);
    2 invalid legacy home; 3 unwritable or unsafe data home; 4 bad arguments.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$LegacyHome,

    [string]$DataHome = '',

    [switch]$Apply,

    [switch]$Backup = $true
)

$ErrorActionPreference = 'Stop'

$ReportSchema = 'okkhor.legacy-import-report/1'
$BackupSchema = 'okkhor.legacy-import-backup/1'

function Resolve-FullPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    $expanded = [Environment]::ExpandEnvironmentVariables($Path)
    if (-not [IO.Path]::IsPathRooted($expanded)) {
        $expanded = Join-Path (Get-Location).ProviderPath $expanded
    }
    return [IO.Path]::GetFullPath($expanded)
}

function Add-TrailingSeparator {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not $Path.EndsWith('\')) { return $Path + '\' }
    return $Path
}

# True when $Child equals $Parent or lives below it (Windows, case-insensitive).
function Test-PathUnder {
    param(
        [Parameter(Mandatory = $true)][string]$Child,
        [Parameter(Mandatory = $true)][string]$Parent
    )

    $childFull = (Add-TrailingSeparator ([IO.Path]::GetFullPath($Child))).TrimEnd('\')
    $parentFull = (Add-TrailingSeparator ([IO.Path]::GetFullPath($Parent))).TrimEnd('\')
    if ($childFull.Equals($parentFull, [StringComparison]::OrdinalIgnoreCase)) { return $true }
    $prefix = $parentFull + '\'
    return $childFull.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)
}

function Get-RelativeToRoot {
    param(
        [Parameter(Mandatory = $true)][string]$FullName,
        [Parameter(Mandatory = $true)][string]$RootWithSeparator
    )

    return $FullName.Substring($RootWithSeparator.Length)
}

function Read-MagicBytes {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [int]$Count = 8
    )

    $buffer = New-Object byte[] $Count
    $read = 0
    $stream = [IO.File]::Open(
        $Path,
        [IO.FileMode]::Open,
        [IO.FileAccess]::Read,
        [IO.FileShare]::ReadWrite)
    try {
        $read = $stream.Read($buffer, 0, $Count)
    }
    finally {
        $stream.Dispose()
    }

    if ($read -le 0) {
        return [pscustomobject]@{ Hex = ''; Ascii = ''; Bytes = @() }
    }

    $bytes = @($buffer[0..($read - 1)])
    $hex = ($bytes | ForEach-Object { $_.ToString('X2') }) -join ''
    $ascii = -join ($bytes | ForEach-Object {
            if ($_ -ge 32 -and $_ -le 126) { [char]$_ } else { '.' }
        })
    return [pscustomobject]@{ Hex = $hex; Ascii = $ascii; Bytes = $bytes }
}

function Get-ContainerKind {
    param([Parameter(Mandatory = $true)][object]$Magic)

    $bytes = @($Magic.Bytes)
    if ($bytes.Count -ge 4) {
        $four = [Text.Encoding]::ASCII.GetString([byte[]]$bytes[0..3])
        if ($four -eq 'WTR1') { return 'wtr1-word-trie' }
        if ($four -eq 'STRI') { return 'stri-sentence-trie' }
        if ($four -eq 'MZ') { return 'pe-executable' }
    }
    if ($bytes.Count -ge 2 -and $bytes[0] -eq 0x80) {
        $protocol = [int]$bytes[1]
        if ($protocol -ge 2 -and $protocol -le 5) {
            return "python-pickle-protocol-$protocol"
        }
    }
    return 'unknown'
}

function Get-FileFacts {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$RootWithSeparator,
        [int]$MagicBytes = 8
    )

    $item = Get-Item -LiteralPath $Path
    $magic = Read-MagicBytes -Path $item.FullName -Count $MagicBytes
    return [pscustomobject]@{
        File       = Get-RelativeToRoot -FullName $item.FullName -RootWithSeparator $RootWithSeparator
        Bytes      = [int64]$item.Length
        Sha256     = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        MagicHex   = $magic.Hex
        MagicAscii = $magic.Ascii
        Container  = Get-ContainerKind -Magic $magic
    }
}

function Get-CustomWordCategories {
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$Entries)

    $phone = 0
    $domain = 0
    $other = 0
    $separator = 0

    foreach ($entry in $Entries) {
        $text = [string]$entry
        if ($text -match '^\+?[0-9][0-9\s\-\(\)\.]{5,}$') {
            $phone++
        }
        elseif ($text -match '^[^@\s]+@[^@\s]+\.[A-Za-z]{2,}$' -or
            $text -match '^[A-Za-z0-9\-]+(\.[A-Za-z0-9\-]+)+$') {
            $domain++
        }
        else {
            $other++
        }
        if ($text -match '=>|->|=|:') { $separator++ }
    }

    return [ordered]@{
        phoneLike        = $phone
        domainLike       = $domain
        other            = $other
        withSeparator    = $separator
        romanToBanglaMap = $false
        note             = 'CustomWords.json is a JSON string array. Entry text is never read out, logged, or written; counts only.'
    }
}

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Text
    )

    $encoding = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($Path, $Text, $encoding)
}

function Get-UtcStamp {
    return (Get-Date).ToUniversalTime()
}

# ---------------------------------------------------------------- arguments ---

$legacyRoot = Resolve-FullPath -Path $LegacyHome
if (-not (Test-Path -LiteralPath $legacyRoot -PathType Container)) {
    [Console]::Error.WriteLine("error: legacy home is not a directory: $LegacyHome")
    exit 2
}
$legacyRoot = [IO.Path]::GetFullPath($legacyRoot)
if (-not $legacyRoot.EndsWith('\')) { $legacyRoot += '\' }

$dictionariesRoot = Join-Path $legacyRoot 'Dictionaries'
$langsRoot = Join-Path $dictionariesRoot 'Langs'
$exePath = Join-Path $legacyRoot 'Kontho.exe'

$installMarkers = New-Object System.Collections.Generic.List[string]
if (Test-Path -LiteralPath $exePath -PathType Leaf) { $installMarkers.Add('Kontho.exe') }
if (Test-Path -LiteralPath $dictionariesRoot -PathType Container) { $installMarkers.Add('Dictionaries\') }
if (Test-Path -LiteralPath $langsRoot -PathType Container) { $installMarkers.Add('Dictionaries\Langs\') }
if (Test-Path -LiteralPath (Join-Path $dictionariesRoot 'CustomWords.json') -PathType Leaf) {
    $installMarkers.Add('Dictionaries\CustomWords.json')
}

$looksLikeKontho = (Test-Path -LiteralPath $langsRoot -PathType Container) -and
    ((Test-Path -LiteralPath $exePath -PathType Leaf) -or
        (Test-Path -LiteralPath (Join-Path $dictionariesRoot 'CustomWords.json') -PathType Leaf))

if (-not $looksLikeKontho) {
    [Console]::Error.WriteLine('error: legacy home does not look like a Kontho install.')
    [Console]::Error.WriteLine('       expected Dictionaries\Langs\ plus Kontho.exe or Dictionaries\CustomWords.json.')
    exit 2
}

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

$dataRoot = Resolve-FullPath -Path $DataHome
if (-not $dataRoot.EndsWith('\')) { $dataRoot += '\' }
$dataRootTrimmed = $dataRoot.TrimEnd('\')

if ($dataRootTrimmed -match '^[A-Za-z]:$') {
    [Console]::Error.WriteLine('error: refusing to use a drive root as the data home.')
    exit 4
}
if ((Test-PathUnder -Child $dataRootTrimmed -Parent $legacyRoot) -or
    (Test-PathUnder -Child $legacyRoot -Parent $dataRootTrimmed)) {
    [Console]::Error.WriteLine('error: data home and legacy home must not overlap.')
    [Console]::Error.WriteLine('       the legacy install is read-only; choose a separate -DataHome.')
    exit 4
}

try {
    New-Item -ItemType Directory -Force -Path $dataRootTrimmed -ErrorAction Stop | Out-Null
    $probe = Join-Path $dataRootTrimmed ('.okkhor-write-probe-' + [guid]::NewGuid().ToString('N'))
    [IO.File]::WriteAllText($probe, 'probe')
    Remove-Item -LiteralPath $probe -Force
}
catch {
    [Console]::Error.WriteLine("error: data home is not writable: $DataHome")
    exit 3
}

$now = Get-UtcStamp
$installFolder = Split-Path -Leaf $legacyRoot.TrimEnd('\')

# ------------------------------------------------------------------- inspect ---

$jsonTargets = New-Object System.Collections.Generic.List[object]
$jsonTargets.Add([pscustomobject]@{ Label = 'Bangla metadata'; Path = (Join-Path $langsRoot 'Bangla\Bangla.json') })
$jsonTargets.Add([pscustomobject]@{ Label = 'English metadata'; Path = (Join-Path $langsRoot 'English\English.json') })
$jsonTargets.Add([pscustomobject]@{ Label = 'Abbreviations'; Path = (Join-Path $dictionariesRoot 'Abbreviations.json') })
$jsonTargets.Add([pscustomobject]@{ Label = 'Custom words'; Path = (Join-Path $dictionariesRoot 'CustomWords.json') })
$speechMaps = @(Get-ChildItem -LiteralPath $langsRoot -Directory -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName 'Speech Map.json' } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Sort-Object)
foreach ($speechMap in $speechMaps) {
    $language = Split-Path -Leaf (Split-Path -Parent $speechMap)
    $jsonTargets.Add([pscustomobject]@{ Label = "Speech map ($language)"; Path = $speechMap })
}

$jsonRows = New-Object System.Collections.Generic.List[object]
$targetedJson = New-Object System.Collections.Generic.List[string]
$customWordCategories = $null

foreach ($target in $jsonTargets) {
    $targetedJson.Add([IO.Path]::GetFullPath($target.Path))
    if (-not (Test-Path -LiteralPath $target.Path -PathType Leaf)) {
        $jsonRows.Add([pscustomobject]@{
                File        = $target.Label
                Status      = 'missing'
                Bytes       = $null
                Sha256      = $null
                TopLevel    = $null
                EntryCount  = $null
                Note        = 'expected file is absent from this install'
            })
        continue
    }

    $facts = Get-FileFacts -Path $target.Path -RootWithSeparator $legacyRoot
    $parseError = $null
    $topLevel = $null
    $entryCount = $null
    $document = $null

    try {
        $raw = [IO.File]::ReadAllText($target.Path)
        $document = $raw | ConvertFrom-Json
        if ($null -eq $document) {
            if ($raw.Trim() -eq '[]') { $topLevel = 'array'; $entryCount = 0 }
            elseif ($raw.Trim() -eq '{}') { $topLevel = 'object'; $entryCount = 0 }
            else { $topLevel = 'empty'; $entryCount = 0 }
        }
        elseif ($document -is [System.Array]) {
            $topLevel = 'array'
            $entryCount = @($document).Count
        }
        elseif ($document -is [pscustomobject]) {
            $topLevel = 'object'
            $entryCount = @($document.PSObject.Properties).Count
        }
        else {
            $topLevel = 'scalar'
            $entryCount = 1
        }
    }
    catch {
        $parseError = 'not valid JSON for this parser'
    }

    if ($target.Label -eq 'Custom words' -and $null -eq $parseError) {
        $entries = @()
        if ($document -is [System.Array]) { $entries = @($document) }
        $customWordCategories = Get-CustomWordCategories -Entries $entries
        $note = 'JSON string array. Counted and categorized only; entry text is never printed or written. No roman -> Bangla mapping exists in this file.'
    }
    elseif ($target.Label -eq 'Custom words') {
        $note = 'array could not be parsed; no entries were read out'
    }
    else {
        $note = 'metadata parsed for key/entry counts only; values are not copied'
    }

    $jsonRows.Add([pscustomobject]@{
            File       = $facts.File
            Label      = $target.Label
            Status     = $(if ($parseError) { 'unsupported' } else { 'inspected' })
            Bytes      = $facts.Bytes
            Sha256     = $facts.Sha256
            TopLevel   = $topLevel
            EntryCount = $entryCount
            Note       = $(if ($parseError) { $parseError } else { $note })
        })
}

$targetedJsonSet = @{}
foreach ($targeted in $targetedJson) { $targetedJsonSet[$targeted] = $true }

$otherJsonRows = New-Object System.Collections.Generic.List[object]
if (Test-Path -LiteralPath $dictionariesRoot -PathType Container) {
    foreach ($jsonFile in @(Get-ChildItem -LiteralPath $dictionariesRoot -Recurse -File -Filter *.json -ErrorAction SilentlyContinue |
                Sort-Object FullName)) {
        if ($targetedJsonSet.ContainsKey([IO.Path]::GetFullPath($jsonFile.FullName))) { continue }
        $facts = Get-FileFacts -Path $jsonFile.FullName -RootWithSeparator $legacyRoot
        $otherJsonRows.Add([pscustomobject]@{
                File      = $facts.File
                Status    = 'unsupported'
                Bytes     = $facts.Bytes
                Sha256    = $facts.Sha256
                Container = $facts.Container
                Note      = 'not targeted by the 0.8 importer; emoji, special-character, encoding, or prompt metadata that needs its own review'
            })
    }
}

$layoutRows = New-Object System.Collections.Generic.List[object]
if (Test-Path -LiteralPath $dictionariesRoot -PathType Container) {
    foreach ($layout in @(Get-ChildItem -LiteralPath $dictionariesRoot -Recurse -File -Filter *.nmsLayout -ErrorAction SilentlyContinue |
                Sort-Object FullName)) {
        $facts = Get-FileFacts -Path $layout.FullName -RootWithSeparator $legacyRoot
        $layoutRows.Add([pscustomobject]@{
                File       = $facts.File
                Bytes      = $facts.Bytes
                Sha256     = $facts.Sha256
                MagicHex   = $facts.MagicHex
                Container  = $facts.Container
                Status     = 'unsupported'
                Note       = $(if ($facts.Container -like 'python-pickle-*') {
                        'Python pickle container; not deserialized. Layout import needs a reviewed exporter or parser with tests.'
                    }
                    else { 'unknown container; not parsed' })
            })
    }
}

$encodingRows = New-Object System.Collections.Generic.List[object]
if (Test-Path -LiteralPath $dictionariesRoot -PathType Container) {
    foreach ($encoding in @(Get-ChildItem -LiteralPath $dictionariesRoot -Recurse -File -Filter *.nmsEnco -ErrorAction SilentlyContinue |
                Sort-Object FullName)) {
        $facts = Get-FileFacts -Path $encoding.FullName -RootWithSeparator $legacyRoot
        $encodingRows.Add([pscustomobject]@{
                File      = $facts.File
                Bytes     = $facts.Bytes
                Sha256    = $facts.Sha256
                MagicHex  = $facts.MagicHex
                Container = $facts.Container
                Status    = 'unsupported'
                Note      = $(if ($facts.Container -like 'python-pickle-*') {
                        'Python pickle container; not deserialized. Encoding import needs a reviewed exporter or parser with tests.'
                    }
                    else { 'unknown container; not parsed' })
            })
    }
}

$dictionaryRows = New-Object System.Collections.Generic.List[object]
$historyRows = New-Object System.Collections.Generic.List[object]
if (Test-Path -LiteralPath $dictionariesRoot -PathType Container) {
    foreach ($binFile in @(Get-ChildItem -LiteralPath $dictionariesRoot -Recurse -File -Filter *.bin -ErrorAction SilentlyContinue |
                Sort-Object FullName)) {
        $facts = Get-FileFacts -Path $binFile.FullName -RootWithSeparator $legacyRoot
        $isHistory = ($binFile.Name -like 'SentenceHistory.*') -or ($facts.Container -eq 'stri-sentence-trie')
        if ($isHistory) {
            $historyRows.Add([pscustomobject]@{
                    File      = $facts.File
                    Bytes     = $facts.Bytes
                    Sha256    = $facts.Sha256
                    MagicHex  = $facts.MagicHex
                    Container = $facts.Container
                    Status    = 'unsupported'
                    Note      = 'recognized as a sentence-history container; not imported because the format is not documented and the content is personal text'
                })
            continue
        }

        $status = 'unsupported'
        $note = switch ($facts.Container) {
            'wtr1-word-trie' {
                'recognized WTR1 word-trie container; not parsed or imported in 0.8 (no reviewed parser yet)'
            }
            'stri-sentence-trie' {
                'recognized STRI sentence-trie container; not imported'
            }
            default {
                'unknown magic; not parsed and not guessed at'
            }
        }
        $dictionaryRows.Add([pscustomobject]@{
                File      = $facts.File
                Bytes     = $facts.Bytes
                Sha256    = $facts.Sha256
                MagicHex  = $facts.MagicHex
                Container = $facts.Container
                Status    = $status
                Note      = $note
            })
    }
}

$pickleRows = New-Object System.Collections.Generic.List[object]
$settingsPath = Join-Path $legacyRoot 'Res\settings.pkl'
if (Test-Path -LiteralPath $settingsPath -PathType Leaf) {
    $facts = Get-FileFacts -Path $settingsPath -RootWithSeparator $legacyRoot
    $pickleRows.Add([pscustomobject]@{
            File      = $facts.File
            Kind      = 'settings'
            Bytes     = $facts.Bytes
            Sha256    = $facts.Sha256
            MagicHex  = $facts.MagicHex
            Container = $facts.Container
            Status    = 'unsupported'
            Note      = 'pickle not deserialized; size and SHA256 recorded only'
        })
}
else {
    $pickleRows.Add([pscustomobject]@{
            File      = 'Res\settings.pkl'
            Kind      = 'settings'
            Bytes     = $null
            Sha256    = $null
            MagicHex  = $null
            Container = $null
            Status    = 'missing'
            Note      = 'no settings.pkl in this install'
        })
}

foreach ($history in @(Get-ChildItem -LiteralPath $legacyRoot -Recurse -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like 'SentenceHistory.pkl' } |
            Sort-Object FullName)) {
    $facts = Get-FileFacts -Path $history.FullName -RootWithSeparator $legacyRoot
    $historyRows.Add([pscustomobject]@{
            File      = $facts.File
            Bytes     = $facts.Bytes
            Sha256    = $facts.Sha256
            MagicHex  = $facts.MagicHex
            Container = $facts.Container
            Status    = 'unsupported'
            Note      = 'pickle not deserialized; size and SHA256 recorded only'
        })
}

# -------------------------------------------------------------------- report ---

$unsupportedCount = @($layoutRows | Where-Object { $_.Status -eq 'unsupported' }).Count +
    @($encodingRows | Where-Object { $_.Status -eq 'unsupported' }).Count +
    @($dictionaryRows | Where-Object { $_.Status -eq 'unsupported' }).Count +
    @($otherJsonRows | Where-Object { $_.Status -eq 'unsupported' }).Count +
    @($pickleRows | Where-Object { $_.Status -eq 'unsupported' }).Count +
    @($historyRows | Where-Object { $_.Status -eq 'unsupported' }).Count

$counts = [ordered]@{
    jsonMetadata        = @($jsonRows | Where-Object { $_.Status -eq 'inspected' }).Count
    jsonMetadataMissing = @($jsonRows | Where-Object { $_.Status -eq 'missing' }).Count
    otherJson           = $otherJsonRows.Count
    layouts             = $layoutRows.Count
    encodings           = $encodingRows.Count
    dictionaries        = $dictionaryRows.Count
    pickleFiles         = @($pickleRows | Where-Object { $_.Status -eq 'unsupported' }).Count
    sentenceHistory     = $historyRows.Count
    inspected           = @($jsonRows | Where-Object { $_.Status -eq 'inspected' }).Count + $dictionaryRows.Count +
        $layoutRows.Count + $encodingRows.Count + $otherJsonRows.Count + $pickleRows.Count + $historyRows.Count
    imported            = 0
    unsupported         = $unsupportedCount
}

$capabilities = @(
    [ordered]@{
        area       = 'json-metadata'
        status     = 'partial'
        reason     = 'Bangla/English metadata, abbreviations, and speech maps parse and are counted, but no field is applied to Okkhor settings in 0.8.'
        ownerAction = 'Decide which metadata keys map to Okkhor settings; add a schema-validated mapper with tests before applying any value.'
    },
    [ordered]@{
        area       = 'custom-words'
        status     = 'partial'
        reason     = 'CustomWords.json is a JSON string array of words, not a roman -> Bangla mapping. It is counted and categorized, never printed.'
        ownerAction = 'Re-add personal words through the Okkhor word manager, or approve a reviewed matching scheme before an automatic import exists.'
    },
    [ordered]@{
        area       = 'dictionaries-wtr1'
        status     = 'unsupported'
        reason     = 'WTR1 word-trie files are recognized by magic but have no reviewed parser; 0.8 must not guess the format.'
        ownerAction = 'Write a bounds-checked WTR1 parser with golden tests, or keep legacy dictionary use behind the compatibility layer.'
    },
    [ordered]@{
        area       = 'layouts'
        status     = 'unsupported'
        reason     = '*.nmsLayout files are Python pickle (protocol 4). Deserializing them is untrusted-input execution.'
        ownerAction = 'Produce a one-time exporter that writes a documented JSON/CSV layout, then import that with tests.'
    },
    [ordered]@{
        area       = 'encodings'
        status     = 'unsupported'
        reason     = '*.nmsEnco files are Python pickle (protocol 4); several encodings are also plain JSON and need separate review.'
        ownerAction = 'Approve a documented encoding table export and add conversion tests against a golden corpus.'
    },
    [ordered]@{
        area       = 'settings'
        status     = 'unsupported'
        reason     = 'Res\settings.pkl is a pickle. It is hashed only and explicitly not deserialized.'
        ownerAction = 'Use a one-time exporter for known keys, or guided re-setup in Okkhor.'
    },
    [ordered]@{
        area       = 'sentence-history'
        status     = 'unsupported'
        reason     = 'STRI .bin history is recognized; .pkl history is pickle. Both contain personal text and no parser is approved.'
        ownerAction = 'Analyze STRI offline, import counts/recency only, encrypt the result, and let the user skip or clear it.'
    },
    [ordered]@{
        area       = 'auxiliary-json'
        status     = 'unsupported'
        reason     = 'Emoji, special-character, prompt, and encoding JSON files are outside the 0.8 importer target and have no reviewed consumer.'
        ownerAction = 'File a follow-up per asset family; keep them out of git until rights and schema are recorded.'
    }
)

$nextSteps = @(
    'Read the parity matrix in docs/parity/0.8-matrix.md before promising a migrated feature.',
    'Nothing was imported in this run: the report is evidence for the matrix, not a migration.',
    'For dictionaries, write a WTR1 parser with bounds checks and golden tests before importing any word.',
    'For layouts and encodings, ask the owner for a one-time exporter; never deserialize legacy pickle.',
    'Re-add custom words through the Okkhor word manager; this file has no roman -> Bangla pairs.',
    'Keep the backup until the user confirms the data home is correct; roll back with scripts/rollback-legacy-import.ps1.'
)

$stableReport = [ordered]@{
    schema       = $ReportSchema
    install      = [ordered]@{
        folder           = $installFolder
        looksLikeKontho  = $looksLikeKontho
        markers          = $installMarkers.ToArray()
        executableSha256 = $(if (Test-Path -LiteralPath $exePath -PathType Leaf) {
                (Get-FileHash -LiteralPath $exePath -Algorithm SHA256).Hash.ToLowerInvariant()
            }
            else { $null })
        executableBytes  = $(if (Test-Path -LiteralPath $exePath -PathType Leaf) { (Get-Item -LiteralPath $exePath).Length } else { $null })
    }
    safety       = [ordered]@{
        legacyWrites       = 'none'
        pickleDeserialized = $false
        wordTextWritten    = $false
        binariesCopied     = 0
    }
    counts       = $counts
    customWords  = $customWordCategories
    json         = $jsonRows.ToArray()
    otherJson    = $otherJsonRows.ToArray()
    layouts      = $layoutRows.ToArray()
    encodings    = $encodingRows.ToArray()
    dictionaries = $dictionaryRows.ToArray()
    settingsAndHistory = $pickleRows.ToArray() + $historyRows.ToArray()
    capabilities = $capabilities
    nextSteps    = $nextSteps
}

$fingerprintSource = $stableReport | ConvertTo-Json -Depth 10 -Compress
$fingerprintBytes = [Text.Encoding]::UTF8.GetBytes($fingerprintSource)
$sha = [Security.Cryptography.SHA256]::Create()
try {
    $contentFingerprint = ([BitConverter]::ToString($sha.ComputeHash($fingerprintBytes)) -replace '-', '').ToLowerInvariant()
}
finally {
    $sha.Dispose()
}

$report = [ordered]@{
    schema             = $ReportSchema
    generatedAtUtc     = $now.ToString('yyyy-MM-ddTHH:mm:ssZ')
    mode               = $(if ($Apply) { 'apply' } else { 'report-only' })
    contentFingerprint = $contentFingerprint
    install            = $stableReport.install
    safety             = $stableReport.safety
    counts             = $counts
    customWords        = $customWordCategories
    json               = $jsonRows.ToArray()
    otherJson          = $otherJsonRows.ToArray()
    layouts            = $layoutRows.ToArray()
    encodings          = $encodingRows.ToArray()
    dictionaries       = $dictionaryRows.ToArray()
    settingsAndHistory = $stableReport.settingsAndHistory
    capabilities       = $capabilities
    nextSteps          = $nextSteps
    backup             = $null
}

# --------------------------------------------------------------------- apply ---

$reportJsonPath = Join-Path (Join-Path $dataRootTrimmed 'import') 'legacy-import-report.json'
$reportMarkdownPath = Join-Path (Join-Path $dataRootTrimmed 'import') 'legacy-import-report.md'

if ($Apply) {
    if ($Backup) {
        $backupRoot = Join-Path $dataRootTrimmed 'backups'
        New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
        $stamp = $now.ToString('yyyyMMdd-HHmmss')
        $backupName = 'legacy-' + $stamp
        $suffix = 1
        while (Test-Path -LiteralPath (Join-Path $backupRoot $backupName)) {
            $suffix++
            $backupName = '{0}-{1:00}' -f ('legacy-' + $stamp), $suffix
        }
        $backupDir = Join-Path $backupRoot $backupName
        New-Item -ItemType Directory -Force -Path $backupDir | Out-Null

        $copied = New-Object System.Collections.Generic.List[object]
        $copiedBytes = [int64]0
        foreach ($child in @(Get-ChildItem -LiteralPath $dataRootTrimmed -Force |
                    Where-Object { $_.Name -ne 'backups' })) {
            Copy-Item -LiteralPath $child.FullName -Destination (Join-Path $backupDir $child.Name) -Recurse -Force
            if ($child.PSIsContainer) {
                foreach ($file in @(Get-ChildItem -LiteralPath $child.FullName -Recurse -File -Force)) {
                    $relative = $file.FullName.Substring($dataRoot.Length)
                    $copied.Add([pscustomobject]@{ file = $relative; bytes = [int64]$file.Length })
                    $copiedBytes += [int64]$file.Length
                }
            }
            else {
                $relative = $child.FullName.Substring($dataRoot.Length)
                $copied.Add([pscustomobject]@{ file = $relative; bytes = [int64]$child.Length })
                $copiedBytes += [int64]$child.Length
            }
        }

        $manifest = [ordered]@{
            schema       = $BackupSchema
            createdUtc   = $now.ToString('yyyy-MM-ddTHH:mm:ssZ')
            backupName   = $backupName
            sourceLeaf   = (Split-Path -Leaf $dataRootTrimmed)
            files        = $copied.Count
            bytes        = $copiedBytes
            entries      = $copied.ToArray()
        }
        Write-Utf8NoBom -Path (Join-Path $backupDir 'backup-manifest.json') -Text (($manifest | ConvertTo-Json -Depth 5) + "`n")

        $report.backup = [ordered]@{ created = $true; name = $backupName; files = $copied.Count; bytes = $copiedBytes }
    }
    else {
        $report.backup = [ordered]@{ created = $false; name = $null; files = 0; bytes = 0; note = 'disabled with -Backup:$false' }
    }

    $importDir = Join-Path $dataRootTrimmed 'import'
    New-Item -ItemType Directory -Force -Path $importDir | Out-Null
    Write-Utf8NoBom -Path $reportJsonPath -Text (($report | ConvertTo-Json -Depth 10) + "`n")

    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add('# Legacy import report')
    $lines.Add('')
    $lines.Add("Generated: $($report.generatedAtUtc)")
    $lines.Add("Mode: $($report.mode)")
    $lines.Add("Install folder: $installFolder")
    $lines.Add('')
    $lines.Add('This report is evidence, not a migration. Okkhor 0.8 imported no legacy')
    $lines.Add('binaries, layouts, dictionaries, settings, or history. Pickle data was')
    $lines.Add('hashed only and never deserialized; custom-word entry text was never')
    $lines.Add('printed or written.')
    $lines.Add('')
    $lines.Add('## Counts')
    $lines.Add('')
    $lines.Add('| Item | Count |')
    $lines.Add('| --- | --- |')
    $lines.Add("| JSON metadata inspected | $($counts.jsonMetadata) |")
    $lines.Add("| JSON metadata missing | $($counts.jsonMetadataMissing) |")
    $lines.Add("| Other JSON (out of scope) | $($counts.otherJson) |")
    $lines.Add("| Layouts (unsupported) | $($counts.layouts) |")
    $lines.Add("| Encodings (unsupported) | $($counts.encodings) |")
    $lines.Add("| Binary dictionaries (unsupported) | $($counts.dictionaries) |")
    $lines.Add("| Pickle files (not deserialized) | $($counts.pickleFiles) |")
    $lines.Add("| Sentence history (unsupported) | $($counts.sentenceHistory) |")
    $lines.Add("| Imported | $($counts.imported) |")
    $lines.Add('')
    $lines.Add("Content fingerprint: ``$contentFingerprint``")
    $lines.Add('')
    $lines.Add('## Capabilities')
    $lines.Add('')
    $lines.Add('| Area | Status | Reason | Next owner action |')
    $lines.Add('| --- | --- | --- | --- |')
    foreach ($capability in $capabilities) {
        $lines.Add("| $($capability.area) | $($capability.status) | $($capability.reason) | $($capability.ownerAction) |")
    }
    $lines.Add('')
    $lines.Add('## Files')
    $lines.Add('')
    $lines.Add('| Kind | File | Bytes | SHA256 | Container | Status |')
    $lines.Add('| --- | --- | --- | --- | --- | --- |')
    foreach ($row in $jsonRows) {
        $lines.Add("| json-metadata | $($row.File) | $($row.Bytes) | $($row.Sha256) | json | $($row.Status) |")
    }
    foreach ($row in $layoutRows) {
        $lines.Add("| layout | $($row.File) | $($row.Bytes) | $($row.Sha256) | $($row.Container) | $($row.Status) |")
    }
    foreach ($row in $encodingRows) {
        $lines.Add("| encoding | $($row.File) | $($row.Bytes) | $($row.Sha256) | $($row.Container) | $($row.Status) |")
    }
    foreach ($row in $dictionaryRows) {
        $lines.Add("| dictionary | $($row.File) | $($row.Bytes) | $($row.Sha256) | $($row.Container) | $($row.Status) |")
    }
    foreach ($row in $stableReport.settingsAndHistory) {
        $lines.Add("| settings-or-history | $($row.File) | $($row.Bytes) | $($row.Sha256) | $($row.Container) | $($row.Status) |")
    }
    $lines.Add('')
    if ($customWordCategories) {
        $lines.Add('## Custom words')
        $lines.Add('')
        $lines.Add('`CustomWords.json` is a JSON string array, not a roman -> Bangla')
        $lines.Add('mapping. Entry text is never read out. Coarse categories:')
        $lines.Add('')
        $lines.Add("| Category | Count |")
        $lines.Add('| --- | --- |')
        $lines.Add("| Phone-like | $($customWordCategories.phoneLike) |")
        $lines.Add("| Domain-like | $($customWordCategories.domainLike) |")
        $lines.Add("| Other | $($customWordCategories.other) |")
        $lines.Add("| Entries containing a separator | $($customWordCategories.withSeparator) |")
        $lines.Add('')
        $lines.Add('No entry is treated as a mapping, so this file cannot seed the')
        $lines.Add('transliteration dictionary by itself.')
        $lines.Add('')
    }
    $lines.Add('## Next steps')
    $lines.Add('')
    foreach ($step in $nextSteps) { $lines.Add("- $step") }
    $lines.Add('')
    $lines.Add('See `docs/parity/0.8-matrix.md` and `docs/compatibility.md`.')
    Write-Utf8NoBom -Path $reportMarkdownPath -Text (($lines -join "`n") + "`n")
}

# ------------------------------------------------------------------- console ---

$modeLabel = if ($Apply) { 'apply' } else { 'report-only' }
Write-Host ''
Write-Host "Legacy import report ($modeLabel)" -ForegroundColor Cyan
Write-Host ("  install:          {0} (looks like Kontho: {1})" -f $installFolder, $looksLikeKontho)
Write-Host ("  JSON metadata:    {0} inspected, {1} missing" -f $counts.jsonMetadata, $counts.jsonMetadataMissing)
Write-Host ("  other JSON:       {0} (out of scope)" -f $counts.otherJson)
Write-Host ("  layouts:          {0} (unsupported)" -f $counts.layouts)
Write-Host ("  encodings:        {0} (unsupported)" -f $counts.encodings)
Write-Host ("  dictionaries:     {0} (recognized or unknown, not imported)" -f $counts.dictionaries)
Write-Host ("  pickle files:     {0} (not deserialized)" -f $counts.pickleFiles)
Write-Host ("  sentence history: {0} (not imported)" -f $counts.sentenceHistory)
Write-Host ("  imported:         {0}" -f $counts.imported)
Write-Host ("  unsupported:      {0}" -f $counts.unsupported)
if ($customWordCategories) {
    Write-Host ("  custom words:     {0} total (phone-like {1}, domain-like {2}, other {3}); no mapping, no word text shown" -f `
        ($customWordCategories.phoneLike + $customWordCategories.domainLike + $customWordCategories.other),
        $customWordCategories.phoneLike, $customWordCategories.domainLike, $customWordCategories.other)
}
if ($Apply) {
    if ($report.backup.created) {
        Write-Host ("  backup:           {0}" -f $report.backup.name)
    }
    else {
        Write-Host '  backup:           skipped (-Backup:$false)'
    }
    Write-Host ("  report:           {0}" -f $reportJsonPath)
    Write-Host ("  report:           {0}" -f $reportMarkdownPath)
}
else {
    Write-Host '  report:           not written (pass -Apply to write it under <data home>\import\)'
}
Write-Host ''
Write-Host 'No legacy file was modified. Pickle data was not deserialized. No word text was printed.' -ForegroundColor DarkGray
Write-Host ''

exit 0
