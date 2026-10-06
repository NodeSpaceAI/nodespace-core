#!/usr/bin/env bun
// Enforces the sync boundary of ADR-081: core is the complete free
// product and ships no Pro code. This is a hard ban (ADR-081 section 8): any
// line that matches a marker, and any file whose basename has a `pro`
// segment, fails the check. There are no baselines to raise and no
// exemptions. A false positive gets a narrow, tested fix to the marker's
// pattern here.
//
// Scope: the files git lists (tracked, plus untracked files that are not
// ignored) under `packages/` and `scripts/`, plus the root `README.md`. This
// file and its test are excluded, because they name the patterns they scan
// for. Listing with git, not walking the filesystem, keeps gitignored build
// output out: the staged skill copy under the Tauri resources, and any
// binary another build drops next to a sidecar.
//
// ALLOWLIST is the only way a hit passes. ADR-081 section 8 allows exactly one
// file in it: the module holding core's display names for known extension
// ids, which maps `sync` to the paid offering's name for the refusal of a
// database that needs it. An entry removes only its exact string from that file's lines
// before the markers are tested, so every other marker in the file still
// counts. The installer and CLI coexistence checks (ADR-081 section 2d)
// compare the installed product with `community` and name no other product,
// so they need no entry.
//
// A test that proves Pro code is absent builds its needle from fragments
// (`["pro", "tier"].join("_")`), so it does not contain the marker it looks
// for. CLAUDE.md, under "Sync Boundary", states these rules; this file
// enforces them.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

export const REPO = join(dirname(new URL(import.meta.url).pathname), "..");

// Every path under these prefixes is in scope, so a package added later is
// scanned automatically.
export const SCAN_PREFIXES: readonly string[] = ["packages/", "scripts/"];
export const SCAN_ROOT_FILES: readonly string[] = ["README.md"];

// Pro markers can live in more than source code: protos, the skill docs,
// Tauri and Cargo config, installer scripts. Paths with no extension (the
// installer's preinstall and postinstall) are scanned too.
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
export const EXCLUDED_FILES: readonly string[] = ["scripts/check-sync-boundary.ts", "scripts/check-sync-boundary.test.ts"];

// The Tauri commands of the removed Pro layer, by full name, and its two
// modules. Whole identifiers, so a core name that merely starts with `pro_`
// (`pro_env`) is not a hit. Entries are regular-expression sources.
const PRO_COMMANDS: readonly string[] = [
  "pro_accept_invite",
  "pro_activate_database",
  "pro_approve_admission",
  "pro_approve_request",
  "pro_bind_[t]enant",
  "pro_create_invite",
  "pro_current_person",
  "pro_current_status",
  "pro_enable_sync",
  "pro_initiate_admission",
  "pro_initiate_oauth",
  "pro_join_collection",
  "pro_leave_collection",
  "pro_list_invites",
  "pro_list_joinable_collections",
  "pro_list_members",
  "pro_list_requests",
  "pro_list_[t]enant_members",
  "pro_list_[t]enant_memberships",
  "pro_remove_from_[t]enant",
  "pro_remove_member",
  "pro_request_join",
  "pro_revoke_invite",
  "pro_set_member",
  "pro_signout",
  "pro_subscribe_sync_status",
  "pro_tier",
  "pro_sync",
  "pro_client",
];

// The data-model identifiers ADR-083 removes or renames, and the
// cloud-embedding hooks of ADR-081 section 4 B2. Entries are
// regular-expression sources. The lower-case field names are anchored so
// that a word ending in the same letters (`async_enabled`, `oauth_status`) is
// not a hit, while a camelCase use (`isSyncEnabled`, `dataSyncEnabled`) is.
// `OAuthStatus` is excluded the same way, through the `Auth` lookbehind.
const REMOVED_IDENTIFIERS: readonly string[] = [
  "[bB]ound_?[Tt]enant",
  "(?<![A-Za-z])sync_?[eE]nabled",
  "Sync_?[eE]nabled",
  "(?<![A-Za-z])auth_?[sS]tatus",
  "(?<![Oo])Auth_?[sS]tatus",
  "restrictedToMembers",
  "restricted_to_members",
  "\\bpersonal_collection_id\\b",
  "\\bseed_personal_ai_chat_collection_if_needed\\b",
  "\\bset_ai_chat_personal_collection_default\\b",
  "play-core-ai-chat-privacy",
  "\\bAI_CHAT_PRIVACY_PLAY_ID\\b",
  "\\bSupersededEdit\\b",
  "\\bsuperseded_edit\\b",
  "\\bDuplicateReactiveCreate\\b",
  "\\bduplicate_reactive_create\\b",
  "\\bsync_seq\\b",
  "\\bidx_emb_modified\\b",
  "\\bupsert_embeddings_with_origin\\b",
  "\\bapply_remote_embeddings\\b",
  "\\bembeddings_modified_since\\b",
  "\\bsubscribe_for_push\\b",
  "\\bset_push_excluded_origin\\b",
  "\\bget_multi_membership_edges\\b",
];

// Each pattern is tested once per line, so a line counts at most once per
// marker, and one line can count under several markers. Markers match whole
// identifiers or exact strings, never bare prefixes (ADR-081 section 8).
//
// A few words are spelled with a one-character class ([t]enant, [S]upabase,
// [S]ync) so that a plain-text search of the repository for those words lists
// only real hits, not this file.
export const MARKERS = {
  proCommands: {
    pattern: new RegExp(`\\b(?:${PRO_COMMANDS.join("|")})\\b`),
    summary: "the Pro Tauri commands by name, and the pro_sync / pro_client modules",
  },
  proSyncModule: {
    pattern: /[pP]roSync|pro-sync/,
    summary: "proSync and pro-sync: the Pro sync store, its variant resolver and their imports",
  },
  proProtocol: {
    pattern: /\bnodespace_pro\b|nodespace\.pro\.v1|Cloud[S]yncService|\bPro(?:Client|Tier)\b|pro\.nodespace\.ai|127\.0\.0\.1:8787/,
    summary: "the Pro proto and its cloud service, ProClient / ProTier, the Pro Worker URLs",
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
      /\bis_pro(?:_build)?\b|NODESPACED?_PRO_|feature\s*=\s*"pro"|nodespaced-pro|(?:daemon|ui)(?:-dev)?-pro\.(?:sock|pid)|incompatible-database(?:-dev)?-pro\b|\.daemon(?:\.dev)?\.pro\b|PRO_DAEMON_BINARY_NAME|tauri\.pro\.conf|--edition\b(?!\s*=?\s*20\d\d)/,
    summary: "Pro edition switches: is_pro, NODESPACE_PRO_*, the pro feature, Pro binary, socket, pid, marker and launchd names, the Pro Tauri overlay, --edition",
  },
  proDataModel: {
    pattern: new RegExp(REMOVED_IDENTIFIERS.join("|")),
    summary:
      "data-model identifiers ADR-083 removed or renamed: binding, sync and auth fields, restriction, AI-chat privacy, sync-only conflict kinds, sync_seq, cloud-embedding hooks, the old change-feed and membership-query names",
  },
  cloudWording: {
    pattern: /[Ss]upabase|S[U]PABASE|[Pp]gvector|PGVECTOR|[Nn]ode[Ss]pace[-_][Ss]ync|NODESPACE[-_]SYNC|\bRLS\b|[Pp]ro daemon/,
    summary: '[S]upabase, row-level security, pgvector, the nodespace[-_]sync repository, "Pro daemon", including upper-case spellings',
  },
  cloudAccountWording: {
    pattern:
      /\b(?:the|a|an|each|every|one|this|that|its|your|our|their|any) [t]enants?\b|(?:bound|cloud|sync|workspace)[-_ ]?[t]enant|\bper[- ][t]enant|[t]enants?[-_ ]?(?:schema|collection|id|root|binding|admission|member|admin|provision|url)|syncs to [t]enant|\w[t]enant|[t]enants?(?=[\w-])|multi-[t]enant/i,
    summary:
      'Pro-sense [t]enant wording: "the / a / each [t]enant", bound / cloud / per- / sync / workspace [t]enant, [t]enant schema / collection / id / root / binding / admission / member / admin / provisioning / URL, "syncs to [t]enant", and the word inside an identifier',
  },
  syncVocabulary: {
    pattern: /(?<!i)cloud[-_ ]?[s]ync|cloud[- ](?:push|pull)|\bto cloud\b|pro-gated/i,
    summary: 'cloud push / pull / [s]ync wording, "to cloud", pro-gated',
  },
  proWording: {
    pattern: /(?<!\b(?:M\d|V\d|MacBook|iPad|iPhone) )\bPro\b/,
    summary: 'the word Pro, except chip and model names such as "M2 Pro", "DeepSeek V4 Pro"',
  },
  productName: {
    pattern: /\bNodeSpace[ ](?:Pro|Sync)\b/,
    summary: "the paid offering's name, NodeSpace[ ]Sync, and its retired name NodeSpace[ ]Pro (case-sensitive, whole word)",
  },
} satisfies Record<string, { pattern: RegExp; summary: string }>;

export type LineMarkerName = keyof typeof MARKERS;
export type MarkerName = LineMarkerName | "proNamedFiles";

const LINE_MARKER_NAMES = Object.keys(MARKERS) as LineMarkerName[];
export const MARKER_NAMES: readonly MarkerName[] = [...LINE_MARKER_NAMES, "proNamedFiles"];

const PRO_NAMED_FILE = /(^|[-_.])pro([-_.]|$)/;
const PRO_NAMED_SUMMARY = "files whose basename has a pro segment";

function summaryOf(marker: MarkerName): string {
  return marker === "proNamedFiles" ? PRO_NAMED_SUMMARY : MARKERS[marker].summary;
}

/** Whether the file's basename has a `pro` segment, as in `pro-plugin.ts` or `tauri.pro.conf.json`. */
export function isProNamedFile(path: string): boolean {
  return PRO_NAMED_FILE.test(path.slice(path.lastIndexOf("/") + 1));
}

// See the file-level comment. The one entry ADR-081 section 8 allows: the
// display-name module for the refusal of a sync database may name the offering, and
// nothing else in it is exempt.
export const ALLOWLIST: readonly { file: string; exempt: string }[] = [
  { file: "packages/proto/src/extension_names.rs", exempt: "NodeSpace Sync" },
];

export type AllowlistEntry = (typeof ALLOWLIST)[number];
export type MarkerHits = Record<MarkerName, string[]>;
export type MarkerCounts = Record<MarkerName, number>;

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
      `check-sync-boundary needs a git checkout: it lists files with git so that gitignored build output stays out of the scan (${err instanceof Error ? err.message : String(err)})`,
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

/** The line with every allowlisted string for its file removed. */
function withoutAllowed(line: string, allowed: readonly string[]): string {
  return allowed.reduce((text, exempt) => text.split(exempt).join(""), line);
}

/**
 * Every marker hit over the given files: `path:line: trimmed text` for a line
 * marker, the bare path for `proNamedFiles`. A pure function of the files'
 * contents and the allowlist, so it is testable against fixtures. A file that
 * can't be read (still in the index, deleted from the working tree) is
 * skipped entirely, `proNamedFiles` included.
 */
export function countMarkers(
  files: readonly string[],
  repoRoot: string = REPO,
  allowlist: readonly AllowlistEntry[] = ALLOWLIST,
): { counts: MarkerCounts; hits: MarkerHits } {
  const hits = emptyHits();
  for (const file of files) {
    const text = readText(repoRoot, file);
    if (text === null) continue;
    if (isProNamedFile(file)) hits.proNamedFiles.push(file);
    const allowed = allowlist.filter((entry) => entry.file === file).map((entry) => entry.exempt);
    text.split("\n").forEach((line, index) => {
      const tested = withoutAllowed(line, allowed);
      for (const name of LINE_MARKER_NAMES) {
        if (MARKERS[name].pattern.test(tested)) hits[name].push(`${file}:${index + 1}: ${line.trim()}`);
      }
    });
  }
  const counts = Object.fromEntries(MARKER_NAMES.map((name) => [name, hits[name].length])) as MarkerCounts;
  return { counts, hits };
}

/**
 * One message per problem with the allowlist. ADR-081 section 8 allows one
 * file. An entry must hide something real: its string is non-empty, matches a
 * marker on its own, and still occurs in a file that is in the scan. A stale
 * entry left in place would hide the next real hit of that string.
 */
export function allowlistProblems(allowlist: readonly AllowlistEntry[] = ALLOWLIST, repoRoot: string = REPO): string[] {
  const problems: string[] = [];
  const files = [...new Set(allowlist.map((entry) => entry.file))];
  if (files.length > 1) {
    problems.push(`ALLOWLIST names ${files.length} files (${files.join(", ")}); ADR-081 section 8 allows exactly one, the extension display-name module.`);
  }
  const scanned = allowlist.length > 0 ? new Set(listScannedFiles(repoRoot)) : new Set<string>();
  for (const entry of allowlist) {
    const label = `ALLOWLIST entry ${JSON.stringify(entry.exempt)} for ${entry.file}`;
    if (entry.exempt === "") {
      problems.push(`${label}: the exempt string is empty.`);
      continue;
    }
    if (!LINE_MARKER_NAMES.some((name) => MARKERS[name].pattern.test(entry.exempt))) {
      problems.push(`${label}: the string matches no marker, so the entry hides nothing. Remove it.`);
    }
    if (!scanned.has(entry.file)) {
      problems.push(`${label}: the file is missing from the scan (deleted, ignored, or outside the scanned paths). Remove the entry.`);
    } else if (!(readText(repoRoot, entry.file) ?? "").includes(entry.exempt)) {
      problems.push(`${label}: the string no longer occurs in the file, so the entry is stale. Remove it.`);
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

const GUIDANCE =
  "Core ships no Pro code (ADR-081): Pro features and Pro fixes belong in the Pro repository (ADR-081 section 5), and core may only expose a generic extension point (ADR-082). " +
  "A test that proves something is absent builds its needle from fragments. " +
  "A false positive gets a narrow, tested fix to the marker's pattern in scripts/check-sync-boundary.ts.";

/** The file a hit belongs to: `path:line: text` for a line marker, the bare path for `proNamedFiles`. */
function hitFile(hit: string): string {
  return /^(.*?):\d+: /.exec(hit)?.[1] ?? hit;
}

/**
 * One actionable message per marker with any hit; empty when there is none.
 * Hits in files this branch changed are listed first, then every hit.
 */
export function hitFailures(hits: MarkerHits, changedFiles: readonly string[] = []): string[] {
  const changed = new Set(changedFiles);
  const failures: string[] = [];
  for (const name of MARKER_NAMES) {
    const all = hits[name];
    if (all.length === 0) continue;
    const lines = [`${name} (${summaryOf(name)}): ${all.length} hit${all.length === 1 ? "" : "s"}; the hard ban allows none.`, GUIDANCE];
    const inChangedFiles = all.filter((hit) => changed.has(hitFile(hit)));
    if (inChangedFiles.length > 0) lines.push("In files this branch changed:", ...inChangedFiles);
    lines.push("All hits:", ...all);
    failures.push(lines.join("\n"));
  }
  return failures;
}

function printTable(counts: MarkerCounts): void {
  const width = Math.max(...MARKER_NAMES.map((name) => name.length));
  console.log(`${"marker".padEnd(width)}  ${"hits".padStart(6)}`);
  for (const name of MARKER_NAMES) console.log(`${name.padEnd(width)}  ${String(counts[name]).padStart(6)}`);
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
      "Usage: bun run scripts/check-sync-boundary.ts [--list <marker> | --changed]",
      "  (no arguments)   print the hits per marker; exit 1 if there is any",
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
    const problems = [...allowlistProblems(), ...hitFailures(hits, changedFilesSinceMain())];
    for (const problem of problems) console.error(`\n❌ ${problem}`);
    if (problems.length > 0) process.exit(1);
    console.log("\n✅ No sync-boundary marker in core (ADR-081 section 8 hard ban).");
  }
}
