import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

import {
  ACCEPTANCE_IDS,
  PREREQUISITE_IDS,
  REQUIRED_EVIDENCE_KINDS,
  evaluateReleaseGates,
} from '../../scripts/raw-development/check-release-gates.mjs'

// Readiness-assessment gates for the final A1-A12 / P1-P7 audit
// (lap-da3 / TASK-603; managed continuation of lap-404.3).
//
// Fail-closed rules enforced here:
//   - every acceptance criterion and prerequisite must carry an explicit
//     classification (passed | failed | blocked | untested) linked to exact
//     evidence; missing platform/fixture/interactive evidence is never
//     "passed";
//   - A6 needs gpu-fixture-parity evidence, A10 needs interactive-verification
//     evidence, A12 needs platform-measurement evidence whenever a definitive
//     classification is claimed;
//   - every planned task must be recorded as executed — a skipped task rejects
//     the assessment;
//   - the machine-readable release decision may only be "ready" when every
//     mandatory evidence gate (including the provenance distribution decision
//     and unknown resolution) genuinely passes.
//
// A blocked verdict is the expected assessment result today (spec P3/P5/A12
// among others); it is not a passing release check.

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../..',
)

const READINESS_RELATIVE = 'docs/raw-development/readiness.json'

// Files outside docs/raw-development that the engine-pin consistency gate
// reads; copied into fixtures so the pin check evaluates real content.
const PIN_CONSISTENCY_FILES = [
  'src-tauri/src/develop/sessions.rs',
  'src-tauri/Cargo.lock',
]

function makeFixture(mutations = []) {
  const dir = mkdtempSync(path.join(tmpdir(), 'lap-readiness-gates-'))
  const docsDir = path.join(dir, 'docs/raw-development')
  cpSync(path.join(repoRoot, 'docs/raw-development'), docsDir, {
    recursive: true,
  })
  for (const relative of PIN_CONSISTENCY_FILES) {
    const target = path.join(dir, relative)
    mkdirSync(path.dirname(target), { recursive: true })
    cpSync(path.join(repoRoot, relative), target)
  }
  const readinessPath = path.join(dir, READINESS_RELATIVE)
  for (const mutate of mutations) {
    const readiness = JSON.parse(readFileSync(readinessPath, 'utf8'))
    mutate(readiness)
    writeFileSync(readinessPath, JSON.stringify(readiness, null, 2) + '\n')
  }
  return dir
}

function evaluateFixture(mutations = []) {
  const dir = makeFixture(mutations)
  try {
    return evaluateReleaseGates({ repoRoot: dir, env: {} })
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

function gate(verdict, id) {
  const found = verdict.gates.find((g) => g.id === id)
  expect(found, `gate ${id} must be evaluated`).toBeTruthy()
  return found
}

describe('readiness assessment structure (lap-da3)', () => {
  it('requires every A1-A12 and P1-P7 entry in the checker contract', () => {
    expect(ACCEPTANCE_IDS).toHaveLength(12)
    expect(PREREQUISITE_IDS).toHaveLength(7)
    expect(REQUIRED_EVIDENCE_KINDS).toMatchObject({
      A6: ['gpu-fixture-parity'],
      A10: ['interactive-verification'],
      A12: ['platform-measurement'],
    })
  })

  it('classifies the real evidence set honestly and keeps the release blocked', () => {
    const readiness = JSON.parse(
      readFileSync(path.join(repoRoot, READINESS_RELATIVE), 'utf8'),
    )
    expect(readiness.schema).toBe('lap-raw-readiness/v1')

    // Missing platform/fixture/interactive evidence is never "passed":
    // A10 has no human interactive verification, A12 measured a target miss.
    expect(readiness.acceptance.A10.classification).toBe('blocked')
    expect(readiness.acceptance.A12.classification).toBe('failed')

    const verdict = evaluateReleaseGates({ repoRoot, env: {} })
    expect(gate(verdict, 'readiness-completeness').passed).toBe(true)
    expect(gate(verdict, 'task-inventory').passed).toBe(true)
    expect(gate(verdict, 'engine-pin-consistency').passed).toBe(true)
    expect(gate(verdict, 'release-decision-consistency').passed).toBe(true)

    // The expected assessment result: blocked, exit 1 — not released.
    expect(verdict.status).toBe('blocked')
    expect(verdict.exitCode).toBe(1)
    expect(readiness.releaseDecision.status).toBe('blocked')
    expect(readiness.releaseDecision.publicationDisabled).toBe(true)
  })
})

describe('readiness gate rejections (fail-closed negatives)', () => {
  it('rejects an assessment omitting the A6 entry', () => {
    const verdict = evaluateFixture([
      (r) => delete r.acceptance.A6,
    ])
    expect(gate(verdict, 'readiness-completeness').passed).toBe(false)
    expect(verdict.status).not.toBe('released')
    expect(verdict.exitCode).not.toBe(0)
    expect(verdict.reasons.join('\n')).toMatch(/A6/)
  })

  it('rejects A6 claimed passed without gpu-fixture-parity evidence', () => {
    const verdict = evaluateFixture([
      (r) => {
        r.acceptance.A6.evidence = [
          { kind: 'command', reference: 'npm test' },
        ]
      },
    ])
    expect(gate(verdict, 'readiness-completeness').passed).toBe(false)
    expect(verdict.reasons.join('\n')).toMatch(/A6.*gpu-fixture-parity|gpu-fixture-parity.*A6/)
  })

  it('rejects an assessment omitting the A10 entry', () => {
    const verdict = evaluateFixture([
      (r) => delete r.acceptance.A10,
    ])
    expect(gate(verdict, 'readiness-completeness').passed).toBe(false)
    expect(verdict.exitCode).not.toBe(0)
    expect(verdict.reasons.join('\n')).toMatch(/A10/)
  })

  it('rejects A10 claimed passed without interactive-verification evidence', () => {
    const verdict = evaluateFixture([
      (r) => {
        r.acceptance.A10.classification = 'passed'
        r.acceptance.A10.evidence = [
          { kind: 'command', reference: 'npm --prefix src-vite run test' },
        ]
      },
    ])
    expect(gate(verdict, 'readiness-completeness').passed).toBe(false)
    expect(verdict.reasons.join('\n')).toMatch(
      /A10.*interactive-verification|interactive-verification.*A10/,
    )
  })

  it('rejects an assessment omitting the A12 entry', () => {
    const verdict = evaluateFixture([
      (r) => delete r.acceptance.A12,
    ])
    expect(gate(verdict, 'readiness-completeness').passed).toBe(false)
    expect(verdict.exitCode).not.toBe(0)
    expect(verdict.reasons.join('\n')).toMatch(/A12/)
  })

  it('rejects A12 claimed passed without platform-measurement evidence', () => {
    const verdict = evaluateFixture([
      (r) => {
        r.acceptance.A12.classification = 'passed'
        r.acceptance.A12.evidence = [
          { kind: 'document', reference: 'docs/raw-development/performance.md' },
        ]
      },
    ])
    expect(gate(verdict, 'readiness-completeness').passed).toBe(false)
    expect(verdict.reasons.join('\n')).toMatch(
      /A12.*platform-measurement|platform-measurement.*A12/,
    )
  })

  it('rejects a skipped task in the imported plan inventory', () => {
    const verdict = evaluateFixture([
      (r) => {
        const task = r.tasks.find((t) => t.key === 'TASK-403')
        expect(task).toBeTruthy()
        task.status = 'skipped'
      },
    ])
    expect(gate(verdict, 'task-inventory').passed).toBe(false)
    expect(verdict.exitCode).not.toBe(0)
    expect(verdict.reasons.join('\n')).toMatch(/TASK-403.*skipped|skipped/i)
  })

  it('rejects a machine-readable ready decision while the P3 distribution decision is unresolved', () => {
    const verdict = evaluateFixture([
      (r) => {
        r.releaseDecision.status = 'ready'
        r.releaseDecision.reasons = []
      },
    ])
    const consistency = gate(verdict, 'release-decision-consistency')
    expect(consistency.passed).toBe(false)
    expect(verdict.status).not.toBe('released')
    expect(verdict.exitCode).not.toBe(0)
    expect(consistency.details.join('\n')).toMatch(/distribution|P3|approved/i)
  })

  it('rejects a ready decision while blockers are recorded', () => {
    const verdict = evaluateFixture([
      (r) => {
        r.releaseDecision.status = 'ready'
        r.releaseDecision.reasons = []
      },
    ])
    expect(gate(verdict, 'release-decision-consistency').passed).toBe(false)
    expect(verdict.status).toBe('blocked')
  })

  it('rejects an assessment with an invalid classification value', () => {
    const verdict = evaluateFixture([
      (r) => {
        r.acceptance.A3.classification = 'pass'
      },
    ])
    expect(gate(verdict, 'readiness-completeness').passed).toBe(false)
    expect(verdict.reasons.join('\n')).toMatch(/A3/)
  })

  it('rejects evidence entries without kind or reference', () => {
    const verdict = evaluateFixture([
      (r) => {
        r.acceptance.A1.evidence = [{ kind: 'document' }]
      },
    ])
    expect(gate(verdict, 'readiness-completeness').passed).toBe(false)
    expect(verdict.reasons.join('\n')).toMatch(/A1/)
  })

  it('fails loudly on a malformed readiness document instead of defaulting open', () => {
    const dir = makeFixture()
    try {
      writeFileSync(path.join(dir, READINESS_RELATIVE), '{ not json', 'utf8')
      const verdict = evaluateReleaseGates({ repoRoot: dir, env: {} })
      expect(verdict.status).toBe('failed')
      expect(verdict.exitCode).toBe(2)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})

describe('machine-readable release decision (lap-da3)', () => {
  it('releases only a fully-evidenced synthetic fixture with an approved decision', () => {
    const dir = makeFixture([
      (r) => {
        for (const id of [...ACCEPTANCE_IDS, ...PREREQUISITE_IDS]) {
          const section = id.startsWith('A')
            ? r.acceptance
            : r.prerequisites
          section[id].classification = 'passed'
          section[id].evidence = section[id].evidence.filter(
            (e) => e.kind === 'gpu-fixture-parity' || e.kind === 'interactive-verification' || e.kind === 'platform-measurement',
          )
          if (section[id].evidence.length === 0) {
            section[id].evidence = [
              { kind: 'document', reference: 'docs/raw-development/spec.md' },
            ]
          }
        }
        r.acceptance.A6.evidence.push({
          kind: 'gpu-fixture-parity',
          reference: 'tests/fixtures/raw-development/baselines/capture-manifest.json',
        })
        r.acceptance.A10.evidence.push({
          kind: 'interactive-verification',
          reference: 'docs/raw-development/interactive-verification.md',
        })
        r.acceptance.A12.evidence.push({
          kind: 'platform-measurement',
          reference: 'tests/raw-development/platform/platform-manifest.json',
        })
        r.blockers = []
        r.releaseDecision = {
          status: 'ready',
          reasons: [],
          publicationDisabled: false,
        }
      },
    ])
    try {
      // Fully identify the provenance inventory and record an approved
      // (synthetic, fixture-only) distribution decision.
      const provenancePath = path.join(
        dir,
        'docs/raw-development/provenance.json',
      )
      const provenance = JSON.parse(readFileSync(provenancePath, 'utf8'))
      for (const component of Object.values(provenance.components)) {
        if (component.license.status !== 'identified') {
          component.license = {
            status: 'identified',
            licenseId: 'FIXTURE-IDENTIFIED',
            evidence: 'synthetic unit-test fixture',
          }
        }
        component.openQuestions = []
      }
      for (const crate of provenance.components['extracted-engine-crates']
        .crates ?? []) {
        crate.licenseStatus = 'identified'
        crate.licenseNote = 'synthetic unit-test fixture'
      }
      provenance.openQuestions = []
      provenance.distributionDecision = {
        status: 'approved',
        decidedBy: 'TEST-FIXTURE-ONLY (not a real decision)',
        decidedAt: '2026-09-28T00:00:00Z',
        scope: 'fixture scope only',
        evidence: [{ kind: 'fixture', reference: 'unit-test' }],
      }
      writeFileSync(
        provenancePath,
        JSON.stringify(provenance, null, 2) + '\n',
      )
      const verdict = evaluateReleaseGates({ repoRoot: dir, env: {} })
      expect(verdict.status).toBe('released')
      expect(verdict.exitCode).toBe(0)
      expect(verdict.approved).toBe(true)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})
