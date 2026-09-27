# RAW regression corpus acquisition (lap-7f5.2 / TASK-102).
#
# Downloads a fixed, bounded set of CC0-1.0 RAW fixtures from the dnglab
# RawDB (https://rawdb.dnglab.org), verifies every byte against the pinned
# sha256 recorded below (taken from the RawDB API, which requires contributed
# sample sets to be CC0 per dnglab's CONTRIBUTE_SAMPLES.md), and writes
# corpus-manifest.json with full provenance.
#
# The download list is a pinned constant: no discovery, no user photo
# libraries, no unlicensed sources. Total size is bounded by $MaxTotalBytes.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\raw-development\acquire-corpus.ps1
#   ... -SkipDownload   # only verify already-downloaded files and refresh the manifest

param(
    [switch]$SkipDownload
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
$CorpusRoot = Join-Path $RepoRoot "tests\fixtures\raw-development"

# Pinned acquisition list. sha256 values come from the RawDB API records
# (https://rawdb.dnglab.org/api/sets/<maker>/<model>), license "CC0 1.0".
$Files = @(
    @{
        Path      = "canon-eos-r6-craw-iso100-nocrop.CR3"
        Maker     = "Canon"; Model = "EOS R6"; Category = "raw_modes"
        Remote    = "Canon EOS R6_CRAW_ISO_100_nocrop_nodual.CR3"
        Bytes     = 11205206
        Sha256    = "4775df76433b1e5844bc7a42b95d93efc8438c0a4cd9681e120e23fe3e3007e8"
        Categories = @("bayer", "real", "fullres")
        Notes     = "CI-exercised rawler set; 5472x3648 Bayer CFA, >4096 px; primary per-adjustment matrix fixture"
    },
    @{
        Path      = "fujifilm-xt5-lossy-iso125.RAF"
        Maker     = "Fujifilm"; Model = "X-T5"; Category = "raw_modes"
        Remote    = "X-T5_ISO_125_Bitdepth_14_lossy.RAF"
        Bytes     = 28040304
        Sha256    = "cddd7ae0c43f9280876e5fdcbdf8878718972ebd3d034d8affc8cfc6752d33dc"
        Categories = @("xtrans", "real", "fullres")
        Notes     = "X-Trans CMOS 5 HR (7728x5152), lossy-compressed; X-Trans demosaic coverage"
    },
    @{
        Path      = "dng-jpegxl-lossy-16bit-linear-tiles.DNG"
        Maker     = "dnglab"; Model = "dng-compression-variants"; Category = "variants"
        Remote    = "dng_jpegxl_lossy_16bit_linear_tiles.dng"
        Bytes     = 14544148
        Sha256    = "21adc5c88d0dd723dc9ca76173b3f1b1515c2aff9abbbb657fc7dfb494c1507d"
        Categories = @("linear-dng", "real")
        Notes     = "Linear (non-CFA) 16-bit DNG, JPEG-XL lossy tiles; dnglab-authored CC0 fixture"
    },
    @{
        Path      = "canon-eos-r6-craw-iso204800-nocrop.CR3"
        Maker     = "Canon"; Model = "EOS R6"; Category = "iso_craw_nocrop_nodual"
        Remote    = "Canon EOS R6_CRAW_ISO_204800_nocrop_nodual.CR3"
        Bytes     = 14971990
        Sha256    = "faabad3baceab3545fd9ae8e716f987534106e866101bfa431a74e0ae92e9653"
        Categories = @("highlight-stress", "bayer", "real", "fullres")
        Notes     = "ISO 204800 daylight capture: massively clipped highlights + extreme noise"
    }
)
$MaxTotalBytes = 200 * 1024 * 1024

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

# BOM-free UTF-8 writer: Set-Content -Encoding UTF8 in Windows PowerShell 5.1
# prepends a BOM, which JSON.parse (vitest) and serde_json (engine gates)
# reject. All manifests under tests/fixtures must stay BOM-free; this is the
# same writer capture-baselines.ps1 uses.
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
function Write-Utf8NoBom([string]$Path, [string]$Text) {
    [System.IO.File]::WriteAllText($Path, $Text, $Utf8NoBom)
}

# Engine-verified decoded dimensions (enriched by capture-baselines.ps1) must
# survive re-verification re-runs of this script: carry them over whenever a
# fixture is byte-identical (same sha256) to the previously recorded entry.
$manifestPath = Join-Path $CorpusRoot "corpus-manifest.json"
$previousByPath = @{}
if (Test-Path -LiteralPath $manifestPath) {
    $previous = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($entry in @($previous.files)) {
        if ($null -ne $entry.decodedDimensions) {
            $previousByPath[$entry.path] = @{ sha256 = $entry.sha256; decodedDimensions = $entry.decodedDimensions }
        }
    }
}
function Get-CarriedDimensions([string]$Path, [string]$Sha256) {
    $prior = $previousByPath[$Path]
    if ($prior -and $prior.sha256 -eq $Sha256) { return $prior.decodedDimensions }
    return $null
}

New-Item -ItemType Directory -Force (Join-Path $CorpusRoot "corpus") | Out-Null
New-Item -ItemType Directory -Force (Join-Path $CorpusRoot "baselines") | Out-Null
New-Item -ItemType Directory -Force (Join-Path $CorpusRoot "presets") | Out-Null
New-Item -ItemType Directory -Force (Join-Path $CorpusRoot "synthetic") | Out-Null

$total = 0
foreach ($f in $Files) { $total += [long]$f.Bytes }
if ($total -gt $MaxTotalBytes) {
    throw "Pinned acquisition list ($total bytes) exceeds the declared budget $MaxTotalBytes"
}

foreach ($f in $Files) {
    $target = Join-Path $CorpusRoot ("corpus\" + $f.Path)
    $needDownload = -not (Test-Path -LiteralPath $target)
    if ($needDownload -and -not $SkipDownload.IsPresent) {
        $url = "https://rawdb.dnglab.org/api/download/{0}/{1}/{2}/{3}" -f
            [uri]::EscapeDataString($f.Maker),
            [uri]::EscapeDataString($f.Model),
            [uri]::EscapeDataString($f.Category),
            [uri]::EscapeDataString($f.Remote)
        Write-Host "Downloading $($f.Remote) from rawdb.dnglab.org ..."
        $tmp = "$target.partial"
        # Bound the transfer: rawdb is a small community host; polite single
        # stream with a hard timeout per file and bounded 429 retries that
        # honor the server's Retry-After hint.
        $attempt = 0
        while ($true) {
            $attempt++
            try {
                Invoke-WebRequest -Uri $url -OutFile $tmp -MaximumRedirection 5
                break
            }
            catch [System.Net.WebException] {
                $response = $_.Exception.Response
                $code = if ($response) { [int]$response.StatusCode } else { 0 }
                if ($code -eq 429 -and $attempt -lt 6) {
                    $wait = 60
                    if ($response.Headers["Retry-After"]) {
                        $parsed = 0
                        if ([int]::TryParse($response.Headers["Retry-After"], [ref]$parsed)) {
                            $wait = [Math]::Min($parsed, 120)
                        }
                    }
                    Write-Host "  rate-limited (429); waiting ${wait}s (attempt $attempt)"
                    Start-Sleep -Seconds $wait
                    continue
                }
                throw
            }
        }
        if ((Get-Item -LiteralPath $tmp).Length -ne $f.Bytes) {
            Remove-Item -LiteralPath $tmp -Force
            throw "Size mismatch for $($f.Path): expected $($f.Bytes) bytes"
        }
        $hash = Get-Sha256 $tmp
        if ($hash -ne $f.Sha256) {
            Remove-Item -LiteralPath $tmp -Force
            throw "sha256 mismatch for $($f.Path): expected $($f.Sha256), got $hash"
        }
        Move-Item -LiteralPath $tmp -Destination $target -Force
    }
    elseif ($needDownload) {
        throw "Missing fixture $($f.Path) and -SkipDownload given"
    }

    $hash = Get-Sha256 $target
    if ($hash -ne $f.Sha256) {
        throw "Local fixture $($f.Path) does not match its pinned sha256 (expected $($f.Sha256), got $hash)"
    }
}

# Derived fixture: orientation coverage. RawDB carries no orientation-tagged
# sample set, so the corpus derives one from the CC0 linear DNG by rewriting
# TIFF tag 274 from 1 to 6 (90-degree display rotation). The derivation is a
# deterministic byte patch (make-orientation-variant.mjs) and inherits the
# source's CC0-1.0 license.
Write-Host "Deriving orientation variant via make-orientation-variant.mjs ..."
$linearDng = Join-Path $CorpusRoot "corpus\dng-jpegxl-lossy-16bit-linear-tiles.DNG"
$orientationDng = Join-Path $CorpusRoot "corpus\dng-linear-orientation6.DNG"
$deriveScript = Join-Path $RepoRoot "scripts\raw-development\make-orientation-variant.mjs"
$derivationJson = & node $deriveScript $linearDng $orientationDng 6
if ($LASTEXITCODE -ne 0) {
    throw "make-orientation-variant.mjs failed with exit $LASTEXITCODE"
}
$derivation = $derivationJson | ConvertFrom-Json
if ($derivation.input.sha256 -ne ($Files | Where-Object { $_.Path -eq "dng-jpegxl-lossy-16bit-linear-tiles.DNG" }).Sha256) {
    throw "orientation derivation input checksum drifted"
}

# The manifest is derived strictly from the pinned list above plus on-disk
# verification results; per-file provenance is complete for every entry.
$manifestFiles = @()
foreach ($f in $Files) {
    $target = Join-Path $CorpusRoot ("corpus\" + $f.Path)
    $item = Get-Item -LiteralPath $target
    $entry = [ordered]@{
        path        = "corpus/$($f.Path)"
        bytes       = $item.Length
        sha256      = (Get-Sha256 $target)
        origin      = "rawdb.dnglab.org"
        sourceUrl   = "https://rawdb.dnglab.org/api/download/{0}/{1}/{2}/{3}" -f
            [uri]::EscapeDataString($f.Maker),
            [uri]::EscapeDataString($f.Model),
            [uri]::EscapeDataString($f.Category),
            [uri]::EscapeDataString($f.Remote)
        license     = "CC0-1.0"
        licenseEvidence = "RawDB API record license field ('CC0 1.0'); dnglab CONTRIBUTE_SAMPLES.md requires contributed sample sets to be CC0 with contributor copyright ownership"
        categories  = $f.Categories
        notes       = $f.Notes
    }
    $carried = Get-CarriedDimensions $entry.path $entry.sha256
    if ($null -ne $carried) { $entry["decodedDimensions"] = $carried }
    $manifestFiles += $entry
}

$derivedEntry = [ordered]@{
    path        = "corpus/dng-linear-orientation6.DNG"
    bytes       = $derivation.output.bytes
    sha256      = $derivation.output.sha256
    origin      = "derived"
    derivedFrom = [ordered]@{
        path            = "corpus/dng-jpegxl-lossy-16bit-linear-tiles.DNG"
        sha256          = $derivation.input.sha256
        transformation  = $derivation.transformation
        tool            = "scripts/raw-development/make-orientation-variant.mjs"
    }
    license     = "CC0-1.0"
    licenseEvidence = "Byte-identical copy of the CC0-1.0 RawDB linear DNG except the 2-byte inline TIFF Orientation value; no new authorship"
    categories  = @("orientation", "linear-dng", "derived")
    notes       = "Orientation=6 fixture: exercises the engine's rawler orientation application path; expect transposed output dimensions"
}
$derivedCarried = Get-CarriedDimensions $derivedEntry.path $derivedEntry.sha256
if ($null -ne $derivedCarried) { $derivedEntry["decodedDimensions"] = $derivedCarried }
$manifestFiles += $derivedEntry

$manifest = [ordered]@{
    schema      = "lap-raw-corpus/v1"
    issue       = "lap-7f5.2"
    acquisition = [ordered]@{
        tool          = "scripts/raw-development/acquire-corpus.ps1"
        date          = (Get-Date -Format "yyyy-MM-dd")
        source        = "https://rawdb.dnglab.org (dnglab RawDB)"
        policy        = "Pinned file list only; sha256 verified against RawDB API records before acceptance; deterministic derived orientation variant; no private photo libraries; no unlicensed sources"
        maxTotalBytes = $MaxTotalBytes
        totalBytes    = ($total + [long]$derivation.output.bytes)
    }
    files       = $manifestFiles
}

Write-Utf8NoBom $manifestPath ($manifest | ConvertTo-Json -Depth 6)
Write-Host "Corpus verified and manifest written: $manifestPath ($($manifestFiles.Count) fixtures)"
Write-Host "NOTE: synthetic fixtures are generated separately by generate-synthetic-fixtures.mjs; run it next."
