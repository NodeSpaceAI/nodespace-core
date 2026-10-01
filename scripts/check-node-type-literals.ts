#!/usr/bin/env bun
// Enforces ADR-086 section 5: no code applies a type's rule by comparing
// `node_type` with a string. A literal comparison is true for the type it
// names and false for every type that extends it, so each one is a place a
// subtype silently falls out of its base type's rule.
//
// The registry is the alternative, in each language:
//
//   Rust        CoreNodeType::nearest(&chain), SqliteStore::type_is_a /
//               core_type_of, NodeService::type_is_a
//   SQL         crate::db::schema::is_a_sql / is_not_a_sql, which resolve the
//               type through the type_ancestry table
//   TypeScript  isA(nodeType, base) from core-node-types
//
// Exactness is sometimes the real semantics: the `schema` meta-type, which
// nothing can extend, and a typed wire shape, which a subtype never borrows
// from its base. Those say so through the registry too
// (`CoreNodeType::X.is_exactly(..)`, `is_exactly_sql`, `isExactly`), which
// keeps every exact comparison greppable and deliberate.
//
// Scope: the files git lists under `packages/` with a scanned extension.
// Tests are out of scope (they assert on concrete types by design): test
// files by path, and in a Rust source file everything from its first
// `#[cfg(test)]` module on. The registry modules themselves are excluded,
// because they define the helpers. This is a hard check with no baseline: a
// hit fails, and the fix is the registry.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

export const REPO = join(dirname(new URL(import.meta.url).pathname), "..");

export const SCAN_PREFIX = "packages/";
export const SCANNED_EXTENSIONS: ReadonlySet<string> = new Set([".rs", ".ts", ".svelte"]);

// The modules that define the registry and its exact-comparison helpers.
export const EXCLUDED_FILES: readonly string[] = [
  "packages/nodespace-types/src/core_type.rs",
  "packages/desktop-app/src/lib/types/core-node-types.ts",
];

const TEST_PATH = /(^|\/)(tests|benches|__tests__)\/|_test\.rs$|(^|\/)tests\.rs$|\.test\.ts$|\.spec\.ts$|\.test\.svelte\.ts$/;

/** Whether a path is test code, which may name concrete types. */
export function isTestPath(path: string): boolean {
  return TEST_PATH.test(path);
}

const TYPE_ID = "[a-z][a-z0-9_-]*";
const QUOTE = "[\"'`]";
const FIELD = "(?:node_type|nodeType)";

// Each pattern is tested once per code line. Sources are composed from the
// fragments above so the intent of each stays readable.
export const PATTERNS = {
  comparison: {
    pattern: new RegExp(`(?<!typeof\\s+[\\w.?]*)${FIELD}(?:\\.as_str\\(\\))?\\s*(?:===|!==|==|!=)\\s*${QUOTE}${TYPE_ID}${QUOTE}`),
    summary: "a node type compared with a string literal",
  },
  reversedComparison: {
    pattern: new RegExp(`${QUOTE}${TYPE_ID}${QUOTE}\\s*(?:===|!==|==|!=)\\s*[\\w.?&()]*${FIELD}\\b`),
    summary: "a string literal compared with a node type",
  },
  // Spaces around the operator are required: that is how SQL is written here,
  // and it keeps a keyword argument in prose (`search_nodes(node_type='task')`)
  // from reading as SQL.
  sqlComparison: {
    pattern: new RegExp(`node_type\\s+(?:=|!=|<>)\\s+'${TYPE_ID}'`),
    summary: "SQL comparing node_type with a literal",
  },
  // `IN` followed by anything but a subquery or bound placeholders, the list
  // on the next line included: a literal list is the only other thing SQL
  // puts there.
  sqlList: {
    pattern: /node_type\s+(?:NOT\s+)?IN\b(?!\s*\(\s*(?:SELECT\b|\{|\?))/,
    summary: "SQL testing node_type against a literal list",
  },
  // A constant holding a type id is a literal by another name.
  constantComparison: {
    pattern: new RegExp(`${FIELD}(?:\\.as_str\\(\\))?\\s*(?:==|!=)\\s*&?[\\w:]*_NODE_TYPE\\b|[\\w:]*_NODE_TYPE\\s*(?:==|!=)\\s*&?[\\w.]*${FIELD}\\b`),
    summary: "a node type compared with a type-id constant",
  },
  rustMatches: {
    pattern: new RegExp(`matches!\\(\\s*[\\w.&()]*${FIELD}[\\w.()]*\\s*,\\s*"`),
    summary: "matches! on a node type against string literals",
  },
} satisfies Record<string, { pattern: RegExp; summary: string }>;

export type PatternName = keyof typeof PATTERNS;
export const PATTERN_NAMES = Object.keys(PATTERNS) as PatternName[];

export interface Hit {
  file: string;
  line: number;
  text: string;
  pattern: PatternName;
}

const COMMENT_LINE = /^\s*(?:\/\/|\/\*|\*|<!--|--\s)/;
const RUST_TEST_MODULE = /^\s*#\[cfg\(test\)\]\s*$/;

/**
 * The lines of a file that are in scope: code lines only, and for a Rust
 * source file only those before its first `#[cfg(test)]` module.
 */
export function codeLines(file: string, text: string): { line: number; text: string }[] {
  const lines = text.split("\n");
  const out: { line: number; text: string }[] = [];
  for (let i = 0; i < lines.length; i++) {
    if (file.endsWith(".rs") && RUST_TEST_MODULE.test(lines[i]) && /^\s*(?:pub\s+)?mod\s+\w+/.test(lines[i + 1] ?? "")) break;
    if (COMMENT_LINE.test(lines[i])) continue;
    out.push({ line: i + 1, text: lines[i] });
  }
  return out;
}

/** Every literal node-type comparison in one file's text. */
export function findHits(file: string, text: string): Hit[] {
  const hits: Hit[] = [];
  for (const { line, text: lineText } of codeLines(file, text)) {
    for (const name of PATTERN_NAMES) {
      if (PATTERNS[name].pattern.test(lineText)) {
        hits.push({ file, line, text: lineText.trim(), pattern: name });
        break;
      }
    }
  }
  return hits;
}

// Git exports the location of the repository it is running in to hooks (a
// pre-push hook in a linked worktree gets GIT_DIR). Inherited, that would
// point `git -C <dir>` at the hook's repository instead of <dir>.
const GIT_LOCATION_VARS = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"];

function git(repoRoot: string, args: string[]): string {
  const env = { ...process.env };
  for (const name of GIT_LOCATION_VARS) delete env[name];
  const result = spawnSync("git", ["-C", repoRoot, ...args], { encoding: "utf8", env, maxBuffer: 256 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(result.stderr.trim() || `git ${args[0]} exited with status ${result.status}`);
  return result.stdout;
}

export function extensionOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? "" : name.slice(dot);
}

/** Whether a repo-relative path is scanned. */
export function isScanned(path: string): boolean {
  if (!path.startsWith(SCAN_PREFIX)) return false;
  if (EXCLUDED_FILES.includes(path)) return false;
  if (isTestPath(path)) return false;
  return SCANNED_EXTENSIONS.has(extensionOf(path));
}

/**
 * Every repo-relative path git tracks, plus untracked files that are not
 * ignored. Throws outside a git checkout rather than returning an empty list,
 * which a caller would read as "nothing to find anywhere". `check` names the
 * calling check in that error.
 */
export function listRepoFiles(repoRoot: string, check: string): string[] {
  let output: string;
  try {
    output = git(repoRoot, ["ls-files", "-z", "--cached", "--others", "--exclude-standard"]);
  } catch (err) {
    throw new Error(
      `${check} needs a git checkout: it lists files with git so that build output stays out of the scan (${err instanceof Error ? err.message : String(err)})`,
    );
  }
  return [...new Set(output.split("\0").filter((path) => path !== ""))].sort();
}

/** Repo-relative paths in scope for the literal node-type comparison check. */
export function listScannedFiles(repoRoot: string = REPO): string[] {
  return listRepoFiles(repoRoot, "check-node-type-literals").filter(isScanned);
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
  const lines = hits.map((hit) => `  ${hit.file}:${hit.line}: ${hit.text}\n      ${PATTERNS[hit.pattern].summary}`);
  return [
    `${hits.length} literal node-type comparison${hits.length === 1 ? "" : "s"} (ADR-086 section 5):`,
    ...lines,
    "",
    "A comparison with a type name misses every type that extends it. Resolve the type through the registry instead:",
    "  Rust        store.type_is_a(node_type, CoreNodeType::X) / CoreNodeType::nearest(&chain)",
    "  SQL         crate::db::schema::is_a_sql(column, &[CoreNodeType::X])",
    "  TypeScript  isA(nodeType, 'x') from $lib/types/core-node-types",
    "Where a subtype must not match (the schema meta-type, a typed wire shape), say so: CoreNodeType::X.is_exactly(..), is_exactly_sql(..), isExactly(..).",
  ].join("\n");
}

if (import.meta.main) {
  const message = failureMessage(scan(listScannedFiles()));
  if (message) {
    console.error(message);
    process.exit(1);
  }
  console.log("✓ No literal node-type comparisons.");
}
