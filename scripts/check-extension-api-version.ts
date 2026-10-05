#!/usr/bin/env bun
/**
 * The extension API's version check (ADR-082 section 8), run by the push check
 * and the merge gate's lint section (scripts/test-gate.ts).
 *
 * An app built on core reads `EXTENSION_API_VERSION` to learn that core's
 * extension API changed, so the change that alters the API bumps the version
 * in the same change. Two kinds of file record the API, and this check fails
 * when one changes and the version does not:
 *
 * - the host API's surface snapshot, which the frontend surface test keeps
 *   equal to the host API's exports and type declarations;
 * - the fixtures that use each extension point with its full signature: the
 *   Rust extension points' (ADR-082 section 9) and the skill-extension
 *   input's accepted layout (ADR-082 section 6).
 *
 * "Changed" is the diff from the merge-base with origin/main to HEAD: what a
 * push publishes, and in the merge gate the stack of queued PRs. Uncommitted
 * edits are not pushed, so they count on neither side.
 *
 * It also fails when the Rust and TypeScript constants disagree, when the
 * version goes backwards, and when an entry in WATCHED_PATHS matches no file,
 * so a moved fixture cannot leave it watching nothing. It reads git alone (no
 * build, no tests), so it takes seconds.
 *
 * Run standalone: bun run scripts/check-extension-api-version.ts
 */

import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");

export interface ApiVersion {
  major: number;
  minor: number;
}

/** Where the Rust constant is declared; release notes read it too. */
export const RUST_VERSION_FILE = "packages/desktop-app/app-lib/src/extensions/mod.rs";

/** Where the TypeScript constant is declared. */
export const TS_VERSION_FILE = "packages/desktop-app/src/lib/plugins/ui-extensions.ts";

/**
 * The files that record the extension API: a change to one needs a version
 * bump. An entry ending in `/` is a directory and covers every file under it.
 * A new fixture of an extension point is added here in the change that adds it.
 */
export const WATCHED_PATHS: readonly string[] = [
  // The host API's surface snapshot: its exports and a hash of its types.
  "packages/desktop-app/src/tests/extension-api/extension-api-surface.json",
  // The Tauri extension points, driven through `assemble` and a real daemon.
  "packages/desktop-app/app-lib/src/extensions/fixture_tests.rs",
  "packages/desktop-app/app-lib/tests/it/extension_hooks_test.rs",
  "packages/desktop-app/app-lib/tests/it/extension_daemon_profile_test.rs",
  // The layout NODESPACE_SKILL_EXTENSIONS accepts.
  "scripts/fixtures/skill-extension/",
];

/** The command that re-records the surface snapshot after a bump. */
const RERECORD_COMMAND = "UPDATE_EXTENSION_API_SURFACE=1 bun run --cwd packages/desktop-app test src/tests/extension-api";

const RUST_DECLARATION = /^pub const EXTENSION_API_VERSION:\s*\(u32,\s*u32\)\s*=\s*\(\s*(\d+)\s*,\s*(\d+)\s*\)\s*;/m;
const TS_DECLARATION = /^export const EXTENSION_API_VERSION\s*=\s*\{\s*major:\s*(\d+)\s*,\s*minor:\s*(\d+)\s*\}\s*as const\s*;/m;

function parseWith(pattern: RegExp, source: string): ApiVersion | null {
  const match = pattern.exec(source);
  return match === null ? null : { major: Number(match[1]), minor: Number(match[2]) };
}

/** The version in `pub const EXTENSION_API_VERSION: (u32, u32) = (M, N);`, or null when there is none. */
export function parseRustVersion(source: string): ApiVersion | null {
  return parseWith(RUST_DECLARATION, source);
}

/** The version in `export const EXTENSION_API_VERSION = { major: M, minor: N } as const;`, or null. */
export function parseTsVersion(source: string): ApiVersion | null {
  return parseWith(TS_DECLARATION, source);
}

export function formatVersion(version: ApiVersion): string {
  return `${version.major}.${version.minor}`;
}

/** Negative when `a` is older than `b`, zero when equal, positive when newer. */
export function compareVersions(a: ApiVersion, b: ApiVersion): number {
  return a.major !== b.major ? a.major - b.major : a.minor - b.minor;
}

/** Whether `file` is, or lies under, one of `watched`. */
export function isWatched(file: string, watched: readonly string[] = WATCHED_PATHS): boolean {
  return watched.some((entry) => (entry.endsWith("/") ? file.startsWith(entry) : file === entry));
}

/** What the check judges, read from git by readVersionState. */
export interface VersionState {
  /** Files changed from the merge-base with origin/main to HEAD. */
  changedFiles: readonly string[];
  /** The Rust constant at the merge-base; null when that revision declares none. */
  base: ApiVersion | null;
  /** Both constants at HEAD; null where one is missing or unreadable. */
  head: { rust: ApiVersion | null; ts: ApiVersion | null };
  /** WATCHED_PATHS entries that match no file at HEAD. */
  unmatchedWatched: readonly string[];
}

/** One actionable message per problem; empty when the change is versioned correctly. */
export function versionProblems(state: VersionState): string[] {
  const problems = state.unmatchedWatched.map(
    (entry) =>
      `WATCHED_PATHS in scripts/check-extension-api-version.ts lists ${entry}, which matches no file. ` +
      "Point the entry at the fixture's new path, or remove it along with the fixture."
  );
  const { rust, ts } = state.head;
  if (rust === null) problems.push(`No \`pub const EXTENSION_API_VERSION: (u32, u32) = (M, N);\` in ${RUST_VERSION_FILE}.`);
  if (ts === null) problems.push(`No \`export const EXTENSION_API_VERSION = { major: M, minor: N } as const;\` in ${TS_VERSION_FILE}.`);
  if (rust === null || ts === null) return problems;

  if (compareVersions(rust, ts) !== 0) {
    problems.push(
      `EXTENSION_API_VERSION is ${formatVersion(rust)} in ${RUST_VERSION_FILE} but ${formatVersion(ts)} in ${TS_VERSION_FILE}. ` +
        "One version covers both surfaces: change the two together."
    );
  }
  const base = state.base;
  if (base === null) return problems;
  if (compareVersions(rust, base) < 0) {
    problems.push(`EXTENSION_API_VERSION went from ${formatVersion(base)} back to ${formatVersion(rust)}; it only moves forward.`);
  }
  const watchedChanges = state.changedFiles.filter((file) => isWatched(file));
  if (watchedChanges.length > 0 && compareVersions(rust, base) === 0) {
    problems.push(
      [
        `The extension API changed but EXTENSION_API_VERSION is still ${formatVersion(base)} (ADR-082 section 8). ` +
          "Changed since the merge-base with origin/main:",
        ...watchedChanges.map((file) => `  ${file}`),
        `Bump it in the same change, in ${RUST_VERSION_FILE} and ${TS_VERSION_FILE} together: major for a removal, ` +
          "rename, retype or changed semantics, minor for an addition. Every change to these files needs a bump; " +
          "review decides major or minor. Then re-record the surface snapshot:",
        `  ${RERECORD_COMMAND}`,
      ].join("\n")
    );
  }
  return problems;
}

// Git exports the location of the repository it is running in to hooks (a
// pre-push hook in a linked worktree gets GIT_DIR). Inherited, that would
// point every `git -C <dir>` below at the hook's repository instead of <dir>.
const GIT_LOCATION_VARS = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"];

/** Runs git in `repoRoot`; null when it exits non-zero. */
function gitOrNull(repoRoot: string, args: string[]): string | null {
  const env = { ...process.env };
  for (const name of GIT_LOCATION_VARS) delete env[name];
  const result = spawnSync("git", ["-C", repoRoot, ...args], { encoding: "utf8", env, maxBuffer: 64 * 1024 * 1024 });
  if (result.error) throw result.error;
  return result.status === 0 ? result.stdout : null;
}

function git(repoRoot: string, args: string[]): string {
  const out = gitOrNull(repoRoot, args);
  if (out === null) throw new Error(`git ${args.join(" ")} failed`);
  return out;
}

function nulSeparated(out: string): string[] {
  return out.split("\0").filter((path) => path !== "");
}

/** Reads the state versionProblems judges from the repository at `repoRoot`. Throws when git cannot give it. */
export function readVersionState(repoRoot: string = REPO): VersionState {
  const base = git(repoRoot, ["merge-base", "origin/main", "HEAD"]).trim();
  // --no-renames: a rename is listed by its destination alone otherwise, and
  // moving a watched file away from its path changes it as much as an edit.
  const changedFiles = nulSeparated(git(repoRoot, ["diff", "-z", "--name-only", "--no-renames", base, "HEAD"]));
  const show = (revision: string, path: string): string | null => gitOrNull(repoRoot, ["show", `${revision}:${path}`]);
  const at = (revision: string, path: string, parse: (source: string) => ApiVersion | null): ApiVersion | null => {
    const source = show(revision, path);
    return source === null ? null : parse(source);
  };
  const headFiles = nulSeparated(git(repoRoot, ["ls-tree", "-r", "-z", "--name-only", "HEAD"]));
  return {
    changedFiles,
    base: at(base, RUST_VERSION_FILE, parseRustVersion),
    head: { rust: at("HEAD", RUST_VERSION_FILE, parseRustVersion), ts: at("HEAD", TS_VERSION_FILE, parseTsVersion) },
    unmatchedWatched: WATCHED_PATHS.filter((entry) => !headFiles.some((file) => isWatched(file, [entry]))),
  };
}

/** The check's problems for the repository at `repoRoot`. A diff git cannot give fails, rather than passing unchecked. */
export function checkExtensionApiVersion(repoRoot: string = REPO): string[] {
  let state: VersionState;
  try {
    state = readVersionState(repoRoot);
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    return [`Could not compare this branch with origin/main (${reason}). Run \`git fetch origin\` and try again.`];
  }
  return versionProblems(state);
}

if (import.meta.main) {
  const problems = checkExtensionApiVersion();
  for (const problem of problems) console.error(`\n❌ ${problem}`);
  if (problems.length > 0) process.exit(1);
  console.log("✅ EXTENSION_API_VERSION accounts for every change to the extension API's snapshot and fixtures (ADR-082 section 8).");
}
