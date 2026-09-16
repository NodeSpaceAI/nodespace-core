#!/usr/bin/env bun
// Correlates a pre-push gate failure with commits on origin/main that the
// current branch does not contain.
//
// `check-branch-behind.ts` already warns when the branch is behind, but a
// bare "N commits behind origin/main" reads as routine mid-feature and says
// nothing about the failure printed alongside it. The case this exists for:
// a test fails locally, and the commit that fixes it is already on
// origin/main, unmerged into this branch. The failure is real output from
// stale code, so re-running or debugging it can never succeed — but nothing
// in the gate's output says so.
//
// This is NOT the semantic-conflict race check-branch-behind.ts documents
// (two branches each green, breaking main on merge). That one is unknowable
// at push time. This one is strictly easier: the failure is already visible
// and the fix already exists upstream — only the correlation is missing.
//
// Deliberately non-blocking and advisory, matching check-branch-behind.ts:
// the branch being behind is normal for a WIP push, and a path overlap is a
// strong hint, not proof. It never changes an exit code; the failure it
// annotates already blocks the push on its own.

import { $ } from "bun";
import { readdirSync } from "node:fs";

export interface UpstreamCommit {
  sha: string;
  subject: string;
}

/**
 * How the commits were matched. "exact" means a commit touches the very
 * file whose test failed; "package" means it only touches that file's
 * package, which is a materially weaker hint and is phrased as one.
 */
export type MatchScope = "exact" | "package";

export interface CorrelationResult {
  /** Paths parsed out of the failing stage's output. */
  failingPaths: string[];
  /** Upstream commits touching those paths (or their packages). */
  commits: UpstreamCommit[];
  /** How `commits` were matched. Absent when there are none. */
  matchScope?: MatchScope;
  /** Present only when the lookup could not run — why. */
  reason?: string;
}

// Vitest and cargo name failing files in a handful of shapes. Captured from
// real reporter output rather than guessed:
//
//   ❯ src/tests/unit/foo.test.ts (1 test | 1 failed) 1ms
//   FAIL  src/tests/unit/foo.test.ts > suite > case
//   ❯ src/tests/unit/foo.test.ts:5:15
//   error[E0425]: ... --> packages/core/src/db/mod.rs:12:9
//
// Each pattern must capture the path in group 1.
const PATH_PATTERNS: RegExp[] = [
  /^\s*(?:❯|×|✗)\s+(\S+\.(?:test|spec|e2e)\.[cm]?[jt]sx?)/gm,
  /^\s*FAIL\s+(\S+\.(?:test|spec|e2e)\.[cm]?[jt]sx?)/gm,
  /-->\s+(\S+\.rs):\d+:\d+/gm,
];

/**
 * Extracts distinct source paths named as failing in a stage's captured
 * output. Returns repo-relative paths as the reporter printed them; callers
 * resolve them against the repo, and a path that no longer resolves simply
 * matches no commits.
 */
export function parseFailingPaths(output: string): string[] {
  const found = new Set<string>();
  for (const pattern of PATH_PATTERNS) {
    // Patterns are module-level and /g, so lastIndex persists between calls.
    pattern.lastIndex = 0;
    for (const match of output.matchAll(pattern)) {
      const path = match[1]?.trim();
      if (path) found.add(path);
    }
  }
  return [...found];
}

/**
 * Widens a failing test path to the package that contains it.
 *
 * Matching only the failing file is precise but misses the motivating case:
 * the test was `watch-nodes.e2e.ts` while the fix was in `dev-proxy.ts`, a
 * different file in a different package. Widening to the package directory
 * catches a fix that landed near the behavior under test without widening
 * to the whole repo, where every upstream commit would match and the
 * warning would become noise.
 *
 * Returns the `packages/<name>` prefix when there is one, else the path's
 * top-level directory, else null when neither exists (a bare filename).
 */
export function packageScopeFor(path: string): string | null {
  const segments = path.split("/").filter(Boolean);
  if (segments[0] === "packages" && segments.length >= 2) {
    return `${segments[0]}/${segments[1]}`;
  }
  return segments.length >= 2 ? segments[0] : null;
}

/**
 * Packages whose vitest config sets cwd to the package, so their reporters
 * print package-relative paths (`src/tests/...`) while git needs
 * repo-relative ones (`packages/desktop-app/src/tests/...`). Cargo's `-->`
 * paths are already repo-relative and need no prefixing.
 *
 * Derived from the filesystem rather than hardcoded: a hardcoded list is
 * correct until someone adds a vitest config, at which point this check
 * goes quiet for that package's failures without anything failing to say
 * so. Falls back to the known two if the scan cannot run, so a sandbox
 * without directory access degrades instead of breaking.
 */
export function vitestPackagePrefixes(readPackages: () => string[] = defaultReadPackages): string[] {
  try {
    const found = readPackages();
    return found.length > 0 ? found : FALLBACK_PACKAGE_PREFIXES;
  } catch {
    return FALLBACK_PACKAGE_PREFIXES;
  }
}

const FALLBACK_PACKAGE_PREFIXES = ["packages/desktop-app", "packages/skill"];

function defaultReadPackages(): string[] {
  const packagesDir = new URL("../packages/", import.meta.url);
  return readdirSync(packagesDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .filter((entry) =>
      readdirSync(new URL(`${entry.name}/`, packagesDir)).some(
        (file) => file.startsWith("vitest") && file.endsWith(".config.ts")
      )
    )
    .map((entry) => `packages/${entry.name}`);
}

/**
 * Expands a reporter path into the candidate repo-relative paths it could
 * mean.
 *
 * A path that already starts with a known root (`packages/`, `scripts/`) is
 * taken as-is. Otherwise it is package-relative, and since the output alone
 * does not say which package produced it, every plausible prefix is offered.
 * A prefix that does not exist simply matches no commits — `git log` with a
 * pathspec that matches nothing is not an error — so guessing wide is safe
 * and cheap, while guessing narrow loses the warning entirely.
 */
export function candidatePathsFor(path: string, prefixes = vitestPackagePrefixes()): string[] {
  if (path.startsWith("packages/") || path.startsWith("scripts/")) return [path];
  return prefixes.map((prefix) => `${prefix}/${path}`);
}

/**
 * The exact repo-relative paths of the failing files — the high-confidence
 * scope. A commit touching the very file whose test failed is worth naming
 * on its own.
 */
export function scopesForPaths(paths: string[]): string[] {
  const scopes = new Set<string>();
  for (const path of paths) {
    for (const candidate of candidatePathsFor(path)) {
      scopes.add(candidate);
    }
  }
  return [...scopes];
}

/**
 * The packages containing the failing files — a deliberately weaker signal,
 * reported only when the exact files match nothing.
 *
 * Measured on this repo: across a 28-commit window, package-level matching
 * hit 11 commits while the exact failing file hit 1. Reporting the package
 * unconditionally would fire on most failures and train people to ignore
 * the warning — the precise failure mode this is meant to prevent. It still
 * earns its place as a fallback: the case that motivated this check had its
 * fix in the dev-proxy, a different file and package from the e2e test that
 * failed, which only a package-level match connects.
 */
export function packageScopesForPaths(paths: string[]): string[] {
  const scopes = new Set<string>();
  for (const path of paths) {
    for (const candidate of candidatePathsFor(path)) {
      const pkg = packageScopeFor(candidate);
      if (pkg) scopes.add(pkg);
    }
  }
  return [...scopes];
}

/**
 * Parses `git log --format=%H %s` output into commits. Tolerates blank
 * lines and a subject containing spaces; a line with no subject is kept
 * with an empty one rather than dropped, so a count never silently shrinks.
 */
export function parseCommitLog(rawOutput: string): UpstreamCommit[] {
  return rawOutput
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "")
    .map((line) => {
      const spaceAt = line.indexOf(" ");
      if (spaceAt === -1) return { sha: line, subject: "" };
      return { sha: line.slice(0, spaceAt), subject: line.slice(spaceAt + 1) };
    });
}

async function defaultLogUpstreamTouching(scopes: string[]): Promise<string> {
  // HEAD..origin/main — commits upstream has that this branch does not.
  // origin/main is already fetched by checkBranchBehind() earlier in the
  // gate, so this adds no network round-trip.
  return await $`git log --format=%H\ %s HEAD..origin/main -- ${scopes}`.text();
}

export interface CorrelateDeps {
  logUpstreamTouching?: (scopes: string[]) => Promise<string>;
}

/**
 * Finds upstream commits touching the code a failing stage named.
 *
 * Never throws: any git failure degrades to an empty result with a reason,
 * matching checkBranchBehind()'s contract. This runs while a push is
 * already failing — it must not be able to turn a clear test failure into a
 * confusing crash in the reporting layer.
 */
export async function correlateUpstreamFixes(
  output: string,
  deps: CorrelateDeps = {}
): Promise<CorrelationResult> {
  const logUpstreamTouching = deps.logUpstreamTouching ?? defaultLogUpstreamTouching;

  const failingPaths = parseFailingPaths(output);
  if (failingPaths.length === 0) {
    return { failingPaths: [], commits: [] };
  }

  try {
    // Exact-file matches first: a commit touching the failing file itself
    // is a strong enough signal to report on its own.
    const exact = parseCommitLog(await logUpstreamTouching(scopesForPaths(failingPaths)));
    if (exact.length > 0) {
      return { failingPaths, commits: exact, matchScope: "exact" };
    }

    // Nothing touched the file itself — widen to its package, which is
    // noisier and so is only consulted (and only phrased) as a weak hint.
    const packageScopes = packageScopesForPaths(failingPaths);
    if (packageScopes.length === 0) {
      return { failingPaths, commits: [] };
    }
    const byPackage = parseCommitLog(await logUpstreamTouching(packageScopes));
    if (byPackage.length === 0) {
      return { failingPaths, commits: [] };
    }
    return { failingPaths, commits: byPackage, matchScope: "package" };
  } catch (err) {
    return {
      failingPaths,
      commits: [],
      reason: `git log HEAD..origin/main failed: ${err instanceof Error ? err.message : String(err)}`,
    };
  }
}

/** Caps the commit list so a long-stale branch cannot flood the output. */
const MAX_LISTED = 5;

export function formatCorrelationWarning(result: CorrelationResult): string {
  const { commits, matchScope } = result;
  if (commits.length === 0) return "";

  const single = commits.length === 1;
  const commitWord = single ? "commit" : "commits";
  const verb = single ? "touches" : "touch";
  const pronoun = single ? "it" : "them";

  const listed = commits.slice(0, MAX_LISTED);
  const lines = listed.map((c) => `    ${c.sha.slice(0, 8)} ${c.subject}`);
  if (commits.length > listed.length) {
    lines.push(`    ... and ${commits.length - listed.length} more`);
  }

  // The two scopes differ enough in strength that they must not read alike:
  // an exact-file match is usually the answer, a package match is a lead.
  const headline =
    matchScope === "exact"
      ? `${commits.length} ${commitWord} on origin/main ${verb} the failing file itself,\n` +
        `  and your branch does not contain ${pronoun}:`
      : `${commits.length} ${commitWord} on origin/main ${verb} the failing file's package\n` +
        `  (not the file itself), and your branch does not contain ${pronoun}:`;

  const advice =
    matchScope === "exact"
      ? "  This failure is likely already fixed upstream. Rebase and re-run before\n" +
        "  investigating it as a live failure:  git rebase origin/main\n"
      : "  This is a weak hint, not a diagnosis — but if the failure looks unrelated\n" +
        "  to your changes, rebase and re-run first:  git rebase origin/main\n";

  return `\n⚠ ${headline}\n${lines.join("\n")}\n${advice}`;
}

/**
 * Runs the correlation for a failing stage and prints it when there is
 * something to say. Prints nothing when no upstream commit overlaps —
 * silence means the failure is the branch's own, which is the common case.
 */
export async function reportUpstreamFixes(
  output: string,
  deps: CorrelateDeps = {}
): Promise<CorrelationResult> {
  const result = await correlateUpstreamFixes(output, deps);
  const warning = formatCorrelationWarning(result);
  if (warning) console.warn(warning);
  return result;
}
