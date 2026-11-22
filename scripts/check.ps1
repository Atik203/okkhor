$ErrorActionPreference = "Stop"

$cargo = Join-Path $HOME ".cargo\bin\cargo.exe"
if (-not (Test-Path $cargo)) {
    throw "cargo not found at $cargo. Install Rust from https://rustup.rs and reopen the shell."
}

$mingw = "C:\msys64\mingw64\bin"
if (Test-Path $mingw) {
    $env:PATH = "$HOME\.cargo\bin;$mingw;$env:PATH"
} else {
    $env:PATH = "$HOME\.cargo\bin;$env:PATH"
}

Write-Host "==> cargo fmt" -ForegroundColor Cyan
& $cargo fmt --all -- --check

Write-Host "==> cargo clippy" -ForegroundColor Cyan
& $cargo clippy --workspace --all-targets -- -D warnings

Write-Host "==> cargo test" -ForegroundColor Cyan
& $cargo test --workspace

Write-Host "==> daemon self-check" -ForegroundColor Cyan
& $cargo run --quiet --package akkhor-daemon -- self-check

Write-Host "All checks passed." -ForegroundColor Green
