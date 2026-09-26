# Lap RAW-development gate runner (TASK-101 / lap-7f5.1).
#
# Runs the Lap baseline quality gates exactly as documented in
# docs/raw-development/baseline.md. Exit code is non-zero if any gate fails.
# Each gate's result is printed so pre-existing baseline failures (currently
# cargo fmt --check, see baseline.md) stay visible instead of being masked.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\raw_dev_gates.ps1

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)

# Canonical toolchain setup: rustup 1.98 MSVC ahead of the standalone GNU 1.85
# toolchain on the default PATH, plus the VS 2022 bundled cmake required by
# src-tauri/build.rs (libheif/libde265/libjpeg-turbo).
$RustupBin = Join-Path $env:USERPROFILE ".cargo\bin"
$CmakeBin = "C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"
if (-not (Test-Path (Join-Path $RustupBin "cargo.exe"))) {
    Write-Error "rustup cargo not found at $RustupBin"
}
if (-not (Test-Path (Join-Path $CmakeBin "cmake.exe"))) {
    Write-Error "cmake not found at $CmakeBin (Visual Studio 2022 bundle expected)"
}
$env:PATH = "$RustupBin;$CmakeBin;$env:PATH"

function Invoke-Gate {
    param([string]$Name, [scriptblock]$Command)
    Write-Host ""
    Write-Host "=== $Name ===" -ForegroundColor Cyan
    # Windows PowerShell 5.1 turns redirected native stderr into ErrorRecords;
    # they must not terminate the run under the script-level Stop preference.
    $ErrorActionPreference = "Continue"
    $output = & $Command 2>&1 | Select-Object -Last 12
    $code = $LASTEXITCODE
    $ErrorActionPreference = "Stop"
    $output | ForEach-Object { Write-Host "$_" }
    if ($code -eq 0) {
        Write-Host "PASS $Name" -ForegroundColor Green
    } else {
        Write-Host "FAIL $Name (exit $code)" -ForegroundColor Red
    }
    return $code
}

$failures = @()
$failures += Invoke-Gate "npm --prefix src-vite run build" { npm --prefix src-vite run build }
$failures += Invoke-Gate "npm --prefix src-vite run test" { npm --prefix src-vite run test }
$failures += Invoke-Gate "cargo test" { cargo test --manifest-path src-tauri/Cargo.toml }
$failures += Invoke-Gate "cargo fmt --check" { cargo fmt --manifest-path src-tauri/Cargo.toml -- --check }
$failures += Invoke-Gate "cargo clippy --all-targets" { cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets }

Write-Host ""
$failed = ($failures | Where-Object { $_ -ne 0 }).Count
if ($failed -gt 0) {
    Write-Host "$failed gate(s) failed (see docs/raw-development/baseline.md for known baseline failures)." -ForegroundColor Red
    exit 1
}
Write-Host "All Lap gates passed." -ForegroundColor Green
exit 0
