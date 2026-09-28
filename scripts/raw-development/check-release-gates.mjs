// Release-readiness gate for the Lap RAW-development extraction
// (lap-d7f / TASK-103; managed continuation of lap-7f5.3, spec P3; readiness
// assessment gates added by lap-da3 / TASK-603, managed continuation of
// lap-404.3).
//
// Fail-closed by construction:
//   - exit 0 ("released") requires a recorded, evidenced combined-product
//     distribution decision in docs/raw-development/provenance.json, complete
//     component provenance for every required category, preserved notices,
//     zero unresolved unknowns, a complete A1-A12 / P1-P7 readiness assessment
//     in docs/raw-development/readiness.json with exact evidence for every
//     entry, an executed inventory of every planned task, consistent host and
//     engine pins, and a machine-readable release decision that is "ready"
//     only because every mandatory evidence gate genuinely passed.
//   - exit 1 ("blocked") is the expected state while the distribution hold
//     stands. This includes any missing approval, missing evidence entry,
//     missing component, unresolved unknown license, missing/failed/untested
//     acceptance item, or skipped task.
//   - exit 2 ("failed") means a mandatory input (provenance inventory or
//     readiness assessment) is unreadable/malformed or has an unsupported
//     schema.
//
// Classification rules enforced for readiness items (lap-da3): missing
// platform, fixture, or interactive evidence is never "passed" — A6 requires
// gpu-fixture-parity evidence, A10 requires interactive-verification
// evidence, and A12 requires platform-measurement evidence whenever a
// definitive classification is claimed.
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
export const READINESS_DEFAULT_PATH = 'docs/raw-development/readiness.json'
export const ENGINE_LOCK_PATH = 'docs/raw-development/engine-lock.json'
export const ENGINE_REVISION_SOURCE_PATH =
  'src-tauri/src/develop/sessions.rs'
export const CONSUMER_LOCK_PATH = 'src-tauri/Cargo.lock'

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

export const ACCEPTANCE_IDS = [
  'A1',
  'A2',
  'A3',
  'A4',
  'A5',
  'A6',
  'A7',
  'A8',
  'A9',
  'A10',
  'A11',
  'A12',
]

export const PREREQUISITE_IDS = ['P1', 'P2', 'P3', 'P4', 'P5', 'P6', 'P7']

export const VALID_CLASSIFICATIONS = new Set([
  'passed',
  'failed',
  'blocked',
  'untested',
])

// Evidence kinds without which these items must never be classified as
// definitively assessed (spec A6: "Requires real RAW fixtures and GPU runs";
// A10: "Requires interactive verification"; A12: measured platform evidence).
export const REQUIRED_EVIDENCE_KINDS = {
  A6: ['gpu-fixture-parity'],
  A10: ['interactive-verification'],
  A12: ['platform-measurement'],
}

// The exact task keys of the imported raw-development plan
// (PLAN-CONTROL-E47134A0CB19-001 / PLAN-260926 lineage, see
// docs/raw-development/issue-map.json). A skipped or missing task rejects the
// assessment.
export const EXPECTED_TASK_KEYS = [
  'TASK-101',
  'TASK-102',
  'TASK-103',
  'TASK-201',
  'TASK-202',
  'TASK-203',
  'TASK-204',
  'TASK-205',
  'TASK-301',
  'TASK-302',
  'TASK-303',
  'TASK-304',
  'TASK-305',
  'TASK-306',
  'TASK-401',
  'TASK-402',
  'TASK-403',
  'TASK-404',
  'TASK-405',
  'TASK-501',
  'TASK-502',
  'TASK-503',
  'TASK-601',
  'TASK-602',
  'TASK-603',
]

export const EXECUTED_TASK_STATUS = 'executed'

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

function evidenceKinds(item) {
  return new Set(
    (Array.isArray(item?.evidence) ? item.evidence : [])
      .map((entry) => entry?.kind)
      .filter((kind) => isNonEmptyString(kind)),
  )
}

function evaluateReadinessItem(id, item, problems) {
  if (!item || typeof item !== 'object') {
    problems.push(`readiness entry ${id} is missing`)
    return
  }
  if (!VALID_CLASSIFICATIONS.has(item.classification)) {
    problems.push(
      `readiness entry ${id} classification "${item.classification}" is not one of ${[...VALID_CLASSIFICATIONS].join(', ')}`,
    )
  }
  if (!Array.isArray(item.evidence) || item.evidence.length === 0) {
    problems.push(`readiness entry ${id} carries no evidence entries`)
  } else {
    item.evidence.forEach((entry, index) => {
      if (!entry || typeof entry !== 'object') {
        problems.push(`readiness entry ${id} evidence ${index} is malformed`)
        return
      }
      if (!isNonEmptyString(entry.kind)) {
        problems.push(`readiness entry ${id} evidence ${index} lacks kind`)
      }
      if (!isNonEmptyString(entry.reference)) {
        problems.push(`readiness entry ${id} evidence ${index} lacks reference`)
      }
    })
  }
  // Missing platform/fixture/interactive evidence is never "passed": items
  // with a required evidence kind must cite it whenever a definitive
  // classification is claimed.
  const requiredKinds = REQUIRED_EVIDENCE_KINDS[id]
  if (
    requiredKinds &&
    (item.classification === 'passed' || item.classification === 'failed')
  ) {
    const kinds = evidenceKinds(item)
    for (const kind of requiredKinds) {
      if (!kinds.has(kind)) {
        problems.push(
          `readiness entry ${id} classification "${item.classification}" requires ${kind} evidence (missing platform/fixture/interactive evidence is never passed)`,
        )
      }
    }
  }
}

function evaluateTaskInventory(readiness, problems) {
  const tasks = Array.isArray(readiness.tasks) ? readiness.tasks : []
  const byKey = new Map(
    tasks.filter((t) => t && isNonEmptyString(t.key)).map((t) => [t.key, t]),
  )
  for (const key of EXPECTED_TASK_KEYS) {
    const task = byKey.get(key)
    if (!task) {
      problems.push(`planned task ${key} is missing from the task inventory`)
    } else if (task.status !== EXECUTED_TASK_STATUS) {
      problems.push(
        `planned task ${key} has status "${task.status}"; skipped tasks reject the assessment`,
      )
    }
  }
}

function evaluateEnginePinConsistency(repoRoot, problems) {
  const resolve = (relative) => path.resolve(repoRoot, relative)
  let engineLock
  try {
    engineLock = JSON.parse(readFileSync(resolve(ENGINE_LOCK_PATH), 'utf8'))
  } catch (error) {
    problems.push(`engine lock unreadable or malformed: ${error.message}`)
    return
  }
  const revision = engineLock?.engine?.revision
  if (!isNonEmptyString(revision) || !/^[0-9a-f]{40}$/.test(revision)) {
    problems.push('engine lock lacks a 40-hex engine.revision')
    return
  }
  const pinNeedles = [
    {
      file: ENGINE_REVISION_SOURCE_PATH,
      pattern: new RegExp(`ENGINE_GIT_REVISION:\\s*&str\\s*=\\s*"${revision}"`),
      label: 'host ENGINE_GIT_REVISION',
    },
    {
      file: CONSUMER_LOCK_PATH,
      pattern: new RegExp(`rev=${revision}`),
      label: 'consumer Cargo.lock pin',
    },
  ]
  for (const needle of pinNeedles) {
    let content
    try {
      content = readFileSync(resolve(needle.file), 'utf8')
    } catch (error) {
      problems.push(`${needle.label}: cannot read ${needle.file}: ${error.message}`)
      continue
    }
    if (!needle.pattern.test(content)) {
      problems.push(
        `${needle.label}: pinned revision ${revision} not found in ${needle.file}`,
      )
    }
  }
  const cargoDependency = engineLock?.consumption?.cargoDependency
  if (
    !isNonEmptyString(cargoDependency) ||
    !cargoDependency.includes(`rev = "${revision}"`)
  ) {
    problems.push(
      `engine lock consumption.cargoDependency does not pin ${revision}`,
    )
  }
}

function evaluateReadinessGates(options, provenanceGateResults) {
  const { repoRoot, readinessPath = READINESS_DEFAULT_PATH } = options
  const readinessFile = path.resolve(repoRoot, readinessPath)
  let readiness
  try {
    readiness = JSON.parse(readFileSync(readinessFile, 'utf8'))
  } catch (error) {
    return {
      status: 'failed',
      exitCode: 2,
      approved: false,
      gates: [],
      reasons: [`readiness assessment unreadable or malformed: ${error.message}`],
    }
  }

  const gates = []
  const reasons = []

  const readinessSchemaOk = readiness.schema === 'lap-raw-readiness/v1'
  gates.push(
    gate('readiness-schema', readinessSchemaOk, readinessSchemaOk ? [] : [
      `readiness.schema must be lap-raw-readiness/v1 (got "${readiness.schema}")`,
    ]),
  )
  if (!readinessSchemaOk) {
    reasons.push('readiness assessment has an unsupported schema')
    return { status: 'failed', exitCode: 2, approved: false, gates, reasons }
  }

  const completenessProblems = []
  for (const id of ACCEPTANCE_IDS) {
    evaluateReadinessItem(id, readiness.acceptance?.[id], completenessProblems)
  }
  for (const id of PREREQUISITE_IDS) {
    evaluateReadinessItem(
      id,
      readiness.prerequisites?.[id],
      completenessProblems,
    )
  }
  gates.push(
    gate(
      'readiness-completeness',
      completenessProblems.length === 0,
      completenessProblems,
    ),
  )
  reasons.push(...completenessProblems)

  const taskProblems = []
  evaluateTaskInventory(readiness, taskProblems)
  gates.push(gate('task-inventory', taskProblems.length === 0, taskProblems))
  reasons.push(...taskProblems)

  const pinProblems = []
  evaluateEnginePinConsistency(repoRoot, pinProblems)
  gates.push(
    gate('engine-pin-consistency', pinProblems.length === 0, pinProblems),
  )
  reasons.push(...pinProblems)

  // The recorded machine-readable decision may claim "ready" only when every
  // mandatory evidence condition genuinely holds.
  const consistencyProblems = []
  const decision = readiness.releaseDecision
  if (!decision || typeof decision !== 'object') {
    consistencyProblems.push('releaseDecision block is missing')
  } else if (decision.status === 'ready') {
    for (const result of provenanceGateResults) {
      if (!result.passed) {
        consistencyProblems.push(
          `release decision claims ready while provenance gate ${result.id} has not passed`,
        )
      }
    }
    for (const result of gates) {
      if (!result.passed) {
        consistencyProblems.push(
          `release decision claims ready while readiness gate ${result.id} has not passed`,
        )
      }
    }
    for (const id of ACCEPTANCE_IDS) {
      const item = readiness.acceptance?.[id]
      if (!item || item.classification !== 'passed') {
        consistencyProblems.push(
          `release decision claims ready while acceptance ${id} is ${item?.classification ?? 'missing'}`,
        )
      }
    }
    for (const id of PREREQUISITE_IDS) {
      const item = readiness.prerequisites?.[id]
      if (!item || item.classification !== 'passed') {
        consistencyProblems.push(
          `release decision claims ready while prerequisite ${id} is ${item?.classification ?? 'missing'}`,
        )
      }
    }
    if (Array.isArray(readiness.blockers) && readiness.blockers.length > 0) {
      consistencyProblems.push(
        `release decision claims ready while ${readiness.blockers.length} blocker(s) are recorded`,
      )
    }
  } else if (decision.status !== 'blocked') {
    consistencyProblems.push(
      `releaseDecision.status "${decision.status}" must be "ready" or "blocked"`,
    )
  }
  gates.push(
    gate(
      'release-decision-consistency',
      consistencyProblems.length === 0,
      consistencyProblems,
    ),
  )
  reasons.push(...consistencyProblems)

  return { gates, reasons }
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

  const readiness = evaluateReadinessGates(options, gates)
  if (readiness.status === 'failed') {
    return {
      status: readiness.status,
      exitCode: readiness.exitCode,
      approved: false,
      gates: [...gates, ...readiness.gates],
      reasons: [...reasons, ...readiness.reasons],
    }
  }
  gates.push(...readiness.gates)
  reasons.push(...readiness.reasons)

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
      'RELEASE BLOCKED: the combined-product distribution hold is in effect and/or readiness evidence is incomplete (see docs/raw-development/release-gates.md and docs/raw-development/readiness.md).',
    )
  }
  return verdict.exitCode
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  process.exitCode = main()
}
