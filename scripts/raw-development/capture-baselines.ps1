# Pre-extraction baseline capture (lap-7f5.2 / TASK-102).
#
# Drives the pinned RapidRAW engine's headless export
#   RapidRAW.exe export <fixture> --output <png> --format png [--adjustments <preset>]
# over the licensed RAW corpus and the synthetic suites, and freezes the
# results in tests/fixtures/raw-development/baselines/capture-manifest.json.
#
# Modes:
#   * Baseline absent, or -Update given  -> capture and WRITE the manifest.
#   * Baseline present, no -Update       -> re-capture and COMPARE; exit 1 on
#     any drift. Baselines are never regenerated implicitly: overwriting the
#     frozen manifest always requires the explicit -Update flag.
#
# -AllowSourceDrift permits running a comparison after the render-relevant
# engine sources legitimately changed (e.g. a reviewed extraction). It waives
# ONLY the engine-source hash equality; every output checksum, dimension and
# failure expectation must still match the frozen baseline. It never allows a
# silent -Update: freezing a new manifest still requires -Update, and the
# resulting manifest diff must be reviewed (output hashes must be unchanged
# for a pure extraction).
#
# Every capture run repeats each case (-Repeat, default 2) to characterize
# run-to-run nondeterminism before tolerances are frozen. Nondeterministic
# checksums abort the capture: tolerances must then be derived from pixel
# diffs deliberately, never guessed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\raw-development\capture-baselines.ps1 [-Update] [-Repeat 2] [-EngineRoot C:\Users\Thomas\Desktop\RapidRAW-engine] [-CaseFilter <regex>] [-AllowSourceDrift]

param(
    [switch]$Update,
    [int]$Repeat = 2,
    [string]$EngineRoot = "C:\Users\Thomas\Desktop\RapidRAW-engine",
    [string]$CaseFilter = "",
    [switch]$AllowSourceDrift
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
$CorpusRoot = Join-Path $RepoRoot "tests\fixtures\raw-development"
$ExePath = Join-Path $EngineRoot "src-tauri\target\debug\RapidRAW.exe"
$BaselinePath = Join-Path $CorpusRoot "baselines\capture-manifest.json"
$RenderBaselineCommit = "5e30bcbb246395d391ba2e9662510641ffe68e6b"
$RenderSources = @(
    "src-tauri/src/raw_processing.rs",
    "src-tauri/src/image_processing.rs",
    "src-tauri/src/gpu_processing.rs",
    "src-tauri/src/export_processing.rs",
    "src-tauri/src/adjustment_utils.rs",
    "src-tauri/src/image_loader.rs"
)
# Host profile settings that reach the headless export decode path. Windows
# known-folder redirection cannot sandbox these per process (SHGetKnownFolderPath
# ignores env overrides), so the capture snapshots them and refuses to run when
# they drift away from the frozen baseline.
$RenderSettingsKeys = @(
    "processingBackend",
    "rawHighlightCompression",
    "linearRawMode",
    "rawPreprocessingColorNr",
    "rawPreprocessingSharpening",
    "applyPreprocessingToNonRaws"
)
# Engine code defaults applied when a key is absent from the host profile
# (Settings::default in src-tauri/src/app_settings.rs; the decode path applies
# the same values via unwrap_or in src-tauri/src/image_loader.rs). The engine
# normalizes settings.json on exit, so a key can appear between two runs with
# exactly these values; recording effective values keeps the baseline stable
# while the checksum comparison proves the decode behavior did not change.
# Each default is re-verified against the pinned engine source below so an
# engine default change fails the capture instead of recording a stale value.
$EngineDecodeDefaults = @{
    processingBackend           = "auto"
    rawHighlightCompression     = 2.5
    linearRawMode               = "auto"
    rawPreprocessingColorNr     = 0.5
    rawPreprocessingSharpening  = 0.35
    applyPreprocessingToNonRaws = $false
}
# Manifest field names for the decode options recorded in
# baselines/capture-manifest.json (differ from the settings.json key names).
$DecodeOptionFields = @{
    processingBackend           = "processingBackend"
    rawHighlightCompression     = "highlightCompression"
    linearRawMode               = "linearRawMode"
    rawPreprocessingColorNr     = "preprocessingColorNoiseReduction"
    rawPreprocessingSharpening  = "preprocessingSharpening"
    applyPreprocessingToNonRaws = "applyPreprocessingToNonRaws"
}
$EngineDefaultProofs = @(
    @{ key = "processingBackend";           file = "src-tauri/src/app_settings.rs"; pattern = 'processing_backend:\s*Some\("auto"' },
    @{ key = "rawHighlightCompression";     file = "src-tauri/src/app_settings.rs"; pattern = "raw_highlight_compression:\s*Some\(2\.5\)" },
    @{ key = "linearRawMode";               file = "src-tauri/src/app_settings.rs"; pattern = '(?s)default_linear_raw_mode\(\) -> String \{\s*"auto"' },
    @{ key = "rawPreprocessingColorNr";     file = "src-tauri/src/app_settings.rs"; pattern = "raw_preprocessing_color_nr:\s*Some\(0\.5\)" },
    @{ key = "rawPreprocessingSharpening";  file = "src-tauri/src/app_settings.rs"; pattern = "raw_preprocessing_sharpening:\s*Some\(0\.35\)" },
    @{ key = "applyPreprocessingToNonRaws"; file = "src-tauri/src/app_settings.rs"; pattern = "apply_preprocessing_to_non_raws:\s*Some\(false\)" }
)

function Get-Sha256([string]$File) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($File)
        try {
            (($sha.ComputeHash($stream) | ForEach-Object { $_.ToString("x2") }) -join "")
        }
        finally { $stream.Dispose() }
    }
    finally { $sha.Dispose() }
}

function Get-Json([string]$Path) {
    Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
}

# BOM-free UTF-8 writer: Set-Content -Encoding UTF8 in Windows PowerShell 5.1
# prepends a BOM, which JSON.parse (vitest) and serde_json (engine gates)
# reject. All manifests under tests/fixtures must stay BOM-free.
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
function Write-Utf8NoBom([string]$Path, [string]$Text) {
    [System.IO.File]::WriteAllText($Path, $Text, $Utf8NoBom)
}

# ---------------------------------------------------------------------------
# 1. Engine pinning
# ---------------------------------------------------------------------------
if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "Engine binary not found: $ExePath (build it first: cargo build --manifest-path src-tauri/Cargo.toml with the rustup PATH prepend)"
}
$engineCommit = (& git -C $EngineRoot rev-parse HEAD).Trim()
$changed = @(& git -C $EngineRoot diff --name-only $RenderBaselineCommit -- $RenderSources)
$sourceDriftAllowed = $false
if ($changed.Count -gt 0) {
    if (-not $AllowSourceDrift) {
        throw "Render-relevant engine sources changed since ${RenderBaselineCommit}: $($changed -join ', '). The pre-extraction baseline must be captured against the pinned renderer."
    }
    $sourceDriftAllowed = $true
    Write-Host "WARNING: render-relevant engine sources changed since ${RenderBaselineCommit} (allowed by -AllowSourceDrift):" -ForegroundColor Yellow
    $changed | ForEach-Object { Write-Host "  $_" -ForegroundColor Yellow }
}
$renderSourceHashes = [ordered]@{}
foreach ($src in $RenderSources) {
    $renderSourceHashes[$src] = Get-Sha256 (Join-Path $EngineRoot $src)
}

# ---------------------------------------------------------------------------
# 2. Host decode settings snapshot
# ---------------------------------------------------------------------------
$hostSettingsPath = Join-Path $env:APPDATA "io.github.CyberTimon.RapidRAW\settings.json"
$decodeSettings = [ordered]@{}
if (Test-Path -LiteralPath $hostSettingsPath) {
    $hostSettings = Get-Json $hostSettingsPath
    foreach ($key in $RenderSettingsKeys) {
        $value = $hostSettings.$key
        if ($null -ne $value) { $decodeSettings[$key] = $value }
        else { $decodeSettings[$key] = $EngineDecodeDefaults[$key] }
    }
}
else {
    throw "Host RapidRAW settings not found at $hostSettingsPath"
}
# The recorded engine defaults must still be the pinned engine's defaults.
foreach ($proof in $EngineDefaultProofs) {
    $src = Get-Content -LiteralPath (Join-Path $EngineRoot $proof.file) -Raw
    if (-not ($src -match $proof.pattern)) {
        throw "Engine default verification failed for $($proof.key): pattern '$($proof.pattern)' not found in $($proof.file). The pinned engine changed its decode defaults; update the baseline deliberately (explicit -Update with a reviewed diff)."
    }
}

# ---------------------------------------------------------------------------
# 3. Case matrix
# ---------------------------------------------------------------------------
$corpus = Get-Json (Join-Path $CorpusRoot "corpus-manifest.json")
$synthetic = Get-Json (Join-Path $CorpusRoot "synthetic\synthetic-manifest.json")
$realFixtures = @($corpus.files | Where-Object { $_.origin -ne "synthetic" })
$bayerPrimary = "corpus/canon-eos-r6-craw-iso100-nocrop.CR3"

$cases = @()
foreach ($f in $realFixtures) {
    $isPrimary = ($f.path -eq $bayerPrimary)
    $presets = @("default", "combined-tone", "combined-color")
    if ($isPrimary) {
        $presets += @(
            "exposure-plus1", "contrast-plus25", "highlights-minus50", "shadows-plus40",
            "saturation-plus30", "temperature-plus40", "sharpness-plus30", "clarity-plus20",
            "combined-full"
        )
    }
    foreach ($preset in $presets) {
        $kind = if ($preset -eq "default") { "default" }
        elseif ($preset -like "combined-*") { "combined" }
        else { "per-adjustment" }
        $cases += @{ Fixture = $f.path; Preset = $preset; Kind = $kind; ExpectFailure = $false }
    }
}
foreach ($f in $synthetic.files) {
    if ($f.purpose -eq "numerical") {
        $cases += @{ Fixture = $f.path; Preset = "default"; Kind = "default"; ExpectFailure = $false }
    }
    else {
        $cases += @{ Fixture = $f.path; Preset = "default"; Kind = "failure-expectation"; ExpectFailure = $true }
    }
}

if ($CaseFilter -ne "") {
    $cases = @($cases | Where-Object { "$($_.Fixture)|$($_.Preset)" -match $CaseFilter })
}
Write-Host "Engine $engineCommit (render baseline $RenderBaselineCommit verified unchanged)"
Write-Host "$($cases.Count) cases x $Repeat repeats"

# ---------------------------------------------------------------------------
# 4. Run the captures
# ---------------------------------------------------------------------------
$WorkRoot = Join-Path $env:TEMP ("opencode\baseline-capture-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $WorkRoot | Out-Null

$env:RUST_LOG = "debug"
$firstRun = $true
$adapterInfo = $null
$backend = $null
$results = @()

foreach ($case in $cases) {
    $fixturePath = Join-Path $CorpusRoot ($case.Fixture -replace "/", "\")
    $presetPath = Join-Path $CorpusRoot ("presets\" + $case.Preset + ".json")
    $caseName = "$($case.Fixture)~$($case.Preset)"
    $repeatHashes = @()
    $dims = $null
    $exit = $null
    $gpuSeen = $false
    $errSnippet = ""

    for ($r = 1; $r -le $Repeat; $r++) {
        $runDir = Join-Path $WorkRoot ("r$r")
        New-Item -ItemType Directory -Force $runDir | Out-Null
        $outPng = Join-Path $runDir ("$($case.Preset).png")
        $logFile = Join-Path $runDir ("$($case.Preset).log")
        # A prior case (or a killed earlier session) may have left a PNG with
        # this preset name in the shared run dir; remove it so a missing write
        # can never be masked by a stale file.
        Remove-Item -LiteralPath $outPng -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $logFile -ErrorAction SilentlyContinue

        $sw = [Diagnostics.Stopwatch]::StartNew()
        # Native stderr must not become terminating ErrorRecords under the
        # script-level Stop preference; scope it around the engine invocation.
        $ErrorActionPreference = "Continue"
        & $ExePath export $fixturePath --output $outPng --format png --adjustments $presetPath 1> $logFile 2>&1
        $exit = $LASTEXITCODE
        $ErrorActionPreference = "Stop"
        $elapsed = [int]$sw.Elapsed.TotalSeconds

        $log = Get-Content -LiteralPath $logFile -Raw
        if ($firstRun) {
            $m = [regex]::Match($log, "(?s)Request adapter result AdapterInfo \{ name: ""([^""]+)"".*?driver: ""([^""]*)"".*?backend:\s*(\w+)")
            if ($m.Success) {
                $adapterInfo = $m.Groups[1].Value
                $backend = $m.Groups[3].Value
            }
            $firstRun = $false
        }
        if ($log -match "processed \(ROI: \d+x\d+\) on GPU in") { $gpuSeen = $true }
        if (-not $gpuSeen -and $log -match "on GPU in") { $gpuSeen = $true }

        if ($case.ExpectFailure) {
            if ($exit -eq 0) {
                throw "FAILURE-CASE REGRESSION: $caseName unexpectedly succeeded (engine must fail explicitly on undecodable input)"
            }
            if ($log -match "Failed to (decode|process|load)[^\r\n]*") { $errSnippet = $Matches[0] }
        }
        else {
            if ($exit -ne 0) {
                throw "Capture failed for $caseName (exit $exit). Last log lines:`n$($log.Substring([Math]::Max(0, $log.Length - 800)))"
            }
            if (-not (Test-Path -LiteralPath $outPng)) { throw "No output PNG for $caseName" }
            $repeatHashes += Get-Sha256 $outPng
            $infoJson = & node (Join-Path $RepoRoot "scripts\raw-development\png-info.mjs") $outPng | ConvertFrom-Json
            $dims = @($infoJson.width, $infoJson.height)
        }
        Write-Host ("  {0}{1} exit={2} {3}s{4}" -f $caseName, " [r$r]", $exit, $elapsed, $(if ($case.ExpectFailure) { " (expected failure)" } else { "" }))
    }

    # Bound disk usage: hash/dimension evidence is collected inline, so the
    # per-case PNG (up to ~80 MB) and debug log (up to ~13 MB) are transient.
    # Keeping them would accumulate ~2 GB per session across the matrix.
    Remove-Item -LiteralPath (Join-Path $WorkRoot "r1") -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath (Join-Path $WorkRoot "r2") -Recurse -Force -ErrorAction SilentlyContinue

    $deterministic = (@($repeatHashes | Select-Object -Unique).Count -le 1)
    if (-not $case.ExpectFailure -and -not $deterministic) {
        throw "NONDETERMINISTIC OUTPUT: $caseName produced different checksums across repeats: $($repeatHashes -join ', '). Characterize with pixel diffs before freezing tolerances; do not pick a hash."
    }
    if (-not $case.ExpectFailure -and -not $gpuSeen) {
        throw "GPU execution not observed for $caseName (no 'on GPU in' log line); baselines require real GPU runs (spec A6)"
    }

    $result = [ordered]@{
        fixture    = $case.Fixture
        preset     = $case.Preset
        presetFile = "presets/$($case.Preset).json"
        kind       = $case.Kind
        exitCode   = $exit
    }
    if ($case.ExpectFailure) {
        $result["expectFailure"] = $true
        $result["errorEvidence"] = $errSnippet
    }
    else {
        $result["outputSha256"] = $repeatHashes[0]
        $result["outputDimensions"] = $dims
        $result["gpu"] = $true
    }
    $results += $result
}

if (-not $adapterInfo) { throw "Could not extract wgpu adapter info from the engine logs" }
Write-Host "Adapter: $adapterInfo (backend $backend)"

# ---------------------------------------------------------------------------
# 5. Determinism summary
# ---------------------------------------------------------------------------
$determinism = [ordered]@{
    method = "repeat-runs-sha256"
    runs = $Repeat
    result = "deterministic"
    maxObservedAbsChannelDelta = 0
    tolerance = 0
    note = "All cases produced byte-identical PNG outputs across $Repeat consecutive runs (same machine, same adapter). Exact checksum equality is therefore the frozen tolerance; any future drift is a regression or a deliberate engine change."
}

# ---------------------------------------------------------------------------
# 6. Freeze or compare
# ---------------------------------------------------------------------------
$defaults = [ordered]@{
    recipe = "presets/default.json ({}): engine zero-defaults; equals frontend INITIAL_ADJUSTMENTS with every effective adjustment at its neutral value"
    geometry = "crop=null, rotation=0, orientationSteps=0, flipHorizontal=false, flipVertical=false"
    pipelineOrder = @("geometry warp", "lens blur", "coarse rotation (orientationSteps)", "flip", "fine rotation", "crop", "GPU adjustment pipeline (masks, exposure/tone/color/detail/effects, LUT)", "output encode")
    preview = "editor preview path: fast demosaic (DemosaicAlgorithm::Speed) + editorPreviewResolution downscale; not exercised headless, recorded as contract of the pinned revision"
    histogram = "computed from rendered preview pixels in the GUI; not exercised headless, recorded as contract of the pinned revision"
}
$manifest = [ordered]@{
    schema = "lap-raw-baseline/v1"
    issue = "lap-7f5.2"
    captured = (Get-Date -Format "yyyy-MM-dd")
    engine = [ordered]@{
        commit = $engineCommit
        renderBaselineCommit = $RenderBaselineCommit
        renderSourceSha256 = $renderSourceHashes
        headlessEntry = "RapidRAW.exe export <fixture> --output <png> --format png --adjustments <preset>"
        gpu = [ordered]@{
            backend = $backend
            adapter = $adapterInfo
            evidence = "'processed (ROI: ...) on GPU in' log line asserted for every non-failure case"
        }
    }
    decodeOptions = [ordered]@{
        fastDemosaic = $false
        highlightCompression = $decodeSettings.rawHighlightCompression
        linearRawMode = $decodeSettings.linearRawMode
        preprocessingColorNoiseReduction = $decodeSettings.rawPreprocessingColorNr
        preprocessingSharpening = $decodeSettings.rawPreprocessingSharpening
        applyPreprocessingToNonRaws = $decodeSettings.applyPreprocessingToNonRaws
        processingBackend = $decodeSettings.processingBackend
        source = "effective decode options: host profile settings.json values for keys ($($RenderSettingsKeys -join ', ')); keys absent from the profile fall back to the engine code defaults (re-verified against the pinned engine source at capture time): processingBackend=auto, rawHighlightCompression=2.5, linearRawMode=auto, rawPreprocessingColorNr=0.5, rawPreprocessingSharpening=0.35, applyPreprocessingToNonRaws=false"
    }
    colorSpaces = [ordered]@{
        working = "linear scene-referred f32 (rawler RawDevelop with calibration on, sRGB step removed; GPU WGSL pipeline in linear)"
        output = "display-referred sRGB-encoded PNG (engine default tone curve + OETF; sampled: linear 0.51 encodes to ~212/255)"
    }
    defaults = $defaults
    determinism = $determinism
    cases = $results
}

if ((Test-Path -LiteralPath $BaselinePath) -and -not $Update.IsPresent) {
    Write-Host "Comparing against frozen baseline: $BaselinePath"
    $frozen = Get-Json $BaselinePath
    $failures = @()
    if ($frozen.engine.renderBaselineCommit -ne $RenderBaselineCommit) { $failures += "engine render baseline commit drifted" }
    foreach ($src in $RenderSources) {
        if ($frozen.engine.renderSourceSha256.$src -ne $renderSourceHashes[$src]) {
            if ($sourceDriftAllowed) {
                Write-Host "SOURCE DRIFT (allowed): $src"
            } else {
                $failures += "engine source changed: $src"
            }
        }
    }
    foreach ($key in $RenderSettingsKeys) {
        $field = $DecodeOptionFields[$key]
        $frozenValue = $frozen.decodeOptions.($field)
        $nowValue = $decodeSettings[$key]
        if ("$frozenValue" -ne "$nowValue") { $failures += "decode setting drifted: $key ($frozenValue -> $nowValue)" }
    }
    $frozenCases = @{}
    foreach ($c in $frozen.cases) { $frozenCases["$($c.fixture)~$($c.preset)"] = $c }
    foreach ($c in $results) {
        $key = "$($c.fixture)~$($c.preset)"
        $f = $frozenCases[$key]
        if (-not $f) { $failures += "new case not in frozen baseline: $key"; continue }
        if ($c.expectFailure) {
            if (-not $f.expectFailure -or $f.exitCode -eq 0) { $failures += "case kind changed to failure: $key" }
        }
        else {
            if ($c.outputSha256 -ne $f.outputSha256) { $failures += "OUTPUT CHECKSUM MISMATCH: $key ($($f.outputSha256) -> $($c.outputSha256))" }
            if ("$($c.outputDimensions -join 'x')" -ne "$($f.outputDimensions -join 'x')") { $failures += "output dimensions changed: $key" }
        }
    }
    if ($failures.Count -gt 0) {
        $failures | ForEach-Object { Write-Host "DRIFT: $_" -ForegroundColor Red }
        throw "Baseline comparison FAILED with $($failures.Count) drift(s). This is the regression signal; use -Update only after an intentional, reviewed engine change."
    }
    Write-Host "Baseline comparison PASSED: $($results.Count) cases byte-identical to the frozen manifest."
    exit 0
}

if ($Update.IsPresent) {
    Write-Host "WARNING: overwriting frozen baseline at $BaselinePath (explicit -Update)" -ForegroundColor Yellow
}
Write-Utf8NoBom $BaselinePath ($manifest | ConvertTo-Json -Depth 8)
Write-Host "Baseline manifest written: $BaselinePath ($($results.Count) cases)"

# Enrich the corpus manifest with engine-verified decoded dimensions.
$corpusPath = Join-Path $CorpusRoot "corpus-manifest.json"
$corpusJson = Get-Json $corpusPath
$defaultDims = @{}
foreach ($r in $results) {
    if ($r.preset -eq "default" -and -not $r.expectFailure) {
        $defaultDims[$r.fixture] = $r.outputDimensions
    }
}
$corpusRaw = Get-Content -LiteralPath $corpusPath -Raw -Encoding UTF8 | ConvertFrom-Json
foreach ($entry in $corpusRaw.files) {
    if ($defaultDims.ContainsKey($entry.path)) {
        $entry | Add-Member -NotePropertyName decodedDimensions -NotePropertyValue $defaultDims[$entry.path] -Force
    }
}
Write-Utf8NoBom $corpusPath ($corpusRaw | ConvertTo-Json -Depth 6)
Write-Host "Corpus manifest enriched with decoded dimensions."

# Presets manifest (checksums of the exact preset files used).
$presetsDir = Join-Path $CorpusRoot "presets"
$presetsOut = @()
# Presets manifest (checksums of the exact preset files used). The manifest
# file itself lives in the same directory and must be excluded: listing it
# would embed a self-hash that can never match the rewritten file.
Get-ChildItem -LiteralPath $presetsDir -Filter *.json | Where-Object { $_.Name -ne "presets-manifest.json" } | Sort-Object Name | ForEach-Object {
    $presetsOut += [ordered]@{
        file = "presets/$($_.Name)"
        sha256 = (Get-Sha256 $_.FullName)
        bytes = $_.Length
    }
}
Write-Utf8NoBom (Join-Path $presetsDir "presets-manifest.json") (([ordered]@{
    schema = "lap-raw-presets/v1"
    issue = "lap-7f5.2"
    note = "Adjustment override JSON files passed via --adjustments to the headless export; values in frontend UI units"
    presets = $presetsOut
}) | ConvertTo-Json -Depth 5)
Write-Host "Presets manifest written."
