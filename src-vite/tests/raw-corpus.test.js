import { createHash } from 'node:crypto'
import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

// Corpus gates for the RAW-development regression fixtures (lap-7f5.2 /
// TASK-102; engine-side twin: RapidRAW-engine src-tauri/tests/raw_corpus_gates.rs).
// They verify fixture integrity, provenance/permitted-use evidence, coverage,
// synthetic-generator determinism, and the pinned baseline manifest before the
// extraction starts modifying the engine (spec P4/A5/A6).

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../..',
)
const corpusRoot = path.join(repoRoot, 'tests/fixtures/raw-development')

function readJson(relative) {
  const file = path.join(corpusRoot, relative)
  return JSON.parse(readFileSync(file, 'utf8'))
}

function sha256File(relative) {
  const file = path.join(corpusRoot, relative)
  return createHash('sha256').update(readFileSync(file)).digest('hex')
}

function corpusEntries() {
  const manifest = readJson('corpus-manifest.json')
  expect(manifest.schema).toBe('lap-raw-corpus/v1')
  expect(Array.isArray(manifest.files)).toBe(true)
  expect(manifest.files.length).toBeGreaterThan(0)
  return manifest
}

describe('RAW regression corpus', () => {
  it('ships a corpus manifest with acquisition provenance', () => {
    const manifest = corpusEntries()
    expect(manifest.acquisition).toBeDefined()
    expect(manifest.acquisition.tool).toMatch(/acquire-corpus\.ps1$/)
    expect(manifest.acquisition.date).toMatch(/^\d{4}-\d{2}-\d{2}$/)
    // Bounded acquisition: a fixed byte budget must be declared and honored.
    expect(typeof manifest.acquisition.maxTotalBytes).toBe('number')
    const total = manifest.files.reduce((sum, f) => sum + f.bytes, 0)
    expect(total).toBeLessThanOrEqual(manifest.acquisition.maxTotalBytes)
  })

  it('keeps every fixture byte-identical to its recorded checksum', () => {
    const manifest = corpusEntries()
    for (const entry of manifest.files) {
      expect(entry.path).toMatch(/^[\w./-]+$/)
      expect(entry.sha256).toMatch(/^[0-9a-f]{64}$/)
      const file = path.join(corpusRoot, entry.path)
      expect(existsSync(file), entry.path).toBe(true)
      expect(sha256File(entry.path), entry.path).toBe(entry.sha256)
    }
  })

  it('records provenance and CC0 permitted-use evidence per fixture', () => {
    const manifest = corpusEntries()
    for (const entry of manifest.files) {
      expect(['rawdb.dnglab.org', 'derived', 'synthetic']).toContain(entry.origin)
      if (entry.origin === 'rawdb.dnglab.org') {
        expect(entry.sourceUrl).toMatch(/^https:\/\/rawdb\.dnglab\.org\//)
      }
      if (entry.origin === 'derived') {
        expect(entry.derivedFrom).toBeDefined()
      }
      expect(entry.license).toBe('CC0-1.0')
    }
  })

  it('covers Bayer, X-Trans, linear DNG, orientation, highlight stress and >4096 px', () => {
    const manifest = corpusEntries()
    const categories = new Set(manifest.files.flatMap((f) => f.categories ?? []))
    for (const required of [
      'bayer',
      'xtrans',
      'linear-dng',
      'orientation',
      'highlight-stress',
    ]) {
      expect(categories, `missing category ${required}`).toContain(required)
    }
    const large = manifest.files.some(
      (f) =>
        Math.max(...(f.decodedDimensions ?? [0, 0]), f.maxDimension ?? 0) > 4096,
    )
    expect(large).toBe(true)
  })

  it('excludes private photo library data (all entries are CC0-acquired)', () => {
    const manifest = corpusEntries()
    for (const entry of manifest.files) {
      expect(entry.license).toBe('CC0-1.0')
      expect(entry.origin).not.toBe('private-library')
    }
  })
})

describe('synthetic edge-case fixtures', () => {
  it('documents the generator and pins synthetic outputs', () => {
    const manifest = readJson('synthetic/synthetic-manifest.json')
    expect(manifest.schema).toBe('lap-raw-synthetic/v1')
    expect(manifest.generator).toMatch(/generate-synthetic-fixtures\.mjs$/)
    expect(manifest.files.length).toBeGreaterThan(0)
    for (const entry of manifest.files) {
      expect(entry.path).toMatch(/^synthetic\//)
      expect(entry.sha256).toMatch(/^[0-9a-f]{64}$/)
      expect(existsSync(path.join(corpusRoot, entry.path)), entry.path).toBe(true)
      expect(sha256File(entry.path), entry.path).toBe(entry.sha256)
    }
    // Numerical inputs and failure inputs are separate suites.
    const purposes = new Set(manifest.files.map((f) => f.purpose))
    expect(purposes).toContain('numerical')
    expect(purposes).toContain('failure')
  })
})

describe('pinned pre-extraction baseline manifest', () => {
  it('records engine revision, GPU/backend, color spaces, decode options and defaults', () => {
    const manifest = readJson('baselines/capture-manifest.json')
    expect(manifest.schema).toBe('lap-raw-baseline/v1')
    expect(manifest.engine.commit).toMatch(/^[0-9a-f]{7,40}$/)
    expect(manifest.engine.renderBaselineCommit).toBe(
      '5e30bcbb246395d391ba2e9662510641ffe68e6b',
    )
    expect(manifest.engine.gpu.backend.length).toBeGreaterThan(0)
    expect(manifest.engine.gpu.adapter.length).toBeGreaterThan(0)
    expect(manifest.engine.renderSourceSha256).toBeDefined()
    expect(manifest.colorSpaces.working).toBeDefined()
    expect(manifest.colorSpaces.output).toBeDefined()
    expect(manifest.defaults).toBeDefined()
    const decode = manifest.decodeOptions
    for (const key of [
      'highlightCompression',
      'linearRawMode',
      'preprocessingColorNoiseReduction',
      'preprocessingSharpening',
      'fastDemosaic',
    ]) {
      expect(decode[key], `decodeOptions.${key}`).toBeDefined()
    }
  })

  it('pins default, per-adjustment and combined output checksums per fixture', () => {
    const manifest = readJson('baselines/capture-manifest.json')
    const cases = manifest.cases
    expect(Array.isArray(cases)).toBe(true)
    expect(cases.length).toBeGreaterThan(0)
    // Failure-expectation cases pin explicit rejection (exit code + logged
    // error), never a render checksum.
    expect(cases.some((c) => c.expectFailure)).toBe(true)
    for (const c of cases) {
      expect(c.fixture).toMatch(/\.(cr3|raf|dng)$/i)
      expect(c.preset.length).toBeGreaterThan(0)
      if (c.expectFailure) {
        expect(c.exitCode).not.toBe(0)
        expect(typeof c.errorEvidence).toBe('string')
        expect(c.errorEvidence.length).toBeGreaterThan(0)
        continue
      }
      expect(c.outputSha256).toMatch(/^[0-9a-f]{64}$/)
      expect(Array.isArray(c.outputDimensions)).toBe(true)
      expect(c.outputDimensions.length).toBe(2)
    }
    const fixtures = new Set(cases.map((c) => c.fixture))
    // Every corpus entry has at least a default capture.
    const corpus = readJson('corpus-manifest.json')
    for (const entry of corpus.files.filter((f) => f.categories.includes('real'))) {
      expect(fixtures, `no baseline case for ${entry.path}`).toContain(entry.path)
    }
    const kinds = new Set(cases.map((c) => c.kind))
    expect(kinds).toContain('default')
    expect(kinds).toContain('per-adjustment')
    expect(kinds).toContain('combined')
    const perAdjustment = cases.filter((c) => c.kind === 'per-adjustment')
    expect(new Set(perAdjustment.map((c) => c.preset)).size).toBeGreaterThanOrEqual(5)
  })

  it('characterizes run-to-run determinism before freezing tolerances', () => {
    const manifest = readJson('baselines/capture-manifest.json')
    const determinism = manifest.determinism
    expect(determinism.method).toMatch(/repeat/i)
    expect(determinism.runs).toBeGreaterThanOrEqual(2)
    expect(['deterministic', 'nondeterministic']).toContain(determinism.result)
    if (determinism.result === 'deterministic') {
      expect(determinism.maxObservedAbsChannelDelta).toBe(0)
    } else {
      expect(determinism.maxObservedAbsChannelDelta).toBeGreaterThan(0)
      expect(determinism.tolerance).toBeGreaterThanOrEqual(
        determinism.maxObservedAbsChannelDelta,
      )
    }
  })

  it('commits the presets the baselines were captured with', () => {
    const manifest = readJson('baselines/capture-manifest.json')
    for (const c of manifest.cases) {
      const presetPath = path.join(corpusRoot, c.presetFile)
      expect(existsSync(presetPath), c.presetFile).toBe(true)
    }
    const presets = readJson('presets/presets-manifest.json')
    expect(presets.schema).toBe('lap-raw-presets/v1')
    for (const preset of presets.presets) {
      expect(existsSync(path.join(corpusRoot, preset.file))).toBe(true)
      expect(preset.sha256).toMatch(/^[0-9a-f]{64}$/)
      expect(sha256File(preset.file)).toBe(preset.sha256)
    }
  })
})
