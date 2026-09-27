# Harness validation wrapper (lap-d13 / TASK-102).
#
# pbh executes .agents/pebbles-harness/config.yml validation_commands as a raw
# argv list without shell or PATHEXT resolution, so a bare `npm` token cannot
# be spawned on Windows (npm is npm.cmd; attempts 1-2 of this issue failed with
# exit 127 "executable not found: npm"). This wrapper resolves npm.cmd
# explicitly (PATH first, then well-known install roots), runs the focused Lap
# vitest gate unchanged and propagates its exit code. Nothing is regenerated
# and no baseline manifest is touched here.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\raw-development\harness-validation.ps1

$ErrorActionPreference = "Stop"
# Script lives in <repo>\scripts\raw-development\; three parent hops reach the repo root.
$RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSCommandPath))
Set-Location -LiteralPath $RepoRoot

$npmPath = $null
$roots = @(
    (Join-Path $env:ProgramFiles "nodejs"),
    (Join-Path ${env:ProgramFiles(x86)} "nodejs"),
    (Join-Path $env:LOCALAPPDATA "Programs\nodejs")
)
$found = $roots | ForEach-Object { Join-Path $_ "npm.cmd" } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if ($found) {
    $npmPath = $found
} else {
    $command = Get-Command npm.cmd -ErrorAction SilentlyContinue
    if ($command) { $npmPath = $command.Source }
}
if (-not $npmPath) {
    Write-Error "npm.cmd not found on PATH, in %ProgramFiles%\nodejs or %LOCALAPPDATA%\Programs\nodejs; cannot run the Lap vitest gate."
    exit 127
}

Write-Host "harness-validation: npm resolved to $npmPath"
# Windows PowerShell 5.1 surfaces redirected native stderr as ErrorRecords;
# keep the script alive long enough to propagate the real exit code.
$ErrorActionPreference = "Continue"
& $npmPath --prefix src-vite run test
$code = $LASTEXITCODE
if ($code -lt 0) { $code = 1 }
Write-Host "harness-validation: npm --prefix src-vite run test exited with $code"
exit $code
