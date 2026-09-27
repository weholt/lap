# Fail-closed release gate wrapper (lap-d7f / TASK-103).
#
# Runs scripts/raw-development/check-release-gates.mjs with node and propagates
# its exit code:
#   0 = released (all gates passed including a recorded, evidenced
#       combined-product distribution decision)
#   1 = blocked (EXPECTED while the distribution hold stands: missing
#       approval/evidence, incomplete provenance, or unresolved unknowns)
#   2 = failed (provenance inventory unreadable/malformed)
#
# A non-zero exit is the correct result today and must not be worked around;
# see docs/raw-development/release-gates.md for how an authorized decision is
# recorded. No environment signal can influence the verdict.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\raw-development\check-release-gates.ps1

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSCommandPath))
Set-Location -LiteralPath $RepoRoot

$nodePath = $null
$roots = @(
    (Join-Path $env:ProgramFiles "nodejs"),
    (Join-Path ${env:ProgramFiles(x86)} "nodejs"),
    (Join-Path $env:LOCALAPPDATA "Programs\nodejs")
)
$found = $roots | ForEach-Object { Join-Path $_ "node.exe" } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if ($found) {
    $nodePath = $found
} else {
    $command = Get-Command node.exe -ErrorAction SilentlyContinue
    if ($command) { $nodePath = $command.Source }
}
if (-not $nodePath) {
    Write-Error "node.exe not found on PATH or in well-known install roots; cannot run the release gate."
    exit 127
}

$ErrorActionPreference = "Continue"
& $nodePath (Join-Path $PSScriptRoot "check-release-gates.mjs") @args
$code = $LASTEXITCODE
if ($null -eq $code) { $code = 1 }
if ($code -lt 0) { $code = 1 }
Write-Host "check-release-gates: exited with $code (0=released, 1=blocked, 2=failed)"
exit $code
