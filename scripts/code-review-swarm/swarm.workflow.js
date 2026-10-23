export const meta = {
  name: 'codebase-swarm',
  description: 'Resumable multi-agent review of the core and sync code bases: find, verify, review, then file only approved issues',
  phases: [
    { title: 'Find', detail: 'one agent per unit and lens; opus on risky code, sonnet elsewhere' },
    { title: 'Consolidate', detail: 'dedupe, rank and cap the candidates' },
    { title: 'Verify', detail: 'independent skeptics try to refute each candidate' },
    { title: 'Review', detail: 'a chair approves, rejects, or marks duplicate / by design' },
    { title: 'File', detail: 'file approved findings only' },
    { title: 'Report', detail: 'ranked report of everything' },
  ],
}

// ---------------------------------------------------------------------------
// Resumability: every agent writes its result to a file under args.stateDir and
// starts by reading that file if it already exists. args.done (built by
// make-args.ts from the directory listing) lets the script skip finished work
// without spawning anything. Re-running with fresh args after a crash or a
// quota stop continues where the files stop.
// ---------------------------------------------------------------------------

const S = args.stateDir
const done = args.done || {}
const doneFind = new Set(done.find || [])
const doneVerify = new Set(done.verify || [])
const doneReview = done.review || {}
const doneFiled = new Set(done.filed || [])
const MAX_CANDIDATES = args.maxCandidates || 60
const LEAN = args.profile === 'lean'
const RUN_DATE = args.runDate || '(date not given)'
const CORE_REPO = args.coreRepo || 'the live core checkout'
const SYNC_REPO = args.syncRepo || 'the live sync checkout'
const DOCS_TEXT = args.docsRepo
  ? `The architecture docs and ADRs are in ${args.docsRepo} (a git checkout of nodespace-docs; run git -C ${args.docsRepo} fetch origin main and read origin/main versions with git show).`
  : 'No docs checkout is available on this machine: skip the ADR checks and say in your result that you could not check the docs.'
const DOCS_REF = args.docsRepo || 'the docs checkout (not available on this machine)'

const RULES = `HARD RULES for every agent in this swarm:
- Read-only on the code. Never edit, commit, push, branch, merge or comment on any PR or issue. Only the FILE role may create one GitHub issue.
- Code snapshots are pinned read-only checkouts: core at ${S}/src-core and sync at ${S}/src-sync (detached at the SHAs in ${S}/shas.json). Read code from there, never from ${CORE_REPO} or ${SYNC_REPO}. ${DOCS_TEXT}
- Each repo has a CLAUDE.md with the project rules and invariants: read the one for your repo first.
- No cargo builds or tests, no bun test or vitest runs, no model loading or GPU work. Static reading, grep or rg, git log and git blame, jq, and small throwaway scripts under ${S}/scratch/<your-key>/ only.
- Never read any .env file or any credentials. Never touch files or processes of any other macOS user. Never signal or kill a process.
- Do not tag or assign anyone (not Michael, not Anoop) anywhere.
- Already-known issues, open and closed, for all three repos: ${S}/existing-issues.json. Do not re-report something already there.
- Write your result ONLY to the file named in your task, as valid JSON, then return the same content. If that file already exists and parses as JSON, read it and return it without redoing the work.
- Be honest. An unproven suspicion is not a finding. Say what you could not check.`

const SEV = `Severity scale: P0 = data loss, security exploit, or a crash in normal use; P1 = user-visible wrong behaviour or a bug in a common path; P2 = latent or edge-case bug, robustness gap, real discrepancy between docs and behaviour; P3 = dead code, bad code, cleanup worth doing.`

const FIND_SCHEMA = {
  type: 'object',
  properties: {
    unit: { type: 'string' },
    lens: { type: 'string' },
    filesRead: { type: 'integer' },
    candidates: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          title: { type: 'string' },
          severity: { type: 'string', enum: ['P0', 'P1', 'P2', 'P3'] },
          category: { type: 'string', enum: ['bug', 'security', 'data-loss', 'concurrency', 'dead-code', 'bad-code', 'discrepancy', 'test-quality', 'infra'] },
          locations: { type: 'array', items: { type: 'object', properties: { path: { type: 'string' }, line: { type: 'integer' }, quote: { type: 'string' } }, required: ['path', 'line', 'quote'] } },
          summary: { type: 'string' },
          scenario: { type: 'string' },
          evidence: { type: 'string' },
          suggestedFix: { type: 'string' },
          confidence: { type: 'number' },
        },
        required: ['title', 'severity', 'category', 'locations', 'summary', 'scenario', 'evidence', 'confidence'],
      },
    },
    notChecked: { type: 'string' },
  },
  required: ['unit', 'lens', 'candidates'],
}

const CONSOLIDATE_SCHEMA = {
  type: 'object',
  properties: {
    candidates: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          id: { type: 'string' },
          repo: { type: 'string' },
          title: { type: 'string' },
          severity: { type: 'string', enum: ['P0', 'P1', 'P2', 'P3'] },
          category: { type: 'string' },
          sources: { type: 'array', items: { type: 'string' } },
        },
        required: ['id', 'repo', 'title', 'severity', 'category', 'sources'],
      },
    },
    totals: { type: 'object', properties: { raw: { type: 'integer' }, afterDedupe: { type: 'integer' }, overflow: { type: 'integer' } }, required: ['raw', 'afterDedupe'] },
  },
  required: ['candidates', 'totals'],
}

const VERDICT_SCHEMA = {
  type: 'object',
  properties: {
    id: { type: 'string' },
    verifier: { type: 'string' },
    verdict: { type: 'string', enum: ['CONFIRMED', 'REFUTED', 'UNSURE'] },
    quotesMatch: { type: 'boolean' },
    evidence: { type: 'string' },
    severityAdjust: { type: 'string' },
  },
  required: ['id', 'verifier', 'verdict', 'evidence'],
}

const REVIEW_SCHEMA = {
  type: 'object',
  properties: {
    id: { type: 'string' },
    decision: { type: 'string', enum: ['APPROVE', 'REJECT', 'DUPLICATE', 'BY_DESIGN', 'NEEDS_DECISION'] },
    finalSeverity: { type: 'string', enum: ['P0', 'P1', 'P2', 'P3'] },
    reasoning: { type: 'string' },
    duplicateOf: { type: 'string' },
  },
  required: ['id', 'decision', 'finalSeverity', 'reasoning'],
}

const FILED_SCHEMA = {
  type: 'object',
  properties: { id: { type: 'string' }, url: { type: 'string' }, repo: { type: 'string' }, number: { type: 'integer' }, skipped: { type: 'string' } },
  required: ['id'],
}

const REPORT_SCHEMA = {
  type: 'object',
  properties: { reportPath: { type: 'string' }, counts: { type: 'object', properties: { filed: { type: 'integer' }, rejected: { type: 'integer' }, duplicate: { type: 'integer' }, byDesign: { type: 'integer' }, needsDecision: { type: 'integer' }, incompleteUnits: { type: 'integer' } } }, markdown: { type: 'string' } },
  required: ['reportPath', 'markdown'],
}

// ---------------------------------------------------------------------------
// Phase 1: Find
// ---------------------------------------------------------------------------

const LENS_TEXT = {
  deep: `LENS deep: correctness, concurrency, error handling, data loss and security. Read EVERY file in the unit in full (not a skim). Look for: wrong results, unhandled errors that lose data or leave state inconsistent, races and lock-order problems, TOCTOU, panics reachable from user or network input (unwrap, indexing, slicing), integer overflow, resource leaks, trust-boundary mistakes (paths, shell, SQL, URLs, deserialisation), permission and auth checks that can be skipped, violations of the repo's invariants in CLAUDE.md, and contracts between this unit and its callers that do not match (follow callers and callees with grep, in both pinned repos).`,
  hygiene: `LENS hygiene: dead code, bad code and discrepancies. Read EVERY file in the unit in full. Look for: functions, types, exports, config keys, CLI flags and feature flags with no live caller (prove it: grep the whole of BOTH pinned repos including tests, macros, serde rename names, string-based dispatch and generated bindings, because sync consumes core), duplicated logic that has already drifted apart, comments, docs or error messages that contradict the code, stale TODO or FIXME that describe a bug that still exists, swallowed errors, misleading names, ignored or skipped tests with an obsolete reason, and patterns the repo's own CLAUDE.md forbids.`,
  tests: `LENS tests: test quality. Read EVERY file in the unit in full. Look for: tests that cannot fail (assert nothing, assert on the mock, tautologies), tests that pass for the wrong reason, #[ignore] or skip with an obsolete reason, sleep-based timing that is flaky, tests whose name promises something the body does not check, duplicate tests, and important invariants from CLAUDE.md or the code that have no test at all. Report only the ones that matter.`,
}

const findPrompt = (t) => `${RULES}

ROLE: FINDER. Unit ${t.u.key} (repo ${t.u.repo}, tier ${t.u.tier}, about ${t.u.loc} lines). Your output file: ${S}/findings/${t.key}.json
First run: jq '.[] | select(.key=="${t.u.key}")' ${S}/units.json  to get the list of files in your unit (paths are relative to the pinned checkout ${S}/src-${t.u.repo}). Read the repo's CLAUDE.md first.
${LENS_TEXT[t.lens]}
${SEV}
QUALITY BAR (a separate panel will try to refute everything you report, and false reports waste real money): report at most 12 candidates, only ones you can prove from the code. Every candidate needs: exact path and line with a verbatim quote of the code (copy it, do not paraphrase), a concrete scenario (inputs or sequence of events that triggers it and what goes wrong), and the evidence you gathered (callers traced, tests checked, git blame or log if it shows intent). No speculative "might" findings, no style nits, no formatting, no missing docs, no "could add a test". For dead code, show the grep commands you ran and that they returned nothing. Confidence is your honest probability (0 to 1) that a skeptical senior engineer will agree it is real. If the unit is clean, return an empty candidates list and say what you checked. Put anything you could not check in notChecked.
Return JSON matching the schema: {unit: "${t.u.key}", lens: "${t.lens}", filesRead, candidates, notChecked}.`

phase('Find')
let findTasks = []
for (const u of args.units) {
  for (const lens of u.lenses) {
    if (LEAN && (lens === 'hygiene' && u.tier === 'critical')) continue
    if (LEAN && u.tier === 'tests') continue
    findTasks.push({ u, lens, key: `${u.key}__${lens}` })
  }
}
const pendingFind = findTasks.filter((t) => !doneFind.has(t.key))
log(`Find: ${findTasks.length} unit-lens tasks, ${findTasks.length - pendingFind.length} already done, ${pendingFind.length} to run`)
const findResults = await parallel(
  pendingFind.map((t) => () =>
    agent(findPrompt(t), {
      label: `find:${t.key}`,
      phase: 'Find',
      model: t.lens === 'deep' ? t.u.model : 'sonnet',
      schema: FIND_SCHEMA,
    })
  )
)
const failedFind = pendingFind.filter((t, i) => !findResults[i]).map((t) => t.key)
log(`Find finished: ${pendingFind.length - failedFind.length} ok, ${failedFind.length} failed or skipped${failedFind.length ? ': ' + failedFind.join(', ') : ''}`)

// ---------------------------------------------------------------------------
// Phase 2: Consolidate (barrier: dedupe needs every finding)
// ---------------------------------------------------------------------------

phase('Consolidate')
const cons = await agent(
  `${RULES}

ROLE: CONSOLIDATOR. Your output file: ${S}/candidates.json  ${args.reuseCandidates ? '(it already exists from an earlier run: read it, return it unchanged, do nothing else)' : ''}
Read every file in ${S}/findings/*.json (use jq to list and summarise; there can be a hundred files). Merge candidates that describe the same root cause or the same lines (keep the best evidence, list every source file name in "sources", raise the severity only if a second source shows a wider impact). Drop anything that is obviously a nit, a duplicate of ${S}/existing-issues.json (match by meaning, not wording), or whose quote does not appear at that path and line in the pinned checkout (spot check by grepping the quote). Give each survivor an id F-001, F-002, ... ordered by severity then confidence. Keep at most ${MAX_CANDIDATES} candidates; if more survive, keep the highest severity times confidence and write the rest to ${S}/overflow-candidates.json (full candidate objects, so they can be shown to the user as UNVERIFIED later). Write the FULL merged candidate objects (title, severity, category, repo, locations with quotes, summary, scenario, evidence, suggestedFix, confidence, sources) to ${S}/candidates.json as {"candidates":[...]}. Return the compact list {id, repo, title, severity, category, sources} plus totals {raw, afterDedupe, overflow}.`,
  { label: 'consolidate', phase: 'Consolidate', model: 'opus', schema: CONSOLIDATE_SCHEMA }
)
if (!cons || !cons.candidates) {
  log('Consolidation failed; stopping. Re-run the workflow; finished finder files are kept.')
  return { stopped: 'consolidation failed', failedFind }
}
const candidates = cons.candidates.slice(0, MAX_CANDIDATES)
log(`Consolidated: ${cons.totals.raw} raw -> ${candidates.length} candidates (overflow ${cons.totals.overflow || 0})`)

// ---------------------------------------------------------------------------
// Phases 3-5 as a pipeline per candidate: verify -> review -> file
// ---------------------------------------------------------------------------

const verifierPlan = (c) => {
  if (c.severity === 'P0' || c.severity === 'P1') return [{ n: 1, kind: 'trace', model: 'opus' }, { n: 2, kind: 'repro', model: 'sonnet' }]
  if (c.severity === 'P2') return [{ n: 1, kind: 'trace', model: 'sonnet' }, { n: 2, kind: 'repro', model: 'sonnet' }]
  return [{ n: 1, kind: 'trace', model: 'sonnet' }]
}

const KIND_TEXT = {
  trace: 'Method TRACE: re-read the cited code yourself in the pinned checkout, follow every caller and callee, check the tests that exercise it, check git log and git blame for intent, and look for a guard elsewhere that makes the problem impossible. Decide whether the claim holds.',
  repro: 'Method REPRO: build the exact input or event sequence from the scenario step by step against the real code (not against the finder\'s description), and walk the execution by hand. Say where the walk-through fails if it does. Also check whether the scenario is reachable in the shipped product (not only in a test or a dead path).',
}

const verifyStage = async (c) => {
  const plan = verifierPlan(c)
  if (doneVerify.has(c.id)) return { c, skippedVerify: true }
  const out = await parallel(
    plan.map((v) => () =>
      agent(
        `${RULES}

ROLE: SKEPTIC VERIFIER (${v.kind}). Candidate ${c.id}: "${c.title}" (${c.repo}, ${c.severity}, ${c.category}). Full candidate details: jq '.candidates[] | select(.id=="${c.id}")' ${S}/candidates.json
Your output file: ${S}/verdicts/${c.id}__${v.n}.json
You are one of independent verifiers. Your job is to try to REFUTE this finding. Default to REFUTED or UNSURE if you cannot prove it from the code. ${KIND_TEXT[v.kind]}
Also verify that each quoted line really appears at the cited path and line (quotesMatch). Dead-code claims: re-run the greps across BOTH pinned repos, including tests, macros and serde names. Do not trust the finder's evidence, re-derive it.
Return {id: "${c.id}", verifier: "${v.kind}-${v.n}", verdict, quotesMatch, evidence (what you checked and found, with paths and lines), severityAdjust (if the severity is wrong, say to what and why)}.`,
        { label: `verify:${c.id}:${v.n}`, phase: 'Verify', model: v.model, schema: VERDICT_SCHEMA }
      )
    )
  )
  return { c, verdicts: out.filter(Boolean) }
}

const reviewStage = async (prev, c) => {
  if (!prev) return null
  if (doneReview[c.id]) return { c, decision: doneReview[c.id], skippedReview: true }
  if (prev.verdicts && prev.verdicts.length > 0 && prev.verdicts.every((v) => v.verdict === 'REFUTED')) {
    // Every verifier refuted it: still record the decision, with a cheap agent.
  }
  const r = await agent(
    `${RULES}

ROLE: CHAIR OF THE REVIEW SESSION. Candidate ${c.id}: "${c.title}" (${c.repo}, ${c.severity}). Details: jq '.candidates[] | select(.id=="${c.id}")' ${S}/candidates.json . Verifier verdicts: ${S}/verdicts/${c.id}__*.json . Your output file: ${S}/review/${c.id}.json
Decide whether this is a REAL, actionable issue and not a hoax. Read the code yourself in the pinned checkout. Then decide:
- APPROVE only if you personally confirm it from the code, at least one verifier confirmed with evidence you checked, no verifier refuted it with evidence you cannot answer, and it is not already tracked.
- REJECT if it is wrong, unreachable, already guarded, or too speculative.
- DUPLICATE if an open or closed issue in ${S}/existing-issues.json covers it (give duplicateOf as repo#number); also run gh issue list --repo <repo> --state all --search "<keywords>" read-only.
- BY_DESIGN if an ADR, the docs in ${DOCS_REF}, a code comment or CLAUDE.md says this is intended (cite it).
- NEEDS_DECISION if it is real but the right behaviour is a product or architecture choice that only the team can make.
Set finalSeverity from the evidence, not from the finder's claim. Dead-code findings are APPROVE only if the proof of no callers holds in both repos.
${SEV}
Return {id, decision, finalSeverity, reasoning (cite paths, lines, ADRs, issues), duplicateOf}.`,
    { label: `review:${c.id}`, phase: 'Review', model: (c.severity === 'P3' ? 'sonnet' : 'opus'), schema: REVIEW_SCHEMA }
  )
  if (!r) return null
  return { c, decision: r.decision, review: r }
}

const fileStage = async (prev, c) => {
  if (!prev) return null
  if (prev.decision !== 'APPROVE') return { c, decision: prev.decision, filed: null }
  if (doneFiled.has(c.id)) return { c, decision: 'APPROVE', filed: 'already' }
  const sev = (prev.review && prev.review.finalSeverity) || c.severity
  const footer = c.repo === 'sync'
    ? 'Do NOT add any "Generated with Claude Code" or robot footer (this is the sync repo).'
    : 'End the body with this exact last line: 🤖 Generated with [Claude Code](https://claude.com/claude-code)'
  const f = await agent(
    `${RULES}

ROLE: FILER. Approved finding ${c.id}: "${c.title}" (repo ${c.repo}, severity ${sev}). Details: jq '.candidates[] | select(.id=="${c.id}")' ${S}/candidates.json . Review decision: ${S}/review/${c.id}.json . Verdicts: ${S}/verdicts/${c.id}__*.json . Your output file: ${S}/filed/${c.id}.json
You are the ONLY role allowed to create a GitHub issue, and only this one. First re-check for a duplicate: gh issue list -R NodeSpaceAI/nodespace-${c.repo} --state all --search "<2-3 keywords>" --limit 20 ; if an issue already covers it, do not file, return {id, skipped: "duplicate of #N"}.
Create the issue with: gh issue create -R NodeSpaceAI/nodespace-${c.repo} --title "<concise title in plain words, no ticket numbers>" --body-file <a temp file under ${S}/scratch/${c.id}/>   Do not pass --assignee, --label (unless the label already exists and clearly fits), --milestone or any project flag; do not mention or tag any person.
Body template (keep it factual and short): "Severity: ${sev}" line; "## Where" with path:line (verbatim quote in a code block); "## What goes wrong" with the concrete scenario; "## Evidence" (what was traced and verified, incl. that independent verification confirmed it); "## Suggested fix"; "## Found by" one line: "Found by an automated whole-codebase review on ${RUN_DATE}; confirmed by independent verification and a review pass." Describe code, not narrative; no ticket numbers inside the text unless an existing issue is directly related (then link it as a plain reference). ${footer}
Return {id, url, repo, number}. Write the same JSON to the output file.`,
    { label: `file:${c.id}`, phase: 'File', model: 'sonnet', schema: FILED_SCHEMA }
  )
  return { c, decision: 'APPROVE', filed: f }
}

phase('Verify')
log(`Verify/Review/File pipeline over ${candidates.length} candidates`)
const pipelineResults = await pipeline(candidates, verifyStage, reviewStage, fileStage)

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

phase('Report')
const report = await agent(
  `${RULES}

ROLE: REPORTER. Write the final report to ${S}/report.md and return it. Read: ${S}/candidates.json, ${S}/overflow-candidates.json (if it exists), ${S}/review/*.json, ${S}/verdicts/*.json, ${S}/filed/*.json, and list ${S}/findings/ against ${S}/units.json (every unit-lens pair that is expected but has no findings file is an INCOMPLETE unit: list them, with the reason if you can tell).
Report layout, in this order, concise tables with one line per item:
1. Counts: units reviewed, find agents run, raw candidates, after dedupe, verified, approved and filed, rejected, duplicate, by design, needs decision, overflow (unverified), incomplete units. Include the rejection rate (how many 'hoaxes' the panel caught).
2. FILED issues grouped by final severity P0, P1, P2, P3: issue URL, one-line summary, repo.
3. NEEDS_DECISION items (real but need a team choice): one line each with the question.
4. Rejected / by design / duplicate: one line each with the reason (so the user can audit the panel).
5. Overflow candidates that were NOT verified (clearly labelled unverified; they were not filed).
6. Incomplete units and how to resume (re-run the workflow with fresh args from make-args.ts).
Return {reportPath: "${S}/report.md", counts, markdown: <the whole report>}.`,
  { label: 'report', phase: 'Report', model: 'sonnet', schema: REPORT_SCHEMA }
)
return { report: report ? report.reportPath : null, counts: report ? report.counts : null, failedFind }
