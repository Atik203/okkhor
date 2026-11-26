<#
.SYNOPSIS
  Rebuild the embedded UI feedback sounds.

.DESCRIPTION
  The toggle click ships inside the binary (`include_bytes!`), so the portable
  artifact needs no sidecar file. This script rewrites the PCM samples in
  place: normalize to a target peak and fade the edges to avoid a click-pop.

  The 0.9.6 click peaked at 45 % of full scale (RMS -19.7 dBFS), which several
  owners reported as too quiet next to system sounds. 0.92 peak is +6.3 dB with
  no clipping; the waveform and length (18 ms) are unchanged.

.EXAMPLE
  .\scripts\make-sounds.ps1
#>
[CmdletBinding()]
param(
  [string]$Path = (Join-Path (Split-Path $PSScriptRoot -Parent) 'assets\sounds\toggle-click.wav'),
  [double]$Peak = 0.92,
  [double]$FadeMs = 0.0
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Path)) { throw "Missing sound: $Path" }
if ($Peak -le 0 -or $Peak -gt 1) { throw '-Peak must be in (0, 1].' }
if ($FadeMs -lt 0) { throw '-FadeMs must be >= 0.' }

$bytes = [System.IO.File]::ReadAllBytes($Path)
if ([Text.Encoding]::ASCII.GetString($bytes, 0, 4) -ne 'RIFF' -or
    [Text.Encoding]::ASCII.GetString($bytes, 8, 4) -ne 'WAVE') {
  throw "Not a RIFF/WAVE file: $Path"
}

# Locate fmt and data chunks without assuming a 44-byte canonical header.
$offset = 12
$fmt = $null
$dataAt = -1
$dataLen = 0
while ($offset + 8 -le $bytes.Length) {
  $id = [Text.Encoding]::ASCII.GetString($bytes, $offset, 4)
  $size = [BitConverter]::ToUInt32($bytes, $offset + 4)
  if ($id -eq 'fmt ') {
    $fmt = [pscustomobject]@{
      FormatTag = [BitConverter]::ToUInt16($bytes, $offset + 8)
      Channels  = [BitConverter]::ToUInt16($bytes, $offset + 10)
      Rate      = [BitConverter]::ToUInt32($bytes, $offset + 12)
      Bits      = [BitConverter]::ToUInt16($bytes, $offset + 22)
    }
  } elseif ($id -eq 'data') {
    $dataAt = $offset + 8
    $dataLen = [int]$size
    break
  }
  $offset += 8 + [int]$size + ([int]$size % 2)
}

if ($null -eq $fmt -or $dataAt -lt 0) { throw 'Missing fmt or data chunk.' }
if ($fmt.FormatTag -ne 1 -or $fmt.Bits -ne 16) {
  throw "Only 16-bit PCM is supported (tag $($fmt.FormatTag), $($fmt.Bits)-bit)."
}

$samples = $dataLen / 2
$values = [int16[]]::new($samples)
[Buffer]::BlockCopy($bytes, $dataAt, $values, 0, $dataLen)

$currentPeak = 0
foreach ($value in $values) {
  $magnitude = [Math]::Abs([int]$value)
  if ($magnitude -gt $currentPeak) { $currentPeak = $magnitude }
}
if ($currentPeak -eq 0) { throw 'The sound is silent.' }

$gain = ($Peak * 32767.0) / $currentPeak
$fadeSamples = [int][Math]::Round(($FadeMs / 1000.0) * $fmt.Rate * $fmt.Channels)
$fadeSamples = [Math]::Min($fadeSamples, [int]($samples / 2))

$peakAfter = 0
for ($i = 0; $i -lt $samples; $i++) {
  $scaled = $values[$i] * $gain
  # Soft-knee the last few percent so a transient cannot clip.
  if ($scaled -gt 32767.0 -or $scaled -lt -32767.0) {
    $scaled = 32767.0 * [Math]::Tanh($scaled / 32767.0)
  }
  if ($fadeSamples -gt 0) {
    if ($i -lt $fadeSamples) {
      $scaled *= $i / $fadeSamples
    } elseif ($i -ge ($samples - $fadeSamples)) {
      $scaled *= ($samples - 1 - $i) / $fadeSamples
    }
  }
  $rounded = [int][Math]::Round($scaled)
  $rounded = [Math]::Max(-32768, [Math]::Min(32767, $rounded))
  $values[$i] = [int16]$rounded
  if ([Math]::Abs($rounded) -gt $peakAfter) { $peakAfter = [Math]::Abs($rounded) }
}

$out = [byte[]]::new($bytes.Length)
[Buffer]::BlockCopy($bytes, 0, $out, 0, $bytes.Length)
[Buffer]::BlockCopy($values, 0, $out, $dataAt, $dataLen)
[System.IO.File]::WriteAllBytes($Path, $out)

$beforeDb = 20 * [Math]::Log10([Math]::Min(1.0, ($currentPeak / 32767.0)))
$afterDb = 20 * [Math]::Log10([Math]::Min(1.0, ($peakAfter / 32767.0)))
"toggle-click.wav: peak {0:N0} -> {1:N0} ({2:N2} dBFS -> {3:N2} dBFS), gain x{4:N2}" -f `
  $currentPeak, $peakAfter, $beforeDb, $afterDb, $gain
