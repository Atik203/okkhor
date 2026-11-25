# Run the keyboard locally for a typing-feel pass.
#
# The daemon alone is enough to feel the ghost hint: the popup is a native
# window, not part of the Tauri UI. Build the release daemon, point it at the
# normal data directory, and type in Notepad/Word/Chrome.
#
# Optional model: place `onet-bn-i8.oknm` / `onet-en-i8.oknm` under
# %LOCALAPPDATA%\Okkhor\data\models\. Without them the ghost uses the built-in
# bigram prior; with them it uses the transformer and the confidence gate.

$ErrorActionPreference = 'Stop'
$env:PATH = "$HOME\.cargo\bin;C:\msys64\mingw64\bin;$env:PATH"

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $models = Join-Path $env:LOCALAPPDATA 'Okkhor\data\models'
    foreach ($name in @('onet-bn-i8.oknm', 'onet-en-i8.oknm')) {
        $path = Join-Path $models $name
        if (Test-Path $path) {
            $size = [math]::Round((Get-Item $path).Length / 1MB, 1)
            Write-Host "model: $name ($size MB)"
        } else {
            Write-Host "model: $name missing - bigram prior only"
        }
    }
    Write-Host 'starting daemon (Ctrl+C to stop)'
    cargo run --release -p okkhor-daemon -- daemon
}
finally {
    Pop-Location
}
