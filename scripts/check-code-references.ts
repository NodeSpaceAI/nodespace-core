#!/usr/bin/env bun
// Prevents new drift against CLAUDE.md's rule against citing GitHub issue
// numbers or nodespace-docs/ paths in code comments ("describe the
// behavior/constraint directly, and reference decisions by ADR").
//
// Doc-path references are fully paid down (baseline 0). Issue-number
// references are not: the patterns once missed the most common shape, a
// bare `#NNNN` in prose ("see #990", "the #2242 audit"), so the backlog of
// those went uncounted while the baseline read 0. The issue-number baseline
// is the real remaining count (all in packages/agent, pending its owner's
// sign-off; every other package is at 0); paying it down means inlining each
// constraint-bearing comment's constraint before dropping the reference, and
// deleting provenance-only citations outright.
//
// Lower BASELINES whenever a change pays down part of the backlog. Never
// raise a baseline to accommodate a new reference — inline the constraint or
// cite an ADR instead, per the rule this check exists to hold the line on.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");

// Every package is in scope. List them explicitly (rather than deriving from
// `packages/*`) so a newly added package fails closed — it stays unscanned
// until someone notices and adds it here, rather than silently entering the
// ratchet with whatever count it happens to start with.
export const SCAN_ROOTS = [
  "scripts",
  "packages/desktop-app",
  "packages/core",
  "packages/daemon",
  "packages/cli",
  "packages/nodespace-types",
  "packages/proto",
  "packages/dev-tools",
  "packages/skill",
  "packages/agent",
  "packages/nlp-engine",
];
// Source files, plus `.proto`: a proto's comments become the generated
// Rust's doc comments, so an issue number there reaches code too.
const EXTENSIONS = new Set([".rs", ".ts", ".svelte", ".js", ".proto"]);
// Generated build output, not source. `.svelte-kit` matters as much as the
// rest: it inlines source comments into its bundles, so a build dir left over
// from before a cleanup keeps reporting references the source no longer has —
// and because the walk reads the filesystem rather than git, being gitignored
// does not spare it. That failure blocks every push until the stale build is
// deleted, which is a false positive rather than drift.
const EXCLUDE_DIR_NAMES = new Set([
  "node_modules",
  "target",
  ".git",
  "dist",
  "build",
  ".svelte-kit",
]);

// This checker's own source and test necessarily describe the patterns they
// scan for in comments/messages/fixtures, which would otherwise self-match.
//
// search_skills_latency.rs computes a real `nodespace-docs` sibling-directory
// path at runtime to write a benchmark report — functional filesystem logic,
// gracefully skipped when the directory is absent, not a stale documentation
// citation. That's a different thing from the rule this check enforces (a
// comment pointing a human at a doc for context), so it's excluded rather
// than rewritten to hide a real dependency.
const EXCLUDE_FILE_NAMES = new Set([
  "check-code-references.ts",
  "check-code-references.test.ts",
  "search_skills_latency.rs",
]);

const ISSUE_NUMBER_PATTERNS: RegExp[] = [
  /core#\d+/,
  /\(#\d+\)/,
  /\b[Ii]ssue #\d+\b/,
  /\bPR#\d+\b/,
  /\b(?:pre|post)-#\d+\b/,
  /\b(?:pre|post)-issue-\d+\b/,
  // A bare #NNNN in prose. Three to five digits, because every 1–2 digit
  // `#N` in this repo is data ("call #2", "UAX #9", "Finding #1") and a
  // 6-digit one is a hex color. The lookarounds drop the other non-issue
  // shapes: `core#N`/`r#` (word char before), `&#39;` entities, `'#333'`
  // colors quoted on both sides (a reference opening a string, as in a test
  // title `'#1234: …'`, still counts), `Invoice #001` fixture titles, and
  // `color: #888;` CSS.
  /(?<![\w&#]|Invoice |['"](?=#\d{3,5}['"]))#\d{3,5}\b(?!;)/,
];
// Also catches the pre-nodespace-docs shape, an in-repo `docs/<section>/`
// path (e.g. `docs/architecture/development/process/`), which predates the
// docs move and no longer exists in this repo. Scoped to the sibling repo's
// actual top-level sections (architecture/components/decisions/development)
// rather than a bare `docs\//`, which would also match unrelated generic
// paths already in the codebase (markdown-fixture paths like
// `docs/architecture.md` or `docs/intro.md`, and external doc-site URLs).
const DOC_PATH_PATTERN = /nodespace-docs\/|\bdocs\/(?:architecture|components|decisions|development)\//;

// Ratchet baselines. See the file-level comment: lower on paydown, never raise.
export const BASELINES = {
  issueNumberReferences: 71,
  docPathReferences: 0,
};

function walk(dir: string, out: string[]): void {
  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch {
    return;
  }
  for (const entry of entries) {
    if (EXCLUDE_DIR_NAMES.has(entry)) continue;
    const full = join(dir, entry);
    const info = statSync(full);
    if (info.isDirectory()) {
      walk(full, out);
    } else if (!EXCLUDE_FILE_NAMES.has(entry)) {
      const dot = entry.lastIndexOf(".");
      if (dot !== -1 && EXTENSIONS.has(entry.slice(dot))) {
        out.push(full);
      }
    }
  }
}

export interface ReferenceCounts {
  issueNumberReferences: number;
  docPathReferences: number;
  /** One `path:line: text` entry per matching line, repo-relative. */
  issueNumberHits: string[];
  docPathHits: string[];
}

/**
 * Scans SCAN_ROOTS for issue-number and nodespace-docs/ path references in
 * code comments/strings, line by line. A pure function of the filesystem —
 * no baseline comparison here, so it's independently testable against
 * injected fixtures.
 */
export function countReferences(roots: string[] = SCAN_ROOTS, repoRoot: string = REPO): ReferenceCounts {
  const files: string[] = [];
  for (const root of roots) {
    walk(join(repoRoot, root), files);
  }

  const issueNumberHits: string[] = [];
  const docPathHits: string[] = [];

  for (const file of files) {
    const lines = readFileSync(file, "utf8").split("\n");
    lines.forEach((line, i) => {
      const hit = `${relative(repoRoot, file)}:${i + 1}: ${line.trim()}`;
      if (ISSUE_NUMBER_PATTERNS.some((re) => re.test(line))) issueNumberHits.push(hit);
      if (DOC_PATH_PATTERN.test(line)) docPathHits.push(hit);
    });
  }

  return {
    issueNumberReferences: issueNumberHits.length,
    docPathReferences: docPathHits.length,
    issueNumberHits,
    docPathHits,
  };
}

/**
 * One actionable message per baseline the counts exceed, each listing every
 * matching line so the new reference can be found among the backlog. Empty
 * when both counts are within their baselines.
 */
export function baselineFailures(counts: ReferenceCounts): string[] {
  const failures: string[] = [];
  if (counts.issueNumberReferences > BASELINES.issueNumberReferences) {
    failures.push(
      `${counts.issueNumberReferences} issue-number references in code (#NNNN, core#NNNN, Issue #NNNN), ` +
        `up from the ${BASELINES.issueNumberReferences}-reference baseline in scripts/check-code-references.ts. ` +
        "Describe the behavior/constraint directly and cite an ADR instead, per CLAUDE.md. Matching lines:\n" +
        counts.issueNumberHits.join("\n"),
    );
  }
  if (counts.docPathReferences > BASELINES.docPathReferences) {
    failures.push(
      `${counts.docPathReferences} nodespace-docs/ path references in code, up from the ` +
        `${BASELINES.docPathReferences}-reference baseline in scripts/check-code-references.ts. ` +
        "Inline the essential fact, or cite an ADR, instead of a path into a separate repo. Matching lines:\n" +
        counts.docPathHits.join("\n"),
    );
  }
  return failures;
}

if (import.meta.main) {
  const counts = countReferences();
  const failures = baselineFailures(counts);
  for (const failure of failures) console.error(`❌ ${failure}`);
  if (failures.length > 0) process.exit(1);

  console.log(
    `✅ Issue-number references: ${counts.issueNumberReferences} (baseline ${BASELINES.issueNumberReferences}). ` +
      `Doc-path references: ${counts.docPathReferences} (baseline ${BASELINES.docPathReferences}).`,
  );
}
