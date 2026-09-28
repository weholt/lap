// Platform qualification results validation and fail-closed release gate for
// the raw-development performance harness (lap-c0e / TASK-601; governing
// contract docs/raw-development/spec.md, acceptance A7/A11/A12, prerequisite
// P5).
//
// This module never manufactures platform evidence. It only decides whether
// MEASURED results exist, are schema-valid, and cover every machine the
// platform manifest declares:
//   - `validateResults(results)` checks one run document against
//     `lap-raw-platform-results/v1`, recomputes p95 from the recorded raw
//     samples, and rejects any attempt to silently redefine the provisional
//     warm-1536px target (150 ms) — a measured miss is reportable, moving the
//     goalposts is not.
//   - `evaluatePlatformGate(...)` is fail-closed by construction:
//       exit 0 ("released") requires EVERY machine declared in
//       tests/raw-development/platform/platform-manifest.json to be
//       available, executed on real hardware, schema-valid, and passing all
//       required workloads.
//       exit 1 ("blocked") is the expected state while Linux/macOS reference
//       machines are absent, any workload failed, hardware identity drifted,
//       or no executed results exist. A CI workflow definition is not
//       execution and never satisfies the gate; the gate does not even look
//       at workflow files.
//       exit 2 ("failed") means the manifest or a results file is
//       unreadable/malformed.
//
// Usage:
//   node scripts/raw-development/benchmark.mjs validate <results.json>
//   node scripts/raw-development/benchmark.mjs gate
//   node scripts/raw-development/benchmark.mjs gate --manifest <path> --runs-dir <dir>

import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

export const PLATFORM_RESULTS_SCHEMA = 'lap-raw-platform-results/v1'
export const PLATFORM_MANIFEST_SCHEMA = 'lap-raw-platform-manifest/v1'

/// Workloads every AVAILABLE machine must have executed and passed.
export const REQUIRED_WORKLOAD_IDS = [
  'cold-decode',
  'warm-slider-1536',
  'settled-preview',
  'export',
  'thumbnail-throughput-indexing',
  'navigation-memory-bound',
  'gpu-failure-modes',
]

/// Spec A12: "Provisional target: warm 1536-pixel preview response p95 below
/// 150 ms; agree final budgets after the first measured slice." The threshold
/// is frozen here; a measured failure blocks the gate and must never be
/// absorbed by redefining the number.
export const PROVISIONAL_WARM_TARGET = {
  id: 'warm-slider-1536-p95',
  workload: 'warm-slider-1536',
  metric: 'p95Ms',
  thresholdMs: 150,
  provisional: true,
}

/// Explicit GPU failure modes the platform evidence must cover (spec A7).
export const REQUIRED_GPU_FAILURE_CHECKS = [
  'missing-adapter-typed-failure',
  'texture-limit-typed-failure',
  'device-loss-injection',
  'oom-injection',
]

/// Nearest-rank percentile over raw samples (same definition the Rust
/// harness uses, so validation can recompute and compare).
export function percentile(samples, q) {
  if (!Array.isArray(samples) || samples.length === 0) return null
  const sorted = [...samples].sort((a, b) => a - b)
  const rank = Math.ceil(q * sorted.length)
  const index = Math.min(Math.max(rank - 1, 0), sorted.length - 1)
  return sorted[index]
}

function isNonEmptyString(value) {
  return typeof value === 'string' && value.trim().length > 0
}

function isFiniteNumber(value) {
  return typeof value === 'number' && Number.isFinite(value)
}

function parseIso(value) {
  return isNonEmptyString(value) && !Number.isNaN(Date.parse(value))
}

// ---------------------------------------------------------------------------
// Results document validation
// ---------------------------------------------------------------------------

export function validateResults(results) {
  const problems = []
  if (!results || typeof results !== 'object' || Array.isArray(results)) {
    return { ok: false, problems: ['results document is missing or not an object'] }
  }
  if (results.schema !== PLATFORM_RESULTS_SCHEMA) {
    problems.push(
      `schema must be ${PLATFORM_RESULTS_SCHEMA} (got "${results.schema}")`,
    )
  }
  if (!isNonEmptyString(results.machineId)) {
    problems.push('machineId is missing')
  }

  // Executed-on-real-hardware evidence. Anything that smells like a
  // definition-only artifact fails here: definitions carry no timestamps,
  // no command, and no execution host.
  const executed = results.executed
  if (!executed || typeof executed !== 'object') {
    problems.push('executed block is missing')
  } else {
    if (!isNonEmptyString(executed.host)) problems.push('executed.host (named execution host) is missing')
    if (!isNonEmptyString(executed.command)) problems.push('executed.command is missing')
    if (!parseIso(executed.startedUtc)) problems.push('executed.startedUtc is missing or not ISO-8601')
    if (!parseIso(executed.finishedUtc)) problems.push('executed.finishedUtc is missing or not ISO-8601')
    if (parseIso(executed.startedUtc) && parseIso(executed.finishedUtc)
      && Date.parse(executed.finishedUtc) < Date.parse(executed.startedUtc)) {
      problems.push('executed.finishedUtc precedes executed.startedUtc')
    }
    if (!isNonEmptyString(executed.lapRevision) || !/^[0-9a-f]{40}$/.test(executed.lapRevision)) {
      problems.push('executed.lapRevision must be a 40-hex git revision')
    }
    if (!isNonEmptyString(executed.engineRevision) || !/^[0-9a-f]{40}$/.test(executed.engineRevision)) {
      problems.push('executed.engineRevision must be a 40-hex git revision')
    }
  }

  // Named hardware/backend (spec A12: "named reference hardware").
  const hardware = results.hardware
  if (!hardware || typeof hardware !== 'object' || !isNonEmptyString(hardware.cpu)) {
    problems.push('hardware.cpu (named CPU) is missing')
  } else if (!isFiniteNumber(hardware.ramBytes) || hardware.ramBytes <= 0) {
    problems.push('hardware.ramBytes must be a positive number')
  }
  const gpu = results.gpu
  if (!gpu || typeof gpu !== 'object') {
    problems.push('gpu block is missing')
  } else {
    if (!isNonEmptyString(gpu.adapter)) problems.push('gpu.adapter (named GPU) is missing')
    if (!isNonEmptyString(gpu.backend)) problems.push('gpu.backend is missing')
    if (!isFiniteNumber(gpu.maxTextureDimension2d) || gpu.maxTextureDimension2d <= 0) {
      problems.push('gpu.maxTextureDimension2d must be a positive number')
    }
  }

  // Fixture revisions (spec A12: measurements bound to fixture revisions).
  if (!Array.isArray(results.fixtures) || results.fixtures.length === 0) {
    problems.push('fixtures must be a non-empty array with sha256 revisions')
  } else {
    results.fixtures.forEach((fixture, index) => {
      if (!fixture || typeof fixture !== 'object') {
        problems.push(`fixtures[${index}] is missing`)
        return
      }
      if (!isNonEmptyString(fixture.path)) problems.push(`fixtures[${index}].path is missing`)
      if (!isNonEmptyString(fixture.sha256) || !/^[0-9a-f]{64}$/.test(fixture.sha256)) {
        problems.push(`fixtures[${index}].sha256 must be a 64-hex digest`)
      }
      if (!isFiniteNumber(fixture.bytes) || fixture.bytes <= 0) {
        problems.push(`fixtures[${index}].bytes must be positive`)
      }
    })
  }

  // Workloads.
  const workloads = results.workloads
  if (!workloads || typeof workloads !== 'object') {
    problems.push('workloads block is missing')
    return { ok: problems.length === 0, problems }
  }
  for (const id of REQUIRED_WORKLOAD_IDS) {
    const workload = workloads[id]
    if (!workload || typeof workload !== 'object') {
      problems.push(`workload ${id} is missing (platform coverage incomplete)`)
      continue
    }
    if (workload.outcome === 'fail') {
      problems.push(`workload ${id} failed on this platform: ${workload.error ?? 'no error detail recorded'}`)
      continue
    }
    if (workload.outcome !== 'pass') {
      problems.push(`workload ${id} outcome must be "pass" or "fail" (got "${workload.outcome}")`)
      continue
    }

    if (id === 'thumbnail-throughput-indexing') {
      if (!isFiniteNumber(workload.thumbnails) || workload.thumbnails <= 0) {
        problems.push(`workload ${id} lacks a positive thumbnails count`)
      }
      if (!isFiniteNumber(workload.perSecond) || workload.perSecond <= 0) {
        problems.push(`workload ${id} lacks a positive perSecond throughput`)
      }
      if (!Array.isArray(workload.memorySeries) || workload.memorySeries.length === 0) {
        problems.push(`workload ${id} lacks memorySeries (memory under indexing load)`)
      }
      continue
    }

    if (id === 'navigation-memory-bound') {
      if (!isFiniteNumber(workload.iterations) || workload.iterations <= 0) {
        problems.push(`workload ${id} lacks a positive iteration count`)
      }
      if (!Array.isArray(workload.memorySeries) || workload.memorySeries.length < 2) {
        problems.push(`workload ${id} lacks a memorySeries with at least two checkpoints`)
      }
      if (workload.bounded !== true) {
        problems.push(
          `workload ${id} reports UNBOUNDED navigation memory (growth ${workload.growthBytes} bytes)`,
        )
      }
      continue
    }

    if (id === 'gpu-failure-modes') {
      const checks = Array.isArray(workload.checks) ? workload.checks : []
      for (const checkId of REQUIRED_GPU_FAILURE_CHECKS) {
        const check = checks.find((entry) => entry && entry.id === checkId)
        if (!check) {
          problems.push(`workload ${id} lacks the "${checkId}" check`)
        } else if (check.ok !== true) {
          problems.push(`workload ${id} check "${checkId}" did not pass: ${check.detail ?? ''}`)
        }
      }
      continue
    }

    // Timing workloads: raw samples must exist and the reported p95 must be
    // the computed nearest-rank p95 OF THOSE SAMPLES (no invented numbers).
    if (!Array.isArray(workload.samplesMs) || workload.samplesMs.length === 0
      || !workload.samplesMs.every(isFiniteNumber)) {
      problems.push(`workload ${id} lacks raw samplesMs measurements`)
      continue
    }
    const recomputed = percentile(workload.samplesMs, 0.95)
    if (!isFiniteNumber(workload.p95Ms) || Math.abs(workload.p95Ms - recomputed) > 0.5) {
      problems.push(
        `workload ${id} p95Ms (${workload.p95Ms}) does not match the nearest-rank p95 of the raw samples (${recomputed})`,
      )
    }

    if (id === PROVISIONAL_WARM_TARGET.workload) {
      const target = workload.target
      if (!target || typeof target !== 'object') {
        problems.push(`workload ${id} lacks the provisional target block`)
      } else {
        if (target.thresholdMs !== PROVISIONAL_WARM_TARGET.thresholdMs) {
          problems.push(
            `workload ${id} redefined the provisional target threshold (${target.thresholdMs} != ${PROVISIONAL_WARM_TARGET.thresholdMs} ms); measured misses must stay visible, the target must not move`,
          )
        }
        if (target.provisional !== true) {
          problems.push(
            `workload ${id} dropped the provisional flag from the warm preview target`,
          )
        }
        const met = isFiniteNumber(workload.p95Ms) && workload.p95Ms < PROVISIONAL_WARM_TARGET.thresholdMs
        if (target.met !== met) {
          problems.push(
            `workload ${id} target.met (${target.met}) contradicts the measured p95 (${workload.p95Ms} ms vs threshold ${PROVISIONAL_WARM_TARGET.thresholdMs} ms)`,
          )
        }
      }
    }
  }

  return { ok: problems.length === 0, problems }
}

// ---------------------------------------------------------------------------
// Platform release gate
// ---------------------------------------------------------------------------

export function evaluatePlatformGate(options = {}) {
  const {
    repoRoot = process.cwd(),
    manifestPath = path.join(repoRoot, 'tests/raw-development/platform/platform-manifest.json'),
    runsDir = path.join(repoRoot, 'tests/raw-development/platform/runs'),
  } = options

  let manifest
  try {
    manifest = JSON.parse(readFileSync(path.resolve(manifestPath), 'utf8'))
  } catch (error) {
    return {
      status: 'failed',
      exitCode: 2,
      gates: [],
      reasons: [`platform manifest unreadable or malformed: ${error.message}`],
    }
  }
  if (manifest.schema !== PLATFORM_MANIFEST_SCHEMA) {
    return {
      status: 'failed',
      exitCode: 2,
      gates: [],
      reasons: [
        `platform manifest schema must be ${PLATFORM_MANIFEST_SCHEMA} (got "${manifest.schema}")`,
      ],
    }
  }
  if (!Array.isArray(manifest.machines) || manifest.machines.length === 0) {
    return {
      status: 'failed',
      exitCode: 2,
      gates: [],
      reasons: ['platform manifest declares no machines'],
    }
  }

  const reasons = []
  const gates = []
  const machineVerdicts = []
  let malformed = false

  for (const machine of manifest.machines) {
    const id = machine?.id
    if (!isNonEmptyString(id) || !['available', 'unavailable'].includes(machine.status)) {
      malformed = true
      reasons.push(`manifest machine entry is invalid (id "${id}", status "${machine?.status}")`)
      continue
    }

    if (machine.status === 'unavailable') {
      // Explicit, honest blocker: this platform was NOT tested. It is never
      // silently skipped and never satisfied by a CI definition.
      const detail = isNonEmptyString(machine.reason) ? machine.reason : 'no reason recorded'
      reasons.push(
        `machine ${id} is explicitly unavailable and therefore UNTESTED for release: ${detail}`,
      )
      machineVerdicts.push({ id, status: 'untested' })
      gates.push({ id: `machine:${id}`, passed: false, details: [`unavailable: ${detail}`] })
      continue
    }

    const resultsPath = path.join(path.resolve(runsDir), `platform-results-${id}.json`)
    let results
    try {
      results = JSON.parse(readFileSync(resultsPath, 'utf8'))
    } catch (error) {
      if (error.code === 'ENOENT') {
        reasons.push(
          `machine ${id}: no executed results found at ${path.relative(path.resolve(repoRoot), resultsPath).replaceAll('\\', '/')} — an executed benchmark/stress run is required; a CI workflow definition alone is not platform evidence`,
        )
      } else {
        malformed = true
        reasons.push(`machine ${id}: results file unreadable or malformed: ${error.message}`)
      }
      machineVerdicts.push({ id, status: 'missing-results' })
      gates.push({ id: `machine:${id}`, passed: false, details: ['no valid executed results'] })
      continue
    }

    const validation = validateResults(results)
    if (results.machineId !== id) {
      validation.ok = false
      validation.problems.push(`results document machineId "${results.machineId}" does not match manifest machine "${id}"`)
    }
    // Hardware identity binding: the results must come from the machine the
    // manifest names (adapter + backend family + OS).
    if (isNonEmptyString(machine.os) && results.executed?.os !== machine.os) {
      validation.ok = false
      validation.problems.push(
        `results executed.os "${results.executed?.os}" does not match manifest machine os "${machine.os}"`,
      )
    }
    if (machine.gpu && isNonEmptyString(machine.gpu.adapter) && results.gpu?.adapter !== machine.gpu.adapter) {
      validation.ok = false
      validation.problems.push(
        `results hardware identity mismatch: gpu.adapter "${results.gpu?.adapter}" != manifest "${machine.gpu.adapter}"`,
      )
    }
    if (machine.gpu && Array.isArray(machine.gpu.backends) && !machine.gpu.backends.includes(results.gpu?.backend)) {
      validation.ok = false
      validation.problems.push(
        `results gpu.backend "${results.gpu?.backend}" is not a declared backend of machine ${id} (${machine.gpu.backends.join(', ')})`,
      )
    }

    const warm = results.workloads?.[PROVISIONAL_WARM_TARGET.workload]
    if (warm?.outcome === 'pass' && isFiniteNumber(warm.p95Ms)
      && warm.p95Ms >= PROVISIONAL_WARM_TARGET.thresholdMs) {
      validation.ok = false
      validation.problems.push(
        `provisional target MISSED on ${id}: warm 1536px preview p95 ${warm.p95Ms} ms >= ${PROVISIONAL_WARM_TARGET.thresholdMs} ms (target unchanged and still provisional; this is a release blocker until met or a human re-baselines the budget)`,
      )
    }

    gates.push({
      id: `machine:${id}`,
      passed: validation.ok,
      details: validation.problems,
    })
    machineVerdicts.push({
      id,
      status: validation.ok ? 'qualified' : 'blocked',
      problems: validation.problems,
    })
    if (!validation.ok) {
      reasons.push(
        ...validation.problems.map((problem) => `machine ${id}: ${problem}`),
      )
    }
  }

  if (malformed) {
    return { status: 'failed', exitCode: 2, gates, reasons, machines: machineVerdicts }
  }

  const allPassed = gates.length > 0 && gates.every((gate) => gate.passed)
  return {
    status: allPassed ? 'released' : 'blocked',
    exitCode: allPassed ? 0 : 1,
    gates,
    reasons,
    machines: machineVerdicts,
  }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const args = { mode: 'gate', validatePath: null }
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === 'validate') {
      args.mode = 'validate'
      args.validatePath = argv[i + 1] ?? null
      i += 1
    } else if (argv[i] === 'gate') {
      args.mode = 'gate'
    } else if (argv[i] === '--manifest') {
      args.manifestPath = argv[i + 1]
      i += 1
    } else if (argv[i] === '--runs-dir') {
      args.runsDir = argv[i + 1]
      i += 1
    }
  }
  return args
}

export function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv)
  if (args.mode === 'validate') {
    if (!args.validatePath) {
      console.error('usage: benchmark.mjs validate <results.json>')
      return 2
    }
    let results
    try {
      results = JSON.parse(readFileSync(path.resolve(args.validatePath), 'utf8'))
    } catch (error) {
      console.log(JSON.stringify({ ok: false, problems: [`unreadable or malformed: ${error.message}`] }, null, 2))
      return 2
    }
    const verdict = validateResults(results)
    console.log(JSON.stringify(verdict, null, 2))
    return verdict.ok ? 0 : 1
  }

  const verdict = evaluatePlatformGate({
    repoRoot: path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..'),
    manifestPath: args.manifestPath,
    runsDir: args.runsDir,
  })
  console.log(JSON.stringify(verdict, null, 2))
  if (verdict.status === 'blocked') {
    console.error(
      'PLATFORM RELEASE BLOCKED: platform qualification coverage is incomplete or a measurement missed its target (see docs/raw-development/performance.md).',
    )
  }
  return verdict.exitCode
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  process.exitCode = main()
}
