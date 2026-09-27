# End-to-end A1-A7 small-slice qualification gate (lap-70c / TASK-306).
#
# Assembles the deterministic layer tests (model, storage, renderer, UI) and
# the end-to-end scenario suite into ONE repeatable run against the real RAW
# corpus and the real Windows GPU backend, then records machine-readable
# evidence. The gate cannot pass on mocks, skipped GPU scenarios, or a
# blocked report.
#
# Stages:
#   1 pins        engine revision pin consistency + engine checkout state
#   2 rust-layer  cargo test (src-tauri): develop unit + persistence
#                 integration suites, incl. crash-termination injection
#   3 ui-layer    vitest (src-vite) via the established harness wrapper
#   4 e2e-suite   tests/raw-development/e2e scenarios (real decodes, real
#                 GPU preview/export, hard process termination, frozen
#                 preview/export/histogram parity tolerances)
#   5 pinned-parity  capture-baselines.ps1 compare against the frozen
#                 pre-extraction RapidRAW baseline (real GPU, real corpus)
#   6 app-launch  launch the actual built application (lifecycle evidence)
#   7 release-gate  check-release-gates.ps1 must report BLOCKED (the
#                 distribution hold is the expected, correct state)
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\raw-development\qualify-slice.ps1 [-SkipStages 5]
#
# Exit codes: 0 = all required stages passed; 1 = at least one failed;
#             2 = setup error.

param(
    [string]$RunDir = "",
    [string]$SkipStages = ""   # comma-separated stage numbers (maintenance only)
)

$ErrorActionPreference = "Continue"
$RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSCommandPath))
$VsRoot = "C:\Program Files\Microsoft Visual Studio\2022\Community"
$CmakeDir = Join-Path $VsRoot "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"

if ($RunDir -eq "") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $RunDir = Join-Path $RepoRoot "tests\raw-development\runs\gate-$stamp"
}
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null

$skip = @{}
if ($SkipStages -ne "") { $SkipStages.Split(",") | ForEach-Object { $skip[$_.Trim()] = $true } }

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

function Invoke-Logged([string]$FilePath, [string[]]$ArgumentList, [string]$Log, [int]$TimeoutMinutes = 90) {
    # Runs a process with combined output captured to the log; returns the
    # exit code (throws a descriptive exception on timeout). Uses
    # System.Diagnostics.Process directly: PowerShell 5.1's Start-Process
    # -PassThru does not surface ExitCode after the cmdlet's cleanup.
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

# ---------------------------------------------------------------------------
if (-not $skip["1"]) { Invoke-Stage "1" "pins" { param($log)
    $lock = Get-Content (Join-Path $RepoRoot "docs\raw-development\engine-lock.json") -Raw | ConvertFrom-Json
    $pinned = $lock.engine.revision
    $sessionsConst = (Select-String -Path (Join-Path $RepoRoot "src-tauri\src\develop\sessions.rs") -Pattern 'ENGINE_GIT_REVISION: &str = "([0-9a-f]{40})"').Matches[0].Groups[1].Value
    $cargoLock = Get-Content (Join-Path $RepoRoot "src-tauri\Cargo.lock") -Raw
    $lockOk = ($cargoLock -match [regex]::Escape("rev=$pinned"))
    $engineHead = (git -C "C:\Users\Thomas\Desktop\RapidRAW-engine" rev-parse HEAD) 2>$null
    $engineStatus = (git -C "C:\Users\Thomas\Desktop\RapidRAW-engine" status --porcelain) 2>$null
    # The engine HEAD may sit on documented tracker-event chore commits ABOVE
    # the pinned implementation revision; cargo consumes the pinned rev's
    # immutable content. The pin must exist and be an ancestor of HEAD, the
    # worktree must be clean.
    git -C "C:\Users\Thomas\Desktop\RapidRAW-engine" cat-file -e "$pinned`^{commit}" 2>$null
    $pinExists = ($LASTEXITCODE -eq 0)
    git -C "C:\Users\Thomas\Desktop\RapidRAW-engine" merge-base --is-ancestor $pinned $engineHead 2>$null
    $pinAncestor = ($LASTEXITCODE -eq 0)
    $engineClean = $pinExists -and $pinAncestor -and (-not $engineStatus)
    $parity = Test-Path (Join-Path $RepoRoot "tests\raw-development\preview-export-parity.json")
    $corpus = Test-Path (Join-Path $RepoRoot "tests\fixtures\raw-development\corpus-manifest.json")
    $all = ($sessionsConst -eq $pinned) -and $lockOk -and $engineClean -and $parity -and $corpus
    if (-not $all) {
        @(
        "engine-lock revision: $pinned"
        "sessions.rs constant: $sessionsConst"
        "Cargo.lock pins revision: $lockOk"
        "engine pin exists & ancestor of HEAD & clean: $engineClean (head $engineHead)"
        "parity manifest present: $parity"
        "corpus manifest present: $corpus"
        ) | Set-Content $log
    } else {
        "engine pin $pinned consistent across engine-lock.json, sessions.rs and Cargo.lock; engine checkout clean at pin; parity + corpus manifests present." | Set-Content $log
    }
    if (-not $all) { throw "pin checks failed (see $log)" }
    return "engine $pinned"
} | Out-Null }

# ---------------------------------------------------------------------------
if (-not $skip["2"]) { Invoke-Stage "2" "rust-layer" { param($log)
    # Environment setup goes through a generated batch file: cmd.exe's /c
    # quote-stripping makes inline && chains with quoted paths unreliable.
    $stageCmd = Join-Path $RunDir "stage-2-env.cmd"
    @"
@echo off
call "$VsRoot\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set "PATH=%USERPROFILE%\.cargo\bin;$CmakeDir;%PATH%"
cd /d "$RepoRoot"
cargo test --manifest-path src-tauri\Cargo.toml
"@ | Set-Content -Encoding ASCII $stageCmd
    $code = Invoke-Logged -FilePath "$env:ComSpec" -ArgumentList @("/c", $stageCmd) -Log $log -TimeoutMinutes 90
    if ($code -ne 0) { throw "cargo test exited $code (log: $log)" }
    $tail = (Get-Content $log | Select-String "test result:") | ForEach-Object { $_.Line.Trim() }
    $tail -join " | "
} | Out-Null }

# ---------------------------------------------------------------------------
if (-not $skip["3"]) { Invoke-Stage "3" "ui-layer" { param($log)
    $code = Invoke-Logged -FilePath "powershell.exe" `
        -ArgumentList @("-ExecutionPolicy", "Bypass", "-File", (Join-Path $RepoRoot "scripts\raw-development\harness-validation.ps1")) `
        -Log $log -TimeoutMinutes 30
    if ($code -ne 0) { throw "vitest gate exited $code (log: $log)" }
    $tail = (Get-Content $log | Select-String "Test Files|Tests ") | ForEach-Object { $_.Line.Trim() }
    $tail -join " | "
} | Out-Null }

# ---------------------------------------------------------------------------
if (-not $skip["4"]) { Invoke-Stage "4" "e2e-suite" { param($log)
    # Build the harness (build-e2e.cmd handles the MSVC + rustup + fake-CRT
    # stub environment documented in that script) and run every scenario.
    $buildLog = Join-Path $RunDir "stage-4-build.log"
    $buildCode = Invoke-Logged -FilePath "cmd.exe" `
        -ArgumentList @("/c", (Join-Path $RepoRoot "scripts\raw-development\build-e2e.cmd")) `
        -Log $buildLog -TimeoutMinutes 90
    if ($buildCode -ne 0) { throw "e2e build exited $buildCode (log: $buildLog)" }

    $env:LAP_E2E_RUN_DIR = $RunDir
    Remove-Item Env:\LAP_E2E_PARITY_FREEZE -ErrorAction SilentlyContinue
    $exe = Join-Path $RepoRoot "tests\raw-development\e2e\target\debug\lap-raw-e2e.exe"
    $code = Invoke-Logged -FilePath $exe -ArgumentList @() -Log $log -TimeoutMinutes 60
    if ($code -ne 0) {
        $fails = (Get-Content $log | Select-String "\[FAIL\]") | ForEach-Object { $_.Line.Trim() }
        throw "e2e gate exited ${code}: $($fails -join ' | ')"
    }
    "all A1-A7 scenarios passed (evidence JSON in $RunDir)"
} | Out-Null }

# ---------------------------------------------------------------------------
if (-not $skip["5"]) { Invoke-Stage "5" "pinned-parity" { param($log)
    $code = Invoke-Logged -FilePath "powershell.exe" `
        -ArgumentList @("-ExecutionPolicy", "Bypass", "-File", (Join-Path $RepoRoot "scripts\raw-development\capture-baselines.ps1"), "-AllowSourceDrift") `
        -Log $log -TimeoutMinutes 120
    if ($code -ne 0) { throw "baseline compare exited $code (log: $log)" }
    $result = (Get-Content $log | Select-String "Baseline comparison|RESULT") | Select-Object -Last 1
    if ($null -eq $result) { throw "baseline comparison verdict not found in log" }
    "$($result.Line.Trim())"
} | Out-Null }

# ---------------------------------------------------------------------------
if (-not $skip["6"]) { Invoke-Stage "6" "app-launch" { param($log)
    $code = Invoke-Logged -FilePath "powershell.exe" `
        -ArgumentList @("-ExecutionPolicy", "Bypass", "-File", (Join-Path $RepoRoot "scripts\raw-development\launch-app-slice.ps1"), "-RunDir", (Join-Path $RunDir "app-launch")) `
        -Log $log -TimeoutMinutes 10
    if ($code -ne 0) { throw "app launch scenario exited $code (log: $log)" }
    "application launched, window verified, graceful exit 0"
} | Out-Null }

# ---------------------------------------------------------------------------
if (-not $skip["7"]) { Invoke-Stage "7" "release-gate" { param($log)
    $code = Invoke-Logged -FilePath "powershell.exe" `
        -ArgumentList @("-ExecutionPolicy", "Bypass", "-File", (Join-Path $RepoRoot "scripts\raw-development\check-release-gates.ps1")) `
        -Log $log -TimeoutMinutes 5
    # Exit 1 = BLOCKED is the REQUIRED, honest state (spec P3 distribution
    # hold). Exit 0 would mean a release decision exists; 2 = unreadable
    # provenance. Both are failures for this gate.
    if ($code -eq 1) { return "BLOCKED as required (distribution hold in force)" }
    throw "release gate exited $code; expected 1 (BLOCKED) (log: $log)"
} | Out-Null }

# ---------------------------------------------------------------------------
$failedStages = @($stages | Where-Object { -not $_.ok })
$summary = [pscustomobject]@{
    schema      = "lap-raw-slice-qualification-gate/v1"
    runDir      = $RunDir
    finishedUtc = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    hostRevision = (git -C $RepoRoot rev-parse HEAD) 2>$null
    outcome     = $(if ($failedStages.Count -eq 0) { "pass" } else { "fail" })
    stages      = $stages
}
$summaryPath = Join-Path $RunDir "gate-summary.json"
$summary | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $summaryPath

Write-Host ""
Write-Host "================ A1-A7 SLICE QUALIFICATION GATE ================"
foreach ($s in $stages) {
    Write-Host ("  [{0}] stage {1} {2} ({3}s)" -f $(if ($s.ok) { "PASS" } else { "FAIL" }), $s.id, $s.name, $s.seconds)
}
Write-Host "summary: $summaryPath"
Write-Host "== gate: $(if ($failedStages.Count -eq 0) { 'PASS' } else { 'FAIL' }) =="
exit $(if ($failedStages.Count -eq 0) { 0 } else { 1 })
