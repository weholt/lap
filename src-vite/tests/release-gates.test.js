import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

import {
  REQUIRED_COMPONENT_IDS,
  evaluateReleaseGates,
} from '../../scripts/raw-development/check-release-gates.mjs'

// Release-gate regressions for the provenance inventory and the distribution
// hold (lap-d7f / TASK-103; managed continuation of lap-7f5.3, spec P3/A11).
//
// The gate is fail-closed: a release verdict ("released", exit 0) requires a
// recorded, evidenced combined-product distribution decision AND complete
// component provenance AND no unresolved unknowns. Missing anything must
// reject. Harness launches and environment variables must never be treated as
// approval evidence.

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../..',
)

const PROVENANCE_RELATIVE = 'docs/raw-development/provenance.json'

function loadProvenance(root = repoRoot) {
  return JSON.parse(readFileSync(path.join(root, PROVENANCE_RELATIVE), 'utf8'))
}

// Build an isolated copy of the provenance file so mutations in tests never
// touch the repository's real (blocked) inventory.
function withProvenanceFixture(mutate) {
  const dir = mkdtempSync(path.join(tmpdir(), 'lap-release-gates-'))
  const docsDir = path.join(dir, 'docs/raw-development')
  try {
    cpSync(path.join(repoRoot, 'docs/raw-development'), docsDir, {
      recursive: true,
    })
    const provenance = loadProvenance(dir)
    mutate(provenance)
    writeProvenance(dir, provenance)
    return evaluateReleaseGates({
      repoRoot: dir,
      provenancePath: PROVENANCE_RELATIVE,
      // Fixture tests only exercise inventory/decision logic; notice files
      // outside the copied docs tree are not stat'ed for these runs.
      skipNoticeFileChecks: true,
      env: {},
    })
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

function writeProvenance(root, provenance) {
  writeFileSync(
    path.join(root, PROVENANCE_RELATIVE),
    JSON.stringify(provenance, null, 2) + '\n',
    'utf8',
  )
}

describe('provenance inventory completeness', () => {
  it('requires every component category named by the acceptance criteria', () => {
    expect(REQUIRED_COMPONENT_IDS).toEqual(
      expect.arrayContaining([
        'rapidraw-agpl-source',
        'lap-gpl-application',
        'extracted-engine-crates',
        'wgsl-shaders',
        'rawler-fork',
        'film-luts',
        'lens-data',
        'lap-native-submodules',
        'optional-ai-models',
        'optional-ffmpeg-sidecar',
      ]),
    )
  })

  it('ships a machine-readable inventory identifying every required component', () => {
    const provenance = loadProvenance()
    expect(provenance.schema).toBe('lap-raw-provenance/v1')

    const verdict = evaluateReleaseGates({
      repoRoot,
      provenancePath: PROVENANCE_RELATIVE,
      skipNoticeFileChecks: true,
      env: {},
    })
    const completeness = verdict.gates.find(
      (g) => g.id === 'inventory-completeness',
    )
    expect(completeness.passed).toBe(true)

    const components = provenance.components
    for (const id of REQUIRED_COMPONENT_IDS) {
      expect(components[id], `missing component ${id}`).toBeTruthy()
      expect(
        components[id].sourceRevision,
        `component ${id} lacks a source revision`,
      ).toBeTruthy()
    }

    // Licenses the acceptance criteria call out by name.
    expect(components['rapidraw-agpl-source'].license.licenseId).toBe(
      'AGPL-3.0',
    )
    expect(components['lap-gpl-application'].license.licenseId).toBe(
      'GPL-3.0-or-later',
    )
    expect(components['rawler-fork'].license.licenseId).toBe('LGPL-2.1')
    expect(components['film-luts'].license.licenseId).toBe('CC-BY-SA-4.0')

    // Unknowns must be explicit, never silently claimed.
    expect(Array.isArray(provenance.openQuestions)).toBe(true)
    expect(provenance.openQuestions.length).toBeGreaterThan(0)
    for (const [id, component] of Object.entries(components)) {
      if (component.license.status !== 'identified') {
        expect(component.license.status, id).toBe('unknown')
        expect(
          component.license.note,
          `unknown license for ${id} lacks a note`,
        ).toBeTruthy()
      }
    }
  })

  it('records existing license notices and keeps the extraction unasserted', () => {
    const provenance = loadProvenance()
    // The extracted crates deliberately assert no license field pending the
    // combined-product decision; the inventory must say so instead of
    // claiming a license for them.
    for (const crate of provenance.components['extracted-engine-crates'].crates) {
      expect(crate.licenseStatus).toBe('unknown')
      expect(crate.licenseNote).toMatch(/no license field is asserted/i)
    }
    expect(
      provenance.notices.some((n) => n.path.endsWith('SPEKTRAFILM_LICENSE.txt')),
    ).toBe(true)
    expect(provenance.notices.some((n) => n.path.endsWith('AGPL-3.0.txt'))).toBe(
      true,
    )
  })
})

describe('distribution hold (fail-closed release gate)', () => {
  it('rejects the real repository state while the combined-product decision is unresolved', () => {
    const provenance = loadProvenance()
    expect(provenance.distributionDecision.status).toBe('blocked')

    const verdict = evaluateReleaseGates({
      repoRoot,
      provenancePath: PROVENANCE_RELATIVE,
      skipNoticeFileChecks: true,
      env: {},
    })
    expect(verdict.status).toBe('blocked')
    expect(verdict.exitCode).not.toBe(0)
    expect(verdict.approved).toBe(false)
    expect(verdict.reasons.join('\n')).toMatch(/distribution/i)
  })

  it('never infers approval from the environment or harness launch', () => {
    const verdict = evaluateReleaseGates({
      repoRoot,
      provenancePath: PROVENANCE_RELATIVE,
      skipNoticeFileChecks: true,
      // A hostile/ignorant agent might export something like this; the gate
      // must ignore environment signals entirely.
      env: { LAP_DISTRIBUTION_APPROVED: '1', WDL_HARNESS_LAUNCHED: 'true' },
    })
    expect(verdict.approved).toBe(false)
    expect(verdict.status).toBe('blocked')
  })

  it('rejects an approval entry that lacks evidence', () => {
    const verdict = withProvenanceFixture((provenance) => {
      provenance.distributionDecision = {
        status: 'approved',
        decidedBy: null,
        decidedAt: null,
        scope: null,
        evidence: [],
      }
    })
    expect(verdict.status).toBe('blocked')
    expect(verdict.approved).toBe(false)
    expect(verdict.reasons.join('\n')).toMatch(/evidence|decidedBy|decidedAt/i)
  })

  it('rejects absent resource provenance', () => {
    const verdict = withProvenanceFixture((provenance) => {
      delete provenance.components['film-luts']
    })
    const completeness = verdict.gates.find(
      (g) => g.id === 'inventory-completeness',
    )
    expect(completeness.passed).toBe(false)
    expect(verdict.status).not.toBe('released')
    expect(verdict.exitCode).not.toBe(0)
  })

  it('rejects release while component licenses remain unknown, even with an approval', () => {
    const verdict = withProvenanceFixture((provenance) => {
      provenance.distributionDecision = {
        status: 'approved',
        decidedBy: 'TEST-FIXTURE-ONLY (not a real decision)',
        decidedAt: '2026-09-27T00:00:00Z',
        scope: 'fixture scope only',
        evidence: [{ kind: 'fixture', reference: 'unit-test' }],
      }
    })
    expect(verdict.status).toBe('blocked')
    expect(
      verdict.gates.find((g) => g.id === 'unknowns-resolved').passed,
    ).toBe(false)
  })

  it('releases only a fully-identified synthetic fixture with an evidenced decision', () => {
    const verdict = withProvenanceFixture((provenance) => {
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
      const crates =
        provenance.components['extracted-engine-crates'].crates || []
      for (const crate of crates) {
        crate.licenseStatus = 'identified'
        crate.licenseNote = 'synthetic unit-test fixture'
      }
      provenance.openQuestions = []
      provenance.distributionDecision = {
        status: 'approved',
        decidedBy: 'TEST-FIXTURE-ONLY (not a real decision)',
        decidedAt: '2026-09-27T00:00:00Z',
        scope: 'fixture scope only',
        evidence: [{ kind: 'fixture', reference: 'unit-test' }],
      }
    })
    expect(verdict.status).toBe('released')
    expect(verdict.exitCode).toBe(0)
    expect(verdict.approved).toBe(true)
  })

  it('fails loudly on a malformed inventory instead of defaulting open', () => {
    const dir = mkdtempSync(path.join(tmpdir(), 'lap-release-gates-bad-'))
    try {
      const docsDir = path.join(dir, 'docs/raw-development')
      cpSync(path.join(repoRoot, 'docs/raw-development'), docsDir, {
        recursive: true,
      })
      writeFileSync(path.join(dir, PROVENANCE_RELATIVE), '{ not json', 'utf8')
      const verdict = evaluateReleaseGates({
        repoRoot: dir,
        provenancePath: PROVENANCE_RELATIVE,
        skipNoticeFileChecks: true,
        env: {},
      })
      expect(verdict.status).toBe('failed')
      expect(verdict.exitCode).toBe(2)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})
