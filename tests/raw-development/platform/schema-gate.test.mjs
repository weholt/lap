// Schema-validation and platform release-gate tests for the raw-development
// performance harness (lap-c0e / TASK-601; governing contract
// docs/raw-development/spec.md A7/A11/A12/P5).
//
// These tests run with the zero-dependency Node built-in test runner:
//   node --test tests/raw-development/platform/
//
// They pin the behavior that the acceptance criteria require:
//   - a results schema that carries named hardware/backend, fixture
//     revisions, raw samples and computed percentiles;
//   - the provisional warm-1536px p95 target (150 ms) can be reported as
//     MISSED but never silently redefined;
//   - the platform release gate is fail-closed: any missing, failed,
//     malformed or unexecuted platform (Linux/macOS/absent machines)
//     blocks release; a CI workflow definition alone never satisfies it.

import { test, describe } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'

import {
  PLATFORM_RESULTS_SCHEMA,
  PLATFORM_MANIFEST_SCHEMA,
  PROVISIONAL_WARM_TARGET,
  REQUIRED_WORKLOAD_IDS,
  validateResults,
  evaluatePlatformGate,
} from '../../../scripts/raw-development/benchmark.mjs'

function tempRepo() {
  const root = mkdtempSync(path.join(tmpdir(), 'lap-platform-gate-'))
  mkdirSync(path.join(root, 'runs'), { recursive: true })
  mkdirSync(path.join(root, '.github', 'workflows'), { recursive: true })
  return root
}

function writeResults(dir, machineId, overrides = {}) {
  const results = validResults(machineId, overrides)
  const file = path.join(dir, 'runs', `platform-results-${machineId}.json`)
  writeFileSync(file, JSON.stringify(results, null, 2))
  return file
}

function validManifest(machineOverrides = []) {
  return {
    schema: PLATFORM_MANIFEST_SCHEMA,
    issue: 'lap-c0e',
    requiredWorkloads: [...REQUIRED_WORKLOAD_IDS],
    provisionalTargets: [PROVISIONAL_WARM_TARGET],
    machines: [
      {
        id: 'windows-test-gpu',
        status: 'available',
        os: 'windows',
        hardware: { cpu: 'Test CPU', ramBytes: 34359738368 },
        gpu: { adapter: 'Test GPU 9000', backends: ['vulkan', 'dx12'] },
      },
      {
        id: 'linux-reference',
        status: 'unavailable',
        reason: 'no Linux reference machine is available to this development environment',
      },
      {
        id: 'macos-reference',
        status: 'unavailable',
        reason: 'no macOS reference machine is available to this development environment',
      },
      ...machineOverrides,
    ],
  }
}

function validResults(machineId = 'windows-test-gpu', overrides = {}) {
  const base = {
    schema: PLATFORM_RESULTS_SCHEMA,
    issue: 'lap-c0e',
    machineId,
    executed: {
      startedUtc: '2026-09-28T10:00:00Z',
      finishedUtc: '2026-09-28T10:05:00Z',
      host: 'run-on-real-hardware',
      command: 'lap-raw-platform.exe full',
      os: 'windows',
      lapRevision: 'a'.repeat(40),
      engineRevision: 'b'.repeat(40),
    },
    hardware: { cpu: 'Test CPU', ramBytes: 34359738368 },
    gpu: { adapter: 'Test GPU 9000', backend: 'vulkan', maxTextureDimension2d: 32768 },
    fixtures: [
      { path: 'corpus/fixture-a.CR3', sha256: 'c'.repeat(64), bytes: 10 },
    ],
    workloads: {
      'cold-decode': { outcome: 'pass', samplesMs: [100, 120], p50Ms: 110, p95Ms: 120 },
      'warm-slider-1536': {
        outcome: 'pass',
        samplesMs: [40, 41, 42],
        p50Ms: 41,
        p95Ms: 42,
        target: { thresholdMs: PROVISIONAL_WARM_TARGET.thresholdMs, provisional: true, met: true },
      },
      'settled-preview': { outcome: 'pass', samplesMs: [70], p50Ms: 70, p95Ms: 70 },
      export: { outcome: 'pass', samplesMs: [900], p50Ms: 900, p95Ms: 900 },
      'thumbnail-throughput-indexing': {
        outcome: 'pass',
        thumbnails: 10,
        totalMs: 5000,
        perSecond: 2,
        memorySeries: [{ label: 'mid', workingSetBytes: 1 }],
      },
      'navigation-memory-bound': {
        outcome: 'pass',
        iterations: 12,
        growthBytes: 1000,
        boundBytes: 268435456,
        bounded: true,
        memorySeries: [
          { label: 'iter-0', workingSetBytes: 1 },
          { label: 'iter-11', workingSetBytes: 1001 },
        ],
      },
      'gpu-failure-modes': {
        outcome: 'pass',
        checks: [
          { id: 'missing-adapter-typed-failure', ok: true, detail: 'typed error' },
          { id: 'texture-limit-typed-failure', ok: true, detail: 'typed error' },
          { id: 'device-loss-injection', ok: true, detail: 'not software-reproducible on this host; typed path verified' },
          { id: 'oom-injection', ok: true, detail: 'not software-reproducible on this host; typed path verified' },
        ],
      },
    },
  }
  return deepMerge(base, overrides)
}

function deepMerge(base, override) {
  const out = structuredClone(base)
  for (const [key, value] of Object.entries(override)) {
    if (value && typeof value === 'object' && !Array.isArray(value) && typeof out[key] === 'object' && out[key] !== null && !Array.isArray(out[key])) {
      out[key] = deepMerge(out[key], value)
    } else {
      out[key] = value
    }
  }
  return out
}

describe('results schema validation', () => {
  test('accepts a complete, passing results document', () => {
    const verdict = validateResults(validResults())
    assert.equal(verdict.ok, true, verdict.problems.join('; '))
    assert.deepEqual(verdict.problems, [])
  })

  test('rejects an unsupported schema version', () => {
    const verdict = validateResults(validResults('windows-test-gpu', { schema: 'lap-raw-platform-results/v2' }))
    assert.equal(verdict.ok, false)
    assert.ok(verdict.problems.some((p) => p.includes('schema')))
  })

  test('rejects missing required workloads (failed/missing platform runs)', () => {
    const results = validResults()
    delete results.workloads['warm-slider-1536']
    const verdict = validateResults(results)
    assert.equal(verdict.ok, false)
    assert.ok(verdict.problems.some((p) => p.includes('warm-slider-1536') && p.includes('missing')))
  })

  test('rejects failed workload outcomes as incomplete coverage', () => {
    const results = validResults('windows-test-gpu', {
      workloads: { 'cold-decode': { outcome: 'fail', error: 'decode timeout' } },
    })
    const verdict = validateResults(results)
    assert.equal(verdict.ok, false)
    assert.ok(verdict.problems.some((p) => p.includes('cold-decode') && p.includes('fail')))
  })

  test('rejects documents whose hardware identity is unnamed', () => {
    const results = validResults('windows-test-gpu', { gpu: { adapter: '', backend: 'vulkan' } })
    const verdict = validateResults(results)
    assert.equal(verdict.ok, false)
    assert.ok(verdict.problems.some((p) => p.includes('adapter')))
  })

  test('rejects unbounded navigation memory', () => {
    const results = validResults('windows-test-gpu', {
      workloads: { 'navigation-memory-bound': { outcome: 'pass', iterations: 12, bounded: false, growthBytes: 9e9 } },
    })
    const verdict = validateResults(results)
    assert.equal(verdict.ok, false)
    assert.ok(verdict.problems.some((p) => p.includes('navigation-memory-bound')))
  })

  test('warm slider p95 miss is reportable but the target stays provisional and redefinition is rejected', () => {
    // A measured miss with the provisional flag intact validates fine — the
    // gate (not the schema) turns it into a blocker.
    const missed = validResults('windows-test-gpu', {
      workloads: {
        'warm-slider-1536': {
          outcome: 'pass',
          samplesMs: [200, 210],
          p50Ms: 205,
          p95Ms: 210,
          target: { thresholdMs: 150, provisional: true, met: false },
        },
      },
    })
    assert.equal(validateResults(missed).ok, true, JSON.stringify(validateResults(missed).problems))

    // Silently redefining the threshold (moving the goalposts to make the
    // number pass) is a schema violation.
    const redefined = validResults('windows-test-gpu', {
      workloads: {
        'warm-slider-1536': {
          outcome: 'pass',
          samplesMs: [200, 210],
          p50Ms: 205,
          p95Ms: 210,
          target: { thresholdMs: 250, provisional: false, met: true },
        },
      },
    })
    const verdict = validateResults(redefined)
    assert.equal(verdict.ok, false)
    assert.ok(verdict.problems.some((p) => p.includes('provisional')))
  })

  test('unexecuted results (host not the real hardware) are rejected', () => {
    const results = validResults('windows-test-gpu', {
      executed: { host: 'ci-definition-only', startedUtc: '', finishedUtc: '', command: '' },
    })
    const verdict = validateResults(results)
    assert.equal(verdict.ok, false)
    assert.ok(verdict.problems.some((p) => p.includes('executed')))
  })
})

describe('platform release gate', () => {
  test('blocks while any declared reference machine is unavailable (explicitly untested)', () => {
    const repo = tempRepo()
    try {
      const manifestPath = path.join(repo, 'platform-manifest.json')
      writeFileSync(manifestPath, JSON.stringify(validManifest()))
      writeResults(repo, 'windows-test-gpu')
      const verdict = evaluatePlatformGate({ repoRoot: repo, manifestPath, runsDir: path.join(repo, 'runs') })
      assert.equal(verdict.status, 'blocked')
      assert.equal(verdict.exitCode, 1)
      assert.ok(
        verdict.reasons.some((r) => r.includes('linux-reference') && r.toLowerCase().includes('unavailable')),
        `expected explicit unavailable reason, got: ${verdict.reasons.join('; ')}`,
      )
      assert.ok(
        verdict.reasons.some((r) => r.includes('macos-reference')),
      )
    } finally {
      rmSync(repo, { recursive: true, force: true })
    }
  })

  test('CI workflow definitions alone never satisfy the gate', () => {
    const repo = tempRepo()
    try {
      writeFileSync(
        path.join(repo, '.github', 'workflows', 'raw-development.yml'),
        'on: workflow_dispatch\njobs: platform:\n  runs-on: ubuntu-latest\n',
      )
      const manifestPath = path.join(repo, 'platform-manifest.json')
      writeFileSync(manifestPath, JSON.stringify(validManifest()))
      const verdict = evaluatePlatformGate({ repoRoot: repo, manifestPath, runsDir: path.join(repo, 'runs') })
      assert.equal(verdict.status, 'blocked')
      assert.equal(verdict.exitCode, 1)
      assert.ok(
        verdict.reasons.some((r) => r.includes('windows-test-gpu') && r.includes('no executed results')),
        `expected missing-execution blocker, got: ${verdict.reasons.join('; ')}`,
      )
      assert.ok(!verdict.reasons.some((r) => r.toLowerCase().includes('workflow satisfied')))
    } finally {
      rmSync(repo, { recursive: true, force: true })
    }
  })

  test('blocks when an available machine has a failed workload', () => {
    const repo = tempRepo()
    try {
      const manifestPath = path.join(repo, 'platform-manifest.json')
      writeFileSync(
        manifestPath,
        JSON.stringify(validManifest([{ id: 'linux-reference', status: 'unavailable', reason: 'x' }])),
      )
      writeResults(repo, 'windows-test-gpu', {
        workloads: { 'settled-preview': { outcome: 'fail', error: 'frame timeout' } },
      })
      const verdict = evaluatePlatformGate({ repoRoot: repo, manifestPath, runsDir: path.join(repo, 'runs') })
      assert.equal(verdict.status, 'blocked')
      assert.ok(verdict.reasons.some((r) => r.includes('settled-preview')))
    } finally {
      rmSync(repo, { recursive: true, force: true })
    }
  })

  test('blocks when hardware identity of results does not match the manifest', () => {
    const repo = tempRepo()
    try {
      const manifestPath = path.join(repo, 'platform-manifest.json')
      writeFileSync(manifestPath, JSON.stringify(validManifest()))
      writeResults(repo, 'windows-test-gpu', {
        gpu: { adapter: 'Different GPU 7000', backend: 'vulkan', maxTextureDimension2d: 32768 },
      })
      const verdict = evaluatePlatformGate({ repoRoot: repo, manifestPath, runsDir: path.join(repo, 'runs') })
      assert.equal(verdict.status, 'blocked')
      assert.ok(verdict.reasons.some((r) => r.includes('hardware identity')))
    } finally {
      rmSync(repo, { recursive: true, force: true })
    }
  })

  test('fails closed (exit 2) on a malformed manifest', () => {
    const repo = tempRepo()
    try {
      const manifestPath = path.join(repo, 'platform-manifest.json')
      writeFileSync(manifestPath, '{ not json')
      const verdict = evaluatePlatformGate({ repoRoot: repo, manifestPath, runsDir: path.join(repo, 'runs') })
      assert.equal(verdict.status, 'failed')
      assert.equal(verdict.exitCode, 2)
    } finally {
      rmSync(repo, { recursive: true, force: true })
    }
  })

  test('fails closed (exit 2) on malformed results', () => {
    const repo = tempRepo()
    try {
      const manifestPath = path.join(repo, 'platform-manifest.json')
      writeFileSync(manifestPath, JSON.stringify(validManifest()))
      writeFileSync(path.join(repo, 'runs', 'platform-results-windows-test-gpu.json'), 'nope')
      const verdict = evaluatePlatformGate({ repoRoot: repo, manifestPath, runsDir: path.join(repo, 'runs') })
      assert.equal(verdict.status, 'failed')
      assert.equal(verdict.exitCode, 2)
    } finally {
      rmSync(repo, { recursive: true, force: true })
    }
  })

  test('releases only when every declared machine is available AND executed AND passing', () => {
    const repo = tempRepo()
    try {
      const manifestPath = path.join(repo, 'platform-manifest.json')
      const manifest = validManifest()
      manifest.machines = manifest.machines.map((machine) => (
        machine.id === 'linux-reference'
          ? {
              id: 'linux-reference',
              status: 'available',
              os: 'linux',
              hardware: { cpu: 'Ref CPU', ramBytes: 1 },
              gpu: { adapter: 'Ref GPU', backends: ['vulkan'] },
            }
          : machine
      ))
      manifest.machines = manifest.machines.map((machine) => (
        machine.id === 'macos-reference'
          ? {
              id: 'macos-reference',
              status: 'available',
              os: 'macos',
              hardware: { cpu: 'Ref Mac CPU', ramBytes: 1 },
              gpu: { adapter: 'Ref Mac GPU', backends: ['metal'] },
            }
          : machine
      ))
      writeFileSync(manifestPath, JSON.stringify(manifest))
      writeResults(repo, 'windows-test-gpu')
      writeResults(repo, 'linux-reference', {
        executed: { os: 'linux', host: 'linux-ref', command: 'lap-raw-platform full', startedUtc: '2026-09-28T10:00:00Z', finishedUtc: '2026-09-28T10:05:00Z', lapRevision: 'a'.repeat(40), engineRevision: 'b'.repeat(40) },
        hardware: { cpu: 'Ref CPU', ramBytes: 1 },
        gpu: { adapter: 'Ref GPU', backend: 'vulkan', maxTextureDimension2d: 8192 },
      })
      writeResults(repo, 'macos-reference', {
        executed: { os: 'macos', host: 'mac-ref', command: 'lap-raw-platform full', startedUtc: '2026-09-28T10:00:00Z', finishedUtc: '2026-09-28T10:05:00Z', lapRevision: 'a'.repeat(40), engineRevision: 'b'.repeat(40) },
        hardware: { cpu: 'Ref Mac CPU', ramBytes: 1 },
        gpu: { adapter: 'Ref Mac GPU', backend: 'metal', maxTextureDimension2d: 8192 },
      })
      const verdict = evaluatePlatformGate({ repoRoot: repo, manifestPath, runsDir: path.join(repo, 'runs') })
      assert.equal(
        verdict.status,
        'released',
        JSON.stringify({ reasons: verdict.reasons, machines: verdict.machines }, null, 2),
      )
      assert.equal(verdict.exitCode, 0)
    } finally {
      rmSync(repo, { recursive: true, force: true })
    }
  })
})
