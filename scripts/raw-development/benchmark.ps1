# Platform qualification and performance harness driver (lap-c0e / TASK-601).
#
# Builds the lap-raw-platform harness with the correct MSVC + rustup
# environment, runs its unit tests, executes the full benchmark/stress
# suite on the real GPU, then validates the results document and runs the
# fail-closed platform release gate.
#
# Stages:
#   1 unit-tests   cargo test (quantile math + harness invariants)
#   2 build        cargo build of the harness (fake-CRT fix applied, same
#                  hazard documented in build-e2e.cmd)
#   3 run          lap-raw-platform.exe full -> results document (raw
#                  measurements + computed p95, RAM/VRAM counters)
#   4 validate     node benchmark.mjs validate <results>   (exit 0 required)
#   5 gate         node benchmark.mjs gate                  (exit 1 = BLOCKED
#                  is the REQUIRED honest state while Linux/macOS reference
#                  machines are absent; exit 0 or 2 fails this stage)
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\raw-development\benchmark.ps1 [-RunDir <dir>]
#
# Exit codes: 0 = all stages passed (gate BLOCKED as required);
#             1 = a stage failed; 2 = setup error.

param(
    [string]$RunDir = ""
)

$ErrorActionPreference = "Continue"
$RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSCommandPath))
$PlatformDir = Join-Path $RepoRoot "tests\raw-development\platform"
$VsRoot = "C:\Program Files\Microsoft Visual Studio\2022\Community"
$CmakeDir = Join-Path $VsRoot "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"
$ResultsPath = Join-Path $PlatformDir "runs\platform-results-windows-thomas-rtx4060.json"

if ($RunDir -eq "") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $RunDir = Join-Path $RepoRoot "tests\raw-development\runs\platform-$stamp"
}
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null

$stages = New-Object System.Collections.Generic.List[object]

function Invoke-Stage([string]$Id, [string]$Name, [scriptblock]$Body) {
    Write-Host ""
    Write-Host "== stage $Id : $Name =="
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $log = Join-Path $RunDir "stage-$Id.log"
    $failed = $false
    $detail = ""
    try {
        $detail = & $Body $log
    } catch {
        $failed = $true
        $detail = "exception: $($_.Exception.Message)"
    }
    $sw.Stop()
    $ok = -not $failed
    Write-Host "== stage $Id : $(if ($ok) { 'PASS' } else { 'FAIL' }) ($([int]$sw.Elapsed.TotalSeconds)s) $detail =="
    $stages.Add([pscustomobject]@{
        id = $Id; name = $Name; ok = $ok; detail = $detail
        seconds = [int]$sw.Elapsed.TotalSeconds; log = $log
    })
    return $ok
}

function Invoke-Logged([string]$FilePath, [string[]]$ArgumentList, [string]$Log, [int]$TimeoutMinutes = 120) {
    # Runs a process with combined output captured to the log; returns the
    # exit code (throws a descriptive exception on timeout).
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    $psi.Arguments = ($ArgumentList | ForEach-Object {
        if ($_ -match '[ "]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
    }) -join ' '
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.WorkingDirectory = $RepoRoot
    $p = [System.Diagnostics.Process]::Start($psi)
    $outTask = $p.StandardOutput.ReadToEndAsync()
    $errTask = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit($TimeoutMinutes * 60 * 1000)) {
        try { & taskkill /F /T /PID $p.Id 2>&1 | Out-Null } catch {}
        throw "timed out after $TimeoutMinutes minutes (log: $Log)"
    }
    ($outTask.Result.TrimEnd() + "`r`n" + $errTask.Result.TrimEnd()) | Set-Content -Encoding UTF8 $Log
    $code = $p.ExitCode
    $p.Dispose()
    return $code
}

# Environment setup through a generated batch file (cmd.exe /c quote
# handling; same approach as qualify-slice.ps1 stage 2).
$envCmd = Join-Path $RunDir "env-cargo.cmd"
@"
@echo off
call "$VsRoot\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set "PATH=%USERPROFILE%\.cargo\bin;$CmakeDir;%PATH%"
cd /d "$RepoRoot"
%*
"@ | Set-Content -Encoding ASCII $envCmd

# ---------------------------------------------------------------------------
if (-not (Test-Path (Join-Path $PlatformDir "Cargo.toml"))) {
    Write-Error "platform harness not found at $PlatformDir"
    exit 2
}

function Update-FakeCrt {
    # Fake-CRT fix (same hazard as build-e2e.cmd): tauri-build's
    # static_vcruntime step writes an (almost) empty fake msvcrt.lib into the
    # Lap build-script OUT_DIR, which breaks the final link of every binary
    # whose dependency graph includes Lap (LNK1120: unresolved
    # memcpy/atexit/...). Replace any sub-4KB fake archive with a copy of the
    # real VC msvcrt.lib; the build script only recreates it when missing, so
    # the real copy persists. Idempotent; returns the number of replacements.
    $realMsvcrt = Get-ChildItem -LiteralPath (Join-Path $VsRoot "VC\Tools\MSVC") -Recurse -Filter msvcrt.lib -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like "*\lib\x64\msvcrt.lib" } |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $realMsvcrt) { throw "real msvcrt.lib not found under $($VsRoot)\VC\Tools\MSVC" }
    $fixed = 0
    foreach ($profile in @("debug", "release")) {
        $buildDir = Join-Path $PlatformDir "target\$profile\build"
        if (-not (Test-Path $buildDir)) { continue }
        Get-ChildItem -LiteralPath $buildDir -Directory -Filter "Lap-*" -ErrorAction SilentlyContinue | ForEach-Object {
            $fake = Join-Path $_.FullName "out\msvcrt.lib"
            if ((Test-Path -LiteralPath $fake) -and ((Get-Item -LiteralPath $fake).Length -lt 4096)) {
                Copy-Item -LiteralPath $realMsvcrt.FullName -Destination $fake -Force
                $fixed++
            }
        }
    }
    return $fixed
}

Invoke-Stage "1" "unit-tests" { param($log)
    Update-FakeCrt | Out-Null
    $code = Invoke-Logged -FilePath "$env:ComSpec" `
        -ArgumentList @("/c", $envCmd, "cargo", "test", "--manifest-path", "tests\raw-development\platform\Cargo.toml") `
        -Log $log -TimeoutMinutes 120
    if ($code -ne 0) { throw "cargo test exited $code (log: $log)" }
    $tail = (Get-Content $log | Select-String "test result:") | ForEach-Object { $_.Line.Trim() }
    $tail -join " | "
} | Out-Null

# ---------------------------------------------------------------------------
Invoke-Stage "2" "build" { param($log)
    $code = Invoke-Logged -FilePath "$env:ComSpec" `
        -ArgumentList @("/c", $envCmd, "cargo", "build", "--manifest-path", "tests\raw-development\platform\Cargo.toml") `
        -Log $log -TimeoutMinutes 120
    if ($code -ne 0) { throw "cargo build exited $code (log: $log)" }

    # Re-link after the CRT fix if any stub was replaced.
    $fixed = Update-FakeCrt
    if ($fixed -gt 0) {
        Write-Host "replaced $fixed fake CRT stub(s); re-linking"
        $code = Invoke-Logged -FilePath "$env:ComSpec" `
            -ArgumentList @("/c", $envCmd, "cargo", "build", "--manifest-path", "tests\raw-development\platform\Cargo.toml") `
            -Log $log -TimeoutMinutes 120
        if ($code -ne 0) { throw "re-link exited $code (log: $log)" }
    }
    $exe = Join-Path $PlatformDir "target\debug\lap-raw-platform.exe"
    if (-not (Test-Path $exe)) { throw "harness binary not produced: $exe" }
    "built $exe"
} | Out-Null

# ---------------------------------------------------------------------------
Invoke-Stage "3" "run" { param($log)
    $exe = Join-Path $PlatformDir "target\debug\lap-raw-platform.exe"
    $code = Invoke-Logged -FilePath $exe -ArgumentList @("full") -Log $log -TimeoutMinutes 60
    if ($code -ne 0) { throw "platform harness exited $code (log: $log)" }
    if (-not (Test-Path $ResultsPath)) { throw "results document not written: $ResultsPath" }
    $results = Get-Content $ResultsPath -Raw | ConvertFrom-Json
    $warm = $results.workloads.'warm-slider-1536'
    "machine $($results.machineId), warm p95 $($warm.p95Ms) ms (target met: $($warm.target.met))"
} | Out-Null

# ---------------------------------------------------------------------------
Invoke-Stage "4" "validate" { param($log)
    $code = Invoke-Logged -FilePath "node" `
        -ArgumentList @((Join-Path $RepoRoot "scripts\raw-development\benchmark.mjs"), "validate", $ResultsPath) `
        -Log $log -TimeoutMinutes 5
    if ($code -ne 0) { throw "results validation exited $code (log: $log)" }
    "results schema valid"
} | Out-Null

# ---------------------------------------------------------------------------
Invoke-Stage "5" "gate" { param($log)
    $code = Invoke-Logged -FilePath "node" `
        -ArgumentList @((Join-Path $RepoRoot "scripts\raw-development\benchmark.mjs"), "gate") `
        -Log $log -TimeoutMinutes 5
    # Exit 1 = BLOCKED is the REQUIRED honest state: Linux/macOS reference
    # machines are explicitly unavailable (spec P5). Exit 0 would mean the
    # full platform matrix passed (impossible here); 2 = malformed evidence.
    if ($code -eq 1) { return "BLOCKED as required (Linux/macOS reference machines explicitly unavailable)" }
    throw "platform gate exited $code; expected 1 (BLOCKED) (log: $log)"
} | Out-Null

# ---------------------------------------------------------------------------
$failedStages = @($stages | Where-Object { -not $_.ok })
$summary = [pscustomobject]@{
    schema      = "lap-raw-platform-run/v1"
    runDir      = $RunDir
    results     = $ResultsPath
    finishedUtc = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    hostRevision = (git -C $RepoRoot rev-parse HEAD) 2>$null
    outcome     = $(if ($failedStages.Count -eq 0) { "pass" } else { "fail" })
    stages      = $stages
}
$summaryPath = Join-Path $RunDir "platform-run-summary.json"
$summary | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $summaryPath

Write-Host ""
Write-Host "================ PLATFORM QUALIFICATION RUN ================"
foreach ($s in $stages) {
    Write-Host ("  [{0}] stage {1} {2} ({3}s)" -f $(if ($s.ok) { "PASS" } else { "FAIL" }), $s.id, $s.name, $s.seconds)
}
Write-Host "results: $ResultsPath"
Write-Host "summary: $summaryPath"
Write-Host "== run: $(if ($failedStages.Count -eq 0) { 'PASS' } else { 'FAIL' }) =="
exit $(if ($failedStages.Count -eq 0) { 0 } else { 1 })
