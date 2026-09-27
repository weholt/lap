# Launches the ACTUAL Lap application for the small-slice qualification
# scenario (lap-70c / TASK-306) and records runtime evidence.
#
# What this proves (and honestly does not):
#   - The real built application binary starts, initializes its window and
#     its subsystems (database + develop-recipe startup reconciliation run
#     inside create_db), stays alive, and exits cleanly on close.
#   - It does NOT drive interactive GUI editing; unattended slider
#     interaction is out of scope here (spec A10 requires interactive
#     verification and is explicitly not claimed). The develop pipeline
#     itself is exercised end-to-end by tests/raw-development/e2e.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\raw-development\launch-app-slice.ps1 [-RunDir <dir>]
#
# Exit codes: 0 = launch scenario passed; 1 = failed; 2 = setup error.

param(
    [string]$RunDir = ""
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSCommandPath))
$ExePath = Join-Path $RepoRoot "src-tauri\target\debug\Lap.exe"

if (-not (Test-Path -LiteralPath $ExePath)) {
    Write-Error "Lap.exe not found at $ExePath; build it first (cargo build --manifest-path src-tauri/Cargo.toml)."
    exit 2
}

if ($RunDir -eq "") {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $RunDir = Join-Path $RepoRoot "tests\raw-development\runs\app-launch-$stamp"
}
New-Item -ItemType Directory -Force -Path $RunDir | Out-Null

$outLog = Join-Path $RunDir "lap-stdout.log"
$errLog = Join-Path $RunDir "lap-stderr.log"
$evidencePath = Join-Path $RunDir "app-launch-evidence.json"

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class WinEnum {
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lp);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    delegate bool EnumWindowsProc(IntPtr h, IntPtr lp);
    public static List<string> VisibleWindowsOf(uint targetPid) {
        var result = new List<string>();
        EnumWindows((h, lp) => {
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (pid == targetPid && IsWindowVisible(h)) {
                var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
                result.Add(sb.ToString());
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
"@

function Get-Sha256([string]$File) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($File)
        try { return ([System.BitConverter]::ToString($sha.ComputeHash($stream))).Replace("-", "").ToLower() }
        finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
}

$steps = New-Object System.Collections.Generic.List[object]
function Add-Step([string]$Name, [bool]$Ok, [string]$Detail) {
    Write-Host "  [$(if ($Ok) {'PASS'} else {'FAIL'})] $Name - $Detail"
    $steps.Add([pscustomobject]@{ name = $Name; ok = $Ok; detail = $Detail })
}

$binarySha = Get-Sha256 $ExePath
$hostRevision = (git -C $RepoRoot rev-parse HEAD) 2>$null
if ($LASTEXITCODE -ne 0) { $hostRevision = "" }

Write-Host "== app-launch: starting =="
Add-Step "binary-present" $true "Lap.exe ($('{0:N0}' -f ((Get-Item $ExePath).Length)) bytes, sha256 $binarySha)"

# Launch the real application with console logs captured (debug build keeps
# the console subsystem, so startup eprintln/println lines are observable).
# System.Diagnostics.Process directly: PowerShell 5.1's Start-Process does
# not surface ExitCode after cmdlet cleanup.
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $ExePath
$psi.WorkingDirectory = (Split-Path -Parent $ExePath)
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$proc = [System.Diagnostics.Process]::Start($psi)
$outTask = $proc.StandardOutput.ReadToEndAsync()
$errTask = $proc.StandardError.ReadToEndAsync()
Add-Step "process-started" ($null -ne $proc -and -not $proc.HasExited) "PID $($proc.Id)"

# Poll for a visible top-level window of the app process (the Tauri main
# window), up to 90 seconds.
$windowTitle = $null
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline) {
    if ($proc.HasExited) { break }
    $windows = [WinEnum]::VisibleWindowsOf([uint32]$proc.Id)
    $windows = @($windows | Where-Object { $_ -and $_.Trim().Length -gt 0 })
    if ($windows.Count -gt 0) { $windowTitle = $windows[0]; break }
    Start-Sleep -Seconds 1
}
Add-Step "window-visible" ($null -ne $windowTitle) "main window title: '$windowTitle'"

# Stability hold: the process must stay alive and keep its window for 15s.
$stable = $false
if ($null -ne $windowTitle -and -not $proc.HasExited) {
    Start-Sleep -Seconds 15
    $windowsAfter = @([WinEnum]::VisibleWindowsOf([uint32]$proc.Id) | Where-Object { $_ -and $_.Trim().Length -gt 0 })
    $stable = (-not $proc.HasExited) -and $windowsAfter.Count -gt 0
    $ws = if ($proc.HasExited) { 0 } else { [int64]($proc.WorkingSet64 / 1MB) }
    Add-Step "startup-stable" $stable "alive after 15s hold, working set ~$ws MB, visible windows: $($windowsAfter.Count)"
} else {
    Add-Step "startup-stable" $false "skipped: no window or process already exited"
}

$closeOutcome = "not-attempted"
$exitCode = $null
if (-not $proc.HasExited) {
    # Graceful close: taskkill without /F posts WM_CLOSE; the app's
    # CloseRequested handler exits the process with code 0.
    $null = & taskkill /PID $proc.Id 2>&1
    $graceful = $proc.WaitForExit(45000)
    if ($graceful) {
        $exitCode = $proc.ExitCode
        $closeOutcome = "graceful-wm-close"
    } else {
        $closeOutcome = "forced-kill-after-timeout"
        $null = & taskkill /F /PID $proc.Id 2>&1
        $null = $proc.WaitForExit(15000)
        if (-not $proc.HasExited) { $closeOutcome = "unresponsive" }
    }
} else {
    $closeOutcome = "exited-before-close"
    $exitCode = $proc.ExitCode
}
$closeOk = ($closeOutcome -eq "graceful-wm-close" -and $exitCode -eq 0)
Add-Step "graceful-exit" $closeOk "close: $closeOutcome, exit code: $exitCode"

# Flush the async log readers now that the process exited.
try {
    [System.IO.File]::WriteAllText($outLog, $outTask.Result)
    [System.IO.File]::WriteAllText($errLog, $errTask.Result)
} catch {
    Add-Step "log-capture" $false "async log flush failed: $($_.Exception.Message)"
}

$stdoutLines = @(Get-Content $outLog -ErrorAction SilentlyContinue)
$stderrLines = @(Get-Content $errLog -ErrorAction SilentlyContinue)
$aiLine = @($stdoutLines + $stderrLines) | Where-Object { $_ -match "AI Engine|database|reconciliation" } | Select-Object -First 4

$allOk = ($steps | Where-Object { -not $_.ok } | Measure-Object).Count -eq 0

$evidence = [pscustomobject]@{
    schema        = "lap-raw-app-launch/v1"
    startedUtc    = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    outcome       = $(if ($allOk) { "pass" } else { "fail" })
    host          = [pscustomobject]@{ repository = $RepoRoot; revision = $hostRevision }
    binary        = [pscustomobject]@{ path = $ExePath; sha256 = $binarySha }
    process       = [pscustomobject]@{ pid = $proc.Id; exitCode = $exitCode; close = $closeOutcome }
    windowTitle   = $windowTitle
    startupLogSample = $aiLine
    stdoutLog     = $outLog
    stderrLog     = $errLog
    steps         = $steps
    honestLimits  = "Unattended launch proves process/window/lifecycle only; interactive editing verification (spec A10) is not claimed. The develop pipeline itself is proven by the e2e scenarios."
}
$evidence | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $evidencePath
Write-Host "  evidence: $evidencePath"
Write-Host "== app-launch: $(if ($allOk) { 'PASS' } else { 'FAIL' }) =="
exit $(if ($allOk) { 0 } else { 1 })
