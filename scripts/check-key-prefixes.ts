#!/usr/bin/env bun
// Enforces ADR-112: a field a user adds to a core type is declared in the
// schema's `custom_fields` and stored under its own name. The `custom:`, `org:`
// and `plugin:` prefixes are gone, so no code matches a key by one of them,
// declares a key that carries one, or teaches one in the agent guidance.
//
// A key's status is a lookup in the schema (declared in `fields` or
// `custom_fields`, or `_`-prefixed), never a property of its spelling.
//
// Scope: every Rust, TypeScript and Svelte file under `packages/` and
// `scripts/`, tests included (a fixture that writes a prefixed key teaches the
// pattern back), plus the Markdown that is served to agents (the seeded skills
// and rules, the shipped skill and its references) and the contributor
// guides. A line comment is out of scope in code. This is a hard check with
// no baseline: a hit fails.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { extensionOf, listRepoFiles, REPO } from "./check-node-type-literals";

export { REPO };

const PREFIX = "(?:custom|org|plugin)";
const QUOTE = "[\"'`]";

export const CODE_PATTERNS = {
  // `starts_with("custom:")`, `strip_prefix("org:")`, `["custom:", "org:"]`:
  // a prefix literal on its own, which is only ever matched against a key.
  prefixLiteral: {
    pattern: new RegExp(`${QUOTE}${PREFIX}:${QUOTE}`),
    summary: "a key prefix literal (custom:, org: or plugin:)",
  },
  // `"custom:effort"`, `` `custom:${name}` ``, `format!("custom:{key}")`: a key
  // that declares or builds a prefixed name. A Tauri command (`plugin:app|version`)
  // is followed by `|` and is not a key.
  prefixedKey: {
    pattern: new RegExp(`${QUOTE}${PREFIX}:(?:[A-Za-z_][A-Za-z0-9_]*|\\$?\\{[^}]*\\})${QUOTE}`),
    summary: "a key carrying a custom:, org: or plugin: prefix",
  },
  prefixedKeyBuilt: {
    pattern: new RegExp(`${QUOTE}${PREFIX}:\\$?\\{`),
    summary: "a key built with a custom:, org: or plugin: prefix",
  },
} satisfies Record<string, { pattern: RegExp; summary: string }>;

// Guidance teaches the prefix in prose: `custom:` in backticks, or a prefixed
// name written out.
export const MARKDOWN_PATTERNS = {
  prefixTaught: {
    pattern: new RegExp(`\`${PREFIX}:|\\b${PREFIX}:[a-z_]+\\b|\\b(?:custom|org|plugin)/(?:custom|org|plugin):|${PREFIX}:/`),
    summary: "guidance that teaches a custom:, org: or plugin: prefix",
  },
} satisfies Record<string, { pattern: RegExp; summary: string }>;

export type PatternName = keyof typeof CODE_PATTERNS | keyof typeof MARKDOWN_PATTERNS;
const SUMMARIES: Record<string, string> = Object.fromEntries(
  [...Object.entries(CODE_PATTERNS), ...Object.entries(MARKDOWN_PATTERNS)].map(([name, { summary }]) => [name, summary]),
);

export interface Hit {
  file: string;
  line: number;
  text: string;
  pattern: PatternName;
}

const COMMENT_LINE = /^\s*(?:\/\/|\/\*|\*)/;
const CODE_EXTENSIONS: ReadonlySet<string> = new Set([".rs", ".ts", ".svelte"]);
const MARKDOWN_ROOTS: readonly string[] = [
  "packages/agent/src/seeds/",
  "packages/skill/SKILL.md",
  "packages/skill/references/",
  "AGENTS.md",
  "CLAUDE.md",
];

/** This check and its own test name the prefixes by design. */
export const EXCLUDED_FILES: readonly string[] = ["scripts/check-key-prefixes.ts", "scripts/check-key-prefixes.test.ts"];

/** Whether a repo-relative path is scanned, and as what. */
export function kindOf(path: string): "code" | "markdown" | null {
  if (EXCLUDED_FILES.includes(path)) return null;
  const ext = extensionOf(path);
  if ((path.startsWith("packages/") || path.startsWith("scripts/")) && CODE_EXTENSIONS.has(ext)) return "code";
  if (ext === ".md" && MARKDOWN_ROOTS.some((root) => path === root || path.startsWith(root))) return "markdown";
  return null;
}

/** Every hit in one file's text. */
export function findHits(file: string, text: string): Hit[] {
  const kind = kindOf(file) ?? (extensionOf(file) === ".md" ? "markdown" : "code");
  const hits: Hit[] = [];
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (kind === "code") {
      if (COMMENT_LINE.test(line)) continue;
      const name = (Object.keys(CODE_PATTERNS) as (keyof typeof CODE_PATTERNS)[]).find((n) => CODE_PATTERNS[n].pattern.test(line));
      if (name) hits.push({ file, line: i + 1, text: line.trim(), pattern: name });
    } else if (MARKDOWN_PATTERNS.prefixTaught.pattern.test(line)) {
      hits.push({ file, line: i + 1, text: line.trim().slice(0, 160), pattern: "prefixTaught" });
    }
  }
  return hits;
}

/** Repo-relative paths in scope. Throws outside a git checkout. */
export function listScannedFiles(repoRoot: string = REPO): string[] {
  return listRepoFiles(repoRoot, "check-key-prefixes").filter((path) => kindOf(path) !== null);
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
    `${hits.length} key prefix use${hits.length === 1 ? "" : "s"} (ADR-112):`,
    ...lines,
    "",
    "A field a user adds to a core type is declared in the schema's custom_fields and stored under its own name.",
    "Whether a key is allowed is a lookup in the schema (fields, custom_fields, or a `_` bookkeeping key), never a match on its spelling.",
  ].join("\n");
}

if (import.meta.main) {
  const message = failureMessage(scan(listScannedFiles()));
  if (message) {
    console.error(message);
    process.exit(1);
  }
  console.log("✓ No custom:/org:/plugin: key prefixes in code or agent guidance.");
}
