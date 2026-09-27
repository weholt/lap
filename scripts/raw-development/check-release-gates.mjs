// Release-readiness gate for the Lap RAW-development extraction
// (lap-d7f / TASK-103; managed continuation of lap-7f5.3, spec P3).
//
// Fail-closed by construction:
//   - exit 0 ("released") requires a recorded, evidenced combined-product
//     distribution decision in docs/raw-development/provenance.json, complete
//     component provenance for every required category, preserved notices,
//     and zero unresolved unknowns.
//   - exit 1 ("blocked") is the expected state while the distribution hold
//     stands. This includes any missing approval, missing evidence entry,
//     missing component, or unresolved unknown license.
//   - exit 2 ("failed") means the inventory itself is unreadable/malformed.
//
// No environment variable, harness state, or launch signal is ever consulted:
// the `env` option exists purely so callers can prove the gate ignores it.
// Approval can only come from the distributionDecision block being explicitly
// completed by an authorized human decision (see
// docs/raw-development/release-gates.md).
//
// Usage:
//   node scripts/raw-development/check-release-gates.mjs
//   node scripts/raw-development/check-release-gates.mjs --skip-notice-checks
//   powershell -ExecutionPolicy Bypass -File scripts/raw-development/check-release-gates.ps1

import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

export const PROVENANCE_DEFAULT_PATH = 'docs/raw-development/provenance.json'

export const REQUIRED_COMPONENT_IDS = [
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
]

const APPROVAL_STATUSES = new Set(['blocked', 'approved'])

function gate(id, passed, details = []) {
  return { id, passed, details }
}

function isNonEmptyString(value) {
  return typeof value === 'string' && value.trim().length > 0
}

// A decision only counts as approval when every evidentiary field is present.
function evaluateDecision(decision) {
  const reasons = []
  if (!decision || typeof decision !== 'object') {
    reasons.push('distributionDecision block is missing')
    return { approved: false, reasons }
  }
  if (!APPROVAL_STATUSES.has(decision.status)) {
    reasons.push(
      `distributionDecision.status "${decision.status}" is not one of ${[...APPROVAL_STATUSES].join(', ')}`,
    )
  }
  if (decision.status !== 'approved') {
    reasons.push(
      'combined-product distribution decision is not recorded as approved (distribution hold in effect)',
    )
    return { approved: false, reasons }
  }

  if (!isNonEmptyString(decision.decidedBy)) {
    reasons.push('approved decision lacks decidedBy (named authority)')
  }
  if (!isNonEmptyString(decision.decidedAt) || Number.isNaN(Date.parse(decision.decidedAt))) {
    reasons.push('approved decision lacks a valid decidedAt timestamp')
  }
  if (!isNonEmptyString(decision.scope)) {
    reasons.push('approved decision lacks scope')
  }
  if (!Array.isArray(decision.evidence) || decision.evidence.length === 0) {
    reasons.push('approved decision lacks evidence entries')
  } else {
    decision.evidence.forEach((entry, index) => {
      if (
        !entry ||
        typeof entry !== 'object' ||
        !isNonEmptyString(entry.kind) ||
        !isNonEmptyString(entry.reference)
      ) {
        reasons.push(
          `evidence entry ${index} must carry non-empty kind and reference`,
        )
      }
    })
  }
  return { approved: reasons.length === 0, reasons }
}

function evaluateComponent(id, component) {
  const problems = []
  if (!component || typeof component !== 'object') {
    problems.push(`component ${id} is missing`)
    return problems
  }
  if (!isNonEmptyString(component.sourceRevision)) {
    problems.push(`component ${id} lacks a sourceRevision`)
  }
  const license = component.license
  if (!license || typeof license !== 'object') {
    problems.push(`component ${id} lacks a license block`)
    return problems
  }
  if (license.status === 'identified') {
    if (!isNonEmptyString(license.licenseId)) {
      problems.push(`component ${id} is identified but lacks licenseId`)
    }
    const hasEvidence =
      isNonEmptyString(license.evidence) ||
      (Array.isArray(license.evidence) && license.evidence.length > 0)
    if (!hasEvidence) {
      problems.push(`component ${id} is identified but lacks license evidence`)
    }
  } else if (license.status === 'unknown') {
    if (!isNonEmptyString(license.note)) {
      problems.push(`component ${id} has an unknown license without an explanatory note`)
    }
  } else {
    problems.push(
      `component ${id} license.status must be "identified" or "unknown" (got "${license.status}")`,
    )
  }
  return problems
}

function evaluateCrates(component) {
  const problems = []
  for (const crate of component.crates ?? []) {
    if (crate.licenseStatus !== 'identified' && !isNonEmptyString(crate.licenseNote)) {
      problems.push(
        `crate ${crate.name} is not identified and lacks a licenseNote`,
      )
    }
  }
  return problems
}

export function evaluateReleaseGates(options = {}) {
  const {
    repoRoot = process.cwd(),
    provenancePath = PROVENANCE_DEFAULT_PATH,
    skipNoticeFileChecks = false,
    // Deliberately unused: proves in the signature that no environment or
    // harness signal can influence the verdict.
    env = process.env,
  } = options
  void env

  const provenanceFile = path.resolve(repoRoot, provenancePath)
  let provenance
  try {
    provenance = JSON.parse(readFileSync(provenanceFile, 'utf8'))
  } catch (error) {
    return {
      status: 'failed',
      exitCode: 2,
      approved: false,
      gates: [],
      reasons: [`provenance inventory unreadable or malformed: ${error.message}`],
    }
  }

  const gates = []
  const reasons = []

  const schemaOk = provenance.schema === 'lap-raw-provenance/v1'
  gates.push(gate('schema', schemaOk, schemaOk ? [] : [
    `provenance.schema must be lap-raw-provenance/v1 (got "${provenance.schema}")`,
  ]))
  if (!schemaOk) {
    reasons.push('provenance inventory has an unsupported schema')
    return { status: 'failed', exitCode: 2, approved: false, gates, reasons }
  }

  const components = provenance.components ?? {}
  const completenessProblems = []
  for (const id of REQUIRED_COMPONENT_IDS) {
    completenessProblems.push(...evaluateComponent(id, components[id]))
  }
  completenessProblems.push(...evaluateCrates(components['extracted-engine-crates'] ?? {}))
  gates.push(gate('inventory-completeness', completenessProblems.length === 0, completenessProblems))
  reasons.push(...completenessProblems)

  const noticeProblems = []
  if (skipNoticeFileChecks) {
    // A caller-requested skip is not a notice failure; the CLI default runs
    // the real filesystem checks.
    gates.push(gate('notice-preservation', true, ['(notice file checks skipped by caller)']))
  } else {
    const repositories = provenance.repositories ?? {}
    for (const notice of provenance.notices ?? []) {
      const repository = repositories[notice.repo]
      if (!repository || !isNonEmptyString(repository.path)) {
        noticeProblems.push(`notice ${notice.path} references unknown repo "${notice.repo}"`)
        continue
      }
      const noticePath = path.resolve(repository.path, notice.path)
      if (!existsSync(noticePath)) {
        noticeProblems.push(`notice file missing: ${noticePath}`)
      }
    }
    if ((provenance.notices ?? []).length === 0) {
      noticeProblems.push('no notices recorded')
    }
  }
  if (!skipNoticeFileChecks) {
    gates.push(gate('notice-preservation', noticeProblems.length === 0, noticeProblems))
  }
  reasons.push(...noticeProblems)

  const unknownProblems = []
  for (const [id, component] of Object.entries(components)) {
    if (component && component.license && component.license.status !== 'identified') {
      unknownProblems.push(`component ${id}: license ${component.license.status}`)
    }
  }
  for (const crate of components['extracted-engine-crates']?.crates ?? []) {
    if (crate.licenseStatus !== 'identified') {
      unknownProblems.push(`crate ${crate.name}: license ${crate.licenseStatus}`)
    }
  }
  for (const question of provenance.openQuestions ?? []) {
    unknownProblems.push(`open question: ${question.id ?? question}`)
  }
  gates.push(gate('unknowns-resolved', unknownProblems.length === 0, unknownProblems))

  const { approved, reasons: decisionReasons } = evaluateDecision(
    provenance.distributionDecision,
  )
  gates.push(gate('distribution-decision', approved, decisionReasons))
  reasons.push(...decisionReasons)

  const allPassed = gates.every((g) => g.passed)
  return {
    status: allPassed ? 'released' : 'blocked',
    exitCode: allPassed ? 0 : 1,
    approved,
    gates,
    reasons,
  }
}

function parseArgs(argv) {
  const args = { skipNoticeChecks: false }
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === '--skip-notice-checks') args.skipNoticeChecks = true
  }
  return args
}

export function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv)
  const repoRoot = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    '../..',
  )
  const verdict = evaluateReleaseGates({
    repoRoot,
    skipNoticeFileChecks: args.skipNoticeChecks,
  })
  console.log(JSON.stringify(verdict, null, 2))
  if (verdict.status === 'blocked') {
    console.error(
      'RELEASE BLOCKED: the combined-product distribution hold is in effect (see docs/raw-development/release-gates.md).',
    )
  }
  return verdict.exitCode
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  process.exitCode = main()
}
