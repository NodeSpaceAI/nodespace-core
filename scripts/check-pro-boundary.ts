#!/usr/bin/env bun
// Holds the Pro / sync boundary from ADR-081: core is the complete free
// product and ships no Pro code, because nodespace-sync builds and owns the
// Pro app. The existing Pro code is being moved out; until it is gone this
// check freezes it, and makes every move lower a number.
//
// It counts Pro/sync markers (see MARKERS) line by line across `packages/`,
// `scripts/` and the root README, plus the files whose basename has a `pro`
// segment. Each count has a ceiling in BASELINES, and the check fails only when
// a count rises above its ceiling: that is new Pro code. A count below its
// ceiling does not fail. Many removal changes land in parallel, and failing on
// a decrease would make every merge force every other open change to rebase
// just to edit BASELINES. Instead the check prints a notice with the exact
// tightened block to paste, so anyone can lower the ceilings.
//
// The one-way ratchet is loose only while Pro code remains: headroom between a
// count and its ceiling can be spent by new Pro code until someone tightens
// it, so tighten in the change that removes code whenever it is cheap. At the
// end state every ceiling is 0, and a ceiling of 0 fails on any hit, which is
// the strict check. A change that must temporarily raise a ceiling edits
// BASELINES in its own diff and justifies it in its description; ceilings are
// never raised to land new Pro code. Headroom below a ceiling is not permission
// either: a change that adds Pro lines needs an accepted class whether or not
// the count still fits under its ceiling.
//
// Only three kinds of change may raise a ceiling, and tests that assert Pro
// code is absent must build their needle from fragments (or carry a per-file
// EXEMPTIONS entry) so they do not add the marker they guard against. Both
// rules live in CLAUDE.md, under "Pro / Sync Boundary". This file only
// enforces them.
//
// Exemptions are per file and per marker, and only two kinds of file may carry
// one: test files, and the installer recognition points (EXEMPTIBLE_NON_TEST_FILES).
// ADR-084 (section 5) lists three such files and ADR-081 (sections 2d and 8)
// lets core keep them: the installation check in build-pkg.sh, the .pkg
// preinstall script and the cask preflight. They must name other NodeSpace
// products to refuse installing over them. The .pkg postinstall script is also
// allowlisted for now, but ADR-084 does not list it: its old guard moves to the
// pre-install stage, and the slot goes when that guard does. An exemption on any
// other non-test file is a problem exemptionProblems reports; such a file builds
// the name from fragments or drops the wording.
//
// Files come from `git ls-files`, not a filesystem walk. A walk also reads
// gitignored output: the staged copy of the skill package under the Tauri
// resources directory would add machine-dependent lines, and the Pro daemon
// binaries that Pro builds drop next to the sidecar would be scanned too.
// What git tracks, plus untracked files that are not ignored, is what a
// reviewer sees in a diff.
//
// Each baseline sits under its own one-line comment, and the printed block
// reproduces that layout exactly. The merge queue replays queued PRs with
// cherry-picks and ejects one that conflicts, and git treats edits on
// adjacent lines as a conflict. With a comment line between every pair of
// values, only PRs that change the same marker conflict.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

export const REPO = join(dirname(new URL(import.meta.url).pathname), "..");

// Every path under these prefixes is in scope, so a package added later enters
// the ratchet automatically. That includes packages/agent, which is counted
// only; the check never modifies anything.
export const SCAN_PREFIXES: readonly string[] = ["packages/", "scripts/"];
export const SCAN_ROOT_FILES: readonly string[] = ["README.md"];

// Pro markers live in more than source code: the vendored proto, the skill
// docs, Tauri and Cargo config, installer scripts. Paths with no extension
// (the installer's postinstall) are scanned too.
export const SCANNED_EXTENSIONS: ReadonlySet<string> = new Set([
  ".rs",
  ".ts",
  ".svelte",
  ".js",
  ".proto",
  ".toml",
  ".md",
  ".json",
  ".sh",
  ".plist",
  ".css",
  ".html",
  ".py",
]);

// This checker's own source and test necessarily name the patterns they scan
// for, and the checker's own name has a `pro` segment.
export const EXCLUDED_FILES: readonly string[] = ["scripts/check-pro-boundary.ts", "scripts/check-pro-boundary.test.ts"];

// Paths an EXEMPTIONS entry may name. Inline Rust test modules in product
// files do not match, so they use fragments.
export const TEST_PATH = /(^|\/)(tests?|__tests__)\/|\.(test|spec)\.ts$|_tests?\.rs$|(^|\/)tests\.rs$/;

// The only non-test files an EXEMPTIONS entry may name: the installer
// recognition points. ADR-084 (section 5) lists build-pkg.sh, preinstall (a new
// script that work adds) and update-homebrew-cask.ts, and ADR-081 (sections 2d
// and 8) lets core keep them: the installer and cask guards must be able to name
// other NodeSpace products in order to refuse installing over them, so that
// naming is the file's job rather than Pro code. postinstall is not in ADR-084's
// list: it still carries the old post-payload guard that ADR-084 moves to the
// pre-install stage, so it is allowlisted only until that guard is gone, and
// should be dropped from this list then. Whole paths, not basenames, so a file
// of the same name elsewhere is not covered. Any other non-test file uses
// fragments or moves the Pro wording out.
export const EXEMPTIBLE_NON_TEST_FILES: readonly string[] = [
  "scripts/update-homebrew-cask.ts",
  "scripts/pkg-resources/preinstall",
  "scripts/pkg-resources/postinstall",
  "scripts/build-pkg.sh",
];

// The pattern is tested once per line, so a line counts at most once per
// marker, and one line can count under several markers. The summary is the
// one-line comment printed above the marker's baseline.
export const MARKERS = {
  proCommands: {
    pattern: /\bpro_[a-z][a-z0-9_]*/,
    summary: "pro_* Tauri commands, the pro_sync / pro_client modules, pro_env",
  },
  proSyncModule: {
    pattern: /[pP]roSync|pro-sync/,
    summary: "the proSync store, resolveProSyncVariant, isProSyncActive, pro-sync imports",
  },
  proProtocol: {
    pattern: /nodespace_pro\b|nodespace\.pro\.v1|CloudSyncService|\bPro(?:Client|Tier)\b|pro\.nodespace\.ai|127\.0\.0\.1:8787/,
    summary: "the Pro proto, CloudSyncService, ProClient / ProTier, Pro Worker URLs",
  },
  proEvents: {
    pattern: /pro:tier-detected|['"`]sync:(?:status|error)['"`]/,
    summary: "quoted pro:tier-detected, sync:status and sync:error event names",
  },
  membershipService: {
    pattern: /membershipService|MembershipService|membership-service/,
    summary: "the Pro membership service",
  },
  editionBranching: {
    pattern:
      /\bis_pro(?:_build)?\b|NODESPACED?_PRO_|feature\s*=\s*"pro"|nodespaced-pro|(?:daemon|ui|incompatible-database)(?:-dev)?-pro\.(?:sock|pid|json)|PRO_DAEMON_BINARY_NAME|tauri\.pro\.conf|\.daemon(?:\.dev)?\.pro\b|--edition\b(?!\s*=?\s*20\d\d)|FORCE_COMMUNITY/,
    summary: "Pro build, daemon, socket, launchd and installer branching",
  },
  cloudBindState: {
    pattern: /bound_?[tT]enant|[bB]oundTenant|sync_?[eE]nabled|[sS]yncEnabled|auth_?[sS]tatus|[aA]uthStatus/,
    summary: "tenant binding, sync_enabled and auth_status state",
  },
  cloudSyncHooks: {
    pattern: /apply_remote_embeddings|embeddings_modified_since|get_multi_membership_edges/,
    summary: "cloud embedding and membership-edge sync hooks",
  },
  cloudWording: {
    pattern: /[Ss]upabase|\bRLS\b|pgvector|nodespace[-_]sync|[Pp]ro daemon/,
    summary: 'Supabase, RLS, pgvector, nodespace-sync and "Pro daemon" wording',
  },
  tenantWording: {
    pattern: /tenant/i,
    summary: "the word tenant (one fixture exempt, see EXEMPTIONS)",
  },
  proWording: {
    pattern: /(?<!\b(?:M\d|V\d|MacBook|iPad|iPhone) )\bPro\b/,
    summary: 'the word Pro, except chip and model names such as "M2 Pro", "DeepSeek V4 Pro"',
  },
} satisfies Record<string, { pattern: RegExp; summary: string }>;

export type LineMarkerName = keyof typeof MARKERS;
export type MarkerName = LineMarkerName | "proNamedFiles";

const LINE_MARKER_NAMES = Object.keys(MARKERS) as LineMarkerName[];
const MARKER_NAMES: readonly MarkerName[] = [...LINE_MARKER_NAMES, "proNamedFiles"];

const PRO_NAMED_FILE = /(^|[-_.])pro([-_.]|$)/;
const PRO_NAMED_SUMMARY = "files whose basename has a pro segment";

function summaryOf(marker: MarkerName): string {
  return marker === "proNamedFiles" ? PRO_NAMED_SUMMARY : MARKERS[marker].summary;
}

function isLineMarker(name: string): name is LineMarkerName {
  return Object.hasOwn(MARKERS, name);
}

/** Whether the file's basename has a `pro` segment, as in `pro-plugin.ts` or `tauri.pro.conf.json`. */
export function isProNamedFile(path: string): boolean {
  return PRO_NAMED_FILE.test(path.slice(path.lastIndexOf("/") + 1));
}

// One narrow, per-file, per-marker exemption for a test file, or an installer
// recognition point (EXEMPTIBLE_NON_TEST_FILES), whose Pro-looking text is not
// Pro code. It hides only its own markers, only in its own file.
// `proNamedFiles` can't be exempted: an absence test must not have a
// Pro-named file name.
export type Exemption = { file: string; markers: readonly LineMarkerName[]; reason: string };

export const EXEMPTIONS: readonly Exemption[] = [
  {
    file: "packages/agent/tests/it/live_embedding_prefix_measurement.rs",
    markers: ["tenantWording"],
    reason: 'lease-agreement fixture prose ("a replacement tenant"), not tenant code',
  },
];

// Ratchet ceilings. See the file-level comment: a count above its ceiling fails,
// a count below it prints a notice, and a ceiling is never raised to land Pro
// code. The layout is formatBaselines(BASELINES); a test holds it there.
export const BASELINES = {
  // pro_* Tauri commands, the pro_sync / pro_client modules, pro_env
  proCommands: 275,
  // the proSync store, resolveProSyncVariant, isProSyncActive, pro-sync imports
  proSyncModule: 368,
  // the Pro proto, CloudSyncService, ProClient / ProTier, Pro Worker URLs
  proProtocol: 95,
  // quoted pro:tier-detected, sync:status and sync:error event names
  proEvents: 81,
  // the Pro membership service
  membershipService: 54,
  // Pro build, daemon, socket, launchd and installer branching
  editionBranching: 113,
  // tenant binding, sync_enabled and auth_status state
  cloudBindState: 203,
  // cloud embedding and membership-edge sync hooks
  cloudSyncHooks: 4,
  // Supabase, RLS, pgvector, nodespace-sync and "Pro daemon" wording
  cloudWording: 72,
  // the word tenant (one fixture exempt, see EXEMPTIONS)
  tenantWording: 463,
  // the word Pro, except chip and model names such as "M2 Pro", "DeepSeek V4 Pro"
  proWording: 281,
  // files whose basename has a pro segment
  proNamedFiles: 18,
} satisfies Record<MarkerName, number>;

export type MarkerCounts = Record<MarkerName, number>;
export type MarkerHits = Record<MarkerName, string[]>;

// Git exports the location of the repository it is running in to hooks (a
// pre-push hook in a linked worktree gets GIT_DIR). Inherited, that would
// point every `git -C <dir>` below at the hook's repository instead of <dir>.
const GIT_LOCATION_VARS = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"];

function git(repoRoot: string, args: string[]): string {
  const env = { ...process.env };
  for (const name of GIT_LOCATION_VARS) delete env[name];
  const result = spawnSync("git", ["-C", repoRoot, ...args], { encoding: "utf8", env, maxBuffer: 256 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(result.stderr.trim() || `git ${args[0]} exited with status ${result.status}`);
  return result.stdout;
}

/** The extension is the part of the basename from its last `.`, unless that `.` is the first character. */
function extensionOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? "" : name.slice(dot);
}

function isScanned(path: string): boolean {
  if (EXCLUDED_FILES.includes(path)) return false;
  if (SCAN_ROOT_FILES.includes(path)) return true;
  if (!SCAN_PREFIXES.some((prefix) => path.startsWith(prefix))) return false;
  const extension = extensionOf(path);
  return extension === "" || SCANNED_EXTENSIONS.has(extension);
}

/**
 * Repo-relative paths in scope: what git tracks plus untracked files that are
 * not ignored, filtered by the file selection rules. Throws outside a git
 * checkout rather than returning an empty list, which would read as "no Pro
 * code anywhere".
 */
export function listScannedFiles(repoRoot: string = REPO): string[] {
  let output: string;
  try {
    output = git(repoRoot, ["ls-files", "-z", "--cached", "--others", "--exclude-standard"]);
  } catch (err) {
    throw new Error(
      `check-pro-boundary needs a git checkout: it lists files with git so that gitignored build output stays out of the counts (${err instanceof Error ? err.message : String(err)})`,
    );
  }
  return [...new Set(output.split("\0").filter((path) => path !== "" && isScanned(path)))].sort();
}

function readText(repoRoot: string, file: string): string | null {
  try {
    return readFileSync(join(repoRoot, file), "utf8");
  } catch {
    return null;
  }
}

function emptyHits(): MarkerHits {
  return Object.fromEntries(MARKER_NAMES.map((name) => [name, []])) as unknown as MarkerHits;
}

function exemptMarkers(exemptions: readonly Exemption[], file: string): ReadonlySet<string> {
  return new Set(exemptions.filter((entry) => entry.file === file).flatMap((entry) => entry.markers));
}

/**
 * Counts every marker over the given files. A pure function of the files'
 * contents, with no baseline comparison, so it is testable against fixtures.
 * A file that can't be read (still in the index, deleted from the working
 * tree) is skipped entirely, `proNamedFiles` included.
 */
export function countMarkers(
  files: readonly string[],
  repoRoot: string = REPO,
  exemptions: readonly Exemption[] = EXEMPTIONS,
): { counts: MarkerCounts; hits: MarkerHits } {
  const hits = emptyHits();
  for (const file of files) {
    const text = readText(repoRoot, file);
    if (text === null) continue;
    if (isProNamedFile(file)) hits.proNamedFiles.push(file);
    const exempt = exemptMarkers(exemptions, file);
    const active = LINE_MARKER_NAMES.filter((name) => !exempt.has(name));
    if (active.length === 0) continue;
    text.split("\n").forEach((line, index) => {
      for (const name of active) {
        if (MARKERS[name].pattern.test(line)) hits[name].push(`${file}:${index + 1}: ${line.trim()}`);
      }
    });
  }
  const counts = Object.fromEntries(MARKER_NAMES.map((name) => [name, hits[name].length])) as MarkerCounts;
  return { counts, hits };
}

/**
 * One message per problem with an EXEMPTIONS entry. An entry that has stopped
 * hiding anything is a problem: left in place, it would silently hide the next
 * real hit in that file.
 */
export function exemptionProblems(exemptions: readonly Exemption[] = EXEMPTIONS, repoRoot: string = REPO): string[] {
  const scanned = new Set(listScannedFiles(repoRoot));
  const problems: string[] = [];
  for (const entry of exemptions) {
    const label = `EXEMPTIONS entry for ${entry.file}`;
    const inScan = scanned.has(entry.file);
    if (!inScan) {
      problems.push(`${label}: the file is missing from the scan (deleted, ignored, or outside the scanned paths). Remove the entry.`);
    }
    if (!TEST_PATH.test(entry.file) && !EXEMPTIBLE_NON_TEST_FILES.includes(entry.file)) {
      problems.push(
        `${label}: only test files (TEST_PATH) and the installer recognition points (EXEMPTIBLE_NON_TEST_FILES) may be exempted. Use fragments in anything else.`,
      );
    }
    if (entry.reason.trim() === "") problems.push(`${label}: the reason is empty.`);
    if (entry.markers.length === 0) problems.push(`${label}: it names no marker.`);
    const lines = inScan ? (readText(repoRoot, entry.file)?.split("\n") ?? []) : [];
    for (const marker of entry.markers) {
      if (!isLineMarker(marker)) {
        problems.push(`${label}: "${String(marker)}" is not a marker that can be exempted.`);
      } else if (inScan && !lines.some((line) => MARKERS[marker].pattern.test(line))) {
        problems.push(`${label}: ${marker} has no hit left in the file, so the exemption is stale. Remove it.`);
      }
    }
  }
  return problems;
}

/**
 * Files this branch changed from the merge-base with origin/main to the
 * working tree, plus untracked files that are not ignored. Best effort: any
 * error gives `[]`, because it only orders the hit list in a failure message.
 */
export function changedFilesSinceMain(repoRoot: string = REPO): string[] {
  try {
    const base = git(repoRoot, ["merge-base", "origin/main", "HEAD"]).trim();
    // --no-renames: a rename is listed by its destination alone otherwise, and
    // the path it left can matter as much as the one it arrived at.
    const changed = git(repoRoot, ["diff", "-z", "--name-only", "--no-renames", base]);
    const untracked = git(repoRoot, ["ls-files", "-z", "--others", "--exclude-standard"]);
    return [...new Set(`${changed}\0${untracked}`.split("\0").filter((path) => path !== ""))];
  } catch {
    return [];
  }
}

/**
 * The BASELINES literal for any set of counts: one summary comment line and
 * one value line per marker, in MARKERS order, then `proNamedFiles`. The
 * source of this file holds `formatBaselines(BASELINES)` verbatim, so pasting
 * a printed block changes only the numbers that moved.
 */
export function formatBaselines(counts: MarkerCounts): string {
  const lines = ["export const BASELINES = {"];
  for (const name of MARKER_NAMES) {
    lines.push(`  // ${summaryOf(name)}`, `  ${name}: ${counts[name]},`);
  }
  lines.push("} satisfies Record<MarkerName, number>;");
  return lines.join("\n");
}

const RAISE_GUIDANCE =
  "Pro and sync code belongs in nodespace-sync (ADR-081); core may only expose a generic extension point. " +
  "A test that asserts Pro code is absent builds its needle from fragments or gets an EXEMPTIONS entry. " +
  "Only the accepted raise classes in CLAUDE.md ('Pro / Sync Boundary') may raise a baseline, and the PR description must name them.";

/** The file a hit belongs to: `path:line: text` for a line marker, the bare path for `proNamedFiles`. */
function hitFile(hit: string): string {
  return /^(.*?):\d+: /.exec(hit)?.[1] ?? hit;
}

/**
 * One actionable message per marker whose count is above its ceiling; a count
 * at or below its ceiling never fails. `hits` is what `countMarkers` returns
 * beside the counts; it feeds the hit listing, which puts hits in changed
 * files first.
 */
export function baselineFailures(
  counts: MarkerCounts,
  baselines: MarkerCounts = BASELINES,
  changedFiles: readonly string[] = [],
  hits: Partial<Record<MarkerName, readonly string[]>> = {},
): string[] {
  const changed = new Set(changedFiles);
  const failures: string[] = [];
  for (const name of MARKER_NAMES) {
    const count = counts[name];
    const baseline = baselines[name];
    if (count <= baseline) continue;
    const lines = [`${name} (${summaryOf(name)}): ${count}, above its baseline of ${baseline}.`];
    if (baseline === 0) lines.push(`${name} is fully removed from core, so no hit may appear.`);
    lines.push(RAISE_GUIDANCE);
    lines.push(
      `Under an accepted raise class only, set \`BASELINES.${name}\` to ${count} in scripts/check-pro-boundary.ts in this change and justify it in the PR description.`,
    );
    const all = hits[name] ?? [];
    const inChangedFiles = all.filter((hit) => changed.has(hitFile(hit)));
    if (inChangedFiles.length > 0) lines.push("In files this branch changed:", ...inChangedFiles);
    if (all.length > 0) lines.push("All hits:", ...all);
    failures.push(lines.join("\n"));
  }
  return failures;
}

/** Each ceiling lowered to its count where the count is below it. A ceiling is never raised. */
function tightenedBaselines(counts: MarkerCounts, baselines: MarkerCounts): MarkerCounts {
  return Object.fromEntries(MARKER_NAMES.map((name) => [name, Math.min(counts[name], baselines[name])])) as MarkerCounts;
}

/**
 * The non-failing counterpart of `baselineFailures`: one notice per marker
 * whose count is below its ceiling, then the exact block to paste over
 * BASELINES. The block lowers only the markers that are below, and leaves every
 * other ceiling as it is, so pasting it can never raise one. Empty when no
 * count is below its ceiling.
 */
export function baselineNotices(counts: MarkerCounts, baselines: MarkerCounts = BASELINES): string[] {
  const notices: string[] = [];
  for (const name of MARKER_NAMES) {
    if (counts[name] < baselines[name]) {
      notices.push(
        `\`${name}\` is now ${counts[name]}, below its baseline ${baselines[name]}. Optional, not a failure: lower \`BASELINES.${name}\` to ${counts[name]} in scripts/check-pro-boundary.ts.`,
      );
    }
  }
  if (notices.length > 0) {
    notices.push(`To tighten every lowered baseline at once, paste over BASELINES in scripts/check-pro-boundary.ts:\n${formatBaselines(tightenedBaselines(counts, baselines))}`);
  }
  return notices;
}

function printTable(counts: MarkerCounts): void {
  const width = Math.max(...MARKER_NAMES.map((name) => name.length));
  console.log(`${"marker".padEnd(width)}  ${"count".padStart(6)}  ${"baseline".padStart(8)}`);
  for (const name of MARKER_NAMES) {
    console.log(`${name.padEnd(width)}  ${String(counts[name]).padStart(6)}  ${String(BASELINES[name]).padStart(8)}`);
  }
}

/** Prints hits grouped by file. A line marker's hit is `path:line: text`; `proNamedFiles` hits are bare paths. */
function printGrouped(hits: readonly string[]): void {
  const byFile = new Map<string, string[]>();
  for (const hit of hits) {
    const file = hitFile(hit);
    const lines = byFile.get(file) ?? [];
    lines.push(hit.startsWith(`${file}:`) ? hit.slice(file.length + 1) : "");
    byFile.set(file, lines);
  }
  for (const [file, lines] of byFile) {
    console.log(file);
    for (const line of lines) if (line !== "") console.log(`  ${line}`);
  }
}

function usage(): never {
  console.error(
    [
      "Usage: bun run scripts/check-pro-boundary.ts [--list <marker> | --changed]",
      "  (no arguments)   print counts against baselines; exit 1 if any is above, notice if any is below",
      "  --list <marker>  print every hit for one marker, grouped by file",
      "  --changed        print, per marker, the hits in files this branch changed",
      `Markers: ${MARKER_NAMES.join(", ")}`,
    ].join("\n"),
  );
  process.exit(2);
}

type Invocation = { mode: "check" } | { mode: "changed" } | { mode: "list"; marker: MarkerName };

/** The parsed command line, or null when it is not one of the three forms usage() lists. */
function parseArguments(args: readonly string[]): Invocation | null {
  if (args.length === 0) return { mode: "check" };
  if (args.length === 1 && args[0] === "--changed") return { mode: "changed" };
  if (args.length === 2 && args[0] === "--list" && (MARKER_NAMES as readonly string[]).includes(args[1])) {
    return { mode: "list", marker: args[1] as MarkerName };
  }
  return null;
}

if (import.meta.main) {
  // Validate the command line before the scan: a usage error must not pay for
  // reading every file in the repository, or need a git checkout at all.
  const invocation = parseArguments(process.argv.slice(2));
  if (invocation === null) usage();
  const { counts, hits } = countMarkers(listScannedFiles());

  if (invocation.mode === "list") {
    printGrouped(hits[invocation.marker]);
  } else if (invocation.mode === "changed") {
    const changed = new Set(changedFilesSinceMain());
    for (const name of MARKER_NAMES) {
      const inChangedFiles = hits[name].filter((hit) => changed.has(hitFile(hit)));
      console.log(`${name}: ${inChangedFiles.length}`);
      for (const hit of inChangedFiles) console.log(`  ${hit}`);
    }
  } else {
    printTable(counts);
    const problems = [...exemptionProblems(), ...baselineFailures(counts, BASELINES, changedFilesSinceMain(), hits)];
    for (const problem of problems) console.error(`\n❌ ${problem}`);
    const notices = baselineNotices(counts);
    for (const notice of notices) console.log(`\nℹ️  ${notice}`);
    if (problems.length > 0) process.exit(1);
    console.log(notices.length > 0 ? "\n✅ No count is above its baseline (some are below: see the notice above)." : "\n✅ Every Pro-boundary count equals its baseline.");
  }
}
