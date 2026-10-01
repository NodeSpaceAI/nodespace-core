#!/usr/bin/env bun
// Enforces ADR-087 section 2: nothing outside the governance module reads
// `lifecycle_status`. The field is governance state with one meaning (an
// archived node participates in nothing), and one check decides it for every
// surface. A comparison written anywhere else is a surface applying its own
// variant of the rule, or a type giving `archived` a meaning of its own.
//
// The governance module is the alternative, in each form:
//
//   Rust   crate::governance::participates(&node) / is_visible(&node, include_archived)
//   SQL    crate::governance::participates_sql(alias) / visible_sql(..) /
//          default_query_conditions(alias, include_archived)
//
// Two tiers:
//
//   Interpreting the field is banned everywhere but the governance module: a
//   comparison (Rust, TypeScript or SQL), a `match`, an `.as_str()`, or a
//   lookup of the field by its name as a key.
//
//   Reading it at all is banned outside the envelope and serialization code.
//   Three shapes carry the field without reading its meaning, and are allowed
//   anywhere: assigning it, initializing the same-named field of another
//   shape (`lifecycle_status: node.lifecycle_status`), and the field of a
//   write request (`update.lifecycle_status`), which is the value being
//   written and not a node's state. The files in ENVELOPE_FILES define the
//   envelope, persist it or print it, and may read it plainly. They may not
//   interpret it.
//
// Scope: the files git lists under `packages/` with a scanned extension.
// Tests are out of scope (they assert on the field by design), as in
// check-node-type-literals. This is a hard check with no baseline: a hit
// fails, and the fix is the governance module.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { codeLines, extensionOf, isTestPath, listRepoFiles, REPO, SCAN_PREFIX, SCANNED_EXTENSIONS } from "./check-node-type-literals";

export { REPO };

/** The module that owns the participation check. Nothing in it is scanned. */
export const GOVERNANCE_MODULE = "packages/core/src/governance.rs";

/**
 * Envelope and serialization code: where the field is defined, persisted or
 * printed. A plain read is allowed here; interpreting the field is not.
 */
export const ENVELOPE_FILES: readonly string[] = [
  "packages/nodespace-types/src/node.rs",
  "packages/nodespace-types/src/helpers.rs",
  "packages/core/src/db/sqlite_store/mod.rs",
  "packages/core/src/db/sqlite_store/nodes.rs",
  "packages/cli/src/output.rs",
];

const FIELD = "(?:lifecycle_status|lifecycleStatus)";
const QUOTE = "[\"'`]";

// Tested once per code line, in order; the first match names the hit.
export const INTERPRETING = {
  comparison: {
    pattern: new RegExp(`${FIELD}(?:\\.as_str\\(\\)|\\.as_deref\\(\\))?\\s*(?:===|!==|==|!=)`),
    summary: "lifecycle_status compared",
  },
  reversedComparison: {
    pattern: new RegExp(`(?:===|!==|==|!=)\\s*[\\w.?&()*]*${FIELD}\\b`),
    summary: "a value compared with lifecycle_status",
  },
  // A write binds the column in a SET list or an INSERT (`SET lifecycle_status
  // = ?1`). A query filters on it: against a literal, in filter position
  // (after WHERE / AND / OR / ON) even with a bound value, with IN / IS, or
  // in a CASE.
  sqlComparison: {
    pattern: new RegExp(
      `lifecycle_status\\s*(?:=|!=|<>)\\s*'|lifecycle_status\\s+(?:NOT\\s+)?IN\\s*\\(|lifecycle_status\\s+IS\\s+(?:NOT\\s+)?NULL\\b|\\b(?:WHERE|AND|OR|ON)\\s+(?:\\w+\\.)?lifecycle_status\\s*(?:=|!=|<>|<|>)|\\bCASE\\s+(?:\\w+\\.)?lifecycle_status\\b`,
      "i",
    ),
    summary: "SQL filtering on lifecycle_status",
  },
  // With or without a receiver: a destructured field is matched on by its
  // bare name.
  match: {
    pattern: new RegExp(`\\bmatch(?:es!|\\b)[^{;]*\\.${FIELD}\\b(?!\\s*\\{)|\\bmatch(?:es!\\(|\\s)\\s*[&*]*${FIELD}\\b(?!\\s*\\{)|${FIELD}\\.as_str\\(\\)`),
    summary: "lifecycle_status matched on",
  },
  keyLookup: {
    pattern: new RegExp(`${QUOTE}${FIELD}${QUOTE}\\s*=>|\\bget\\(\\s*${QUOTE}${FIELD}${QUOTE}\\s*\\)|\\[\\s*${QUOTE}${FIELD}${QUOTE}\\s*\\]|${QUOTE}${FIELD}${QUOTE}\\s*\\||\\|\\s*${QUOTE}${FIELD}${QUOTE}`),
    summary: "lifecycle_status looked up by name",
  },
} satisfies Record<string, { pattern: RegExp; summary: string }>;

export const READING = {
  read: {
    summary: "lifecycle_status read outside the envelope code",
  },
};

export type PatternName = keyof typeof INTERPRETING | keyof typeof READING;
const INTERPRETING_NAMES = Object.keys(INTERPRETING) as (keyof typeof INTERPRETING)[];
const SUMMARIES: Record<PatternName, string> = {
  ...Object.fromEntries(INTERPRETING_NAMES.map((name) => [name, INTERPRETING[name].summary])),
  read: READING.read.summary,
} as Record<PatternName, string>;

export interface Hit {
  file: string;
  line: number;
  text: string;
  pattern: PatternName;
}

const ACCESS = new RegExp(`([\\w)\\]?]*)\\.${FIELD}\\b`, "g");
const ASSIGNED = new RegExp(`^\\s*=(?!=)`);
// The same-named field of another shape being initialized: `lifecycle_status:
// node.lifecycle_status`, or the JSON key form.
const CARRIED = new RegExp(`(?:^|[\\s{(,])${QUOTE}?${FIELD}${QUOTE}?\\s*:`);
// A write request's field is the value being written, not a node's state.
const WRITE_REQUEST = /^(?:update|params|input|req|request|body)$/;

/** Whether a line reads the field off something that isn't a write request. */
function readsNodeState(line: string): boolean {
  if (CARRIED.test(line)) return false;
  for (const access of line.matchAll(ACCESS)) {
    const receiver = access[1].replace(/[)\]?]+$/, "");
    const rest = line.slice((access.index ?? 0) + access[0].length);
    if (ASSIGNED.test(rest)) continue;
    if (WRITE_REQUEST.test(receiver)) continue;
    return true;
  }
  return false;
}

/** Every lifecycle read in one file's text. */
export function findHits(file: string, text: string): Hit[] {
  const hits: Hit[] = [];
  const envelope = ENVELOPE_FILES.includes(file);
  for (const { line, text: lineText } of codeLines(file, text)) {
    const interpreting = INTERPRETING_NAMES.find((name) => INTERPRETING[name].pattern.test(lineText));
    if (interpreting) {
      hits.push({ file, line, text: lineText.trim(), pattern: interpreting });
      continue;
    }
    if (!envelope && readsNodeState(lineText)) {
      hits.push({ file, line, text: lineText.trim(), pattern: "read" });
    }
  }
  return hits;
}

/** Whether a repo-relative path is scanned. */
export function isScanned(path: string): boolean {
  if (!path.startsWith(SCAN_PREFIX)) return false;
  if (path === GOVERNANCE_MODULE) return false;
  if (isTestPath(path)) return false;
  return SCANNED_EXTENSIONS.has(extensionOf(path));
}

/** Repo-relative paths in scope. Throws outside a git checkout. */
export function listScannedFiles(repoRoot: string = REPO): string[] {
  return listRepoFiles(repoRoot, "check-lifecycle-reads").filter(isScanned);
}

/** Every hit over the given files. A file that can't be read is skipped. */
export function scan(files: readonly string[], repoRoot: string = REPO): Hit[] {
  const hits: Hit[] = [];
  for (const file of files) {
    let text: string;
    try {
      text = readFileSync(join(repoRoot, file), "utf8");
    } catch {
      continue;
    }
    hits.push(...findHits(file, text));
  }
  return hits;
}

/** The failure message for a set of hits, or null when there are none. */
export function failureMessage(hits: readonly Hit[]): string | null {
  if (hits.length === 0) return null;
  const lines = hits.map((hit) => `  ${hit.file}:${hit.line}: ${hit.text}\n      ${SUMMARIES[hit.pattern]}`);
  return [
    `${hits.length} lifecycle_status read${hits.length === 1 ? "" : "s"} outside the governance module (ADR-087 section 2):`,
    ...lines,
    "",
    "lifecycle_status is governance state, read through one participation check. Ask the governance module instead:",
    "  Rust  crate::governance::participates(&node) / is_visible(&node, include_archived)",
    "  SQL   crate::governance::participates_sql(alias) / default_query_conditions(alias, include_archived)",
    "A type's own state (an on/off switch, a read-only flag) is a field of its schema, never lifecycle_status.",
  ].join("\n");
}

if (import.meta.main) {
  const message = failureMessage(scan(listScannedFiles()));
  if (message) {
    console.error(message);
    process.exit(1);
  }
  console.log("✓ No lifecycle_status reads outside the governance module.");
}
