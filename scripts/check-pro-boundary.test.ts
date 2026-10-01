// Covers the Pro-boundary hard ban (scripts/check-pro-boundary.ts). Pattern
// tests run each marker's regex against tables of lines that must and must
// not match. Discovery, allowlist and CLI tests build a throwaway git
// repository per test, because the behavior under test is git's: what it
// tracks, what it ignores, and what a branch changed. The real-repo block at
// the bottom is the enforcement path (`bun run test:scripts`, so every merge
// gate) and throws the same actionable message the CLI prints.
//
// Every needle is assembled at run time from fragments with `j(...)`, so the
// needles are not in this file's text: a repository search for them lists only
// real hits. Marker names and a few non-matching examples ("M2 Pro") remain.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import * as checker from "./check-pro-boundary";
import {
  ALLOWLIST,
  EXCLUDED_FILES,
  MARKERS,
  MARKER_NAMES,
  allowlistProblems,
  changedFilesSinceMain,
  countMarkers,
  hitFailures,
  isProNamedFile,
  listScannedFiles,
  type AllowlistEntry,
  type LineMarkerName,
  type MarkerCounts,
  type MarkerHits,
} from "./check-pro-boundary";

// Many tests spawn git or bun processes. Bun's 5s default per-test timeout is
// tight on the loaded machines these tests run on (the merge gate shares them
// with Rust builds), and a timeout here would eject an unrelated PR.
setDefaultTimeout(30_000);

const CHECKER_PATH = join(dirname(new URL(import.meta.url).pathname), "check-pro-boundary.ts");
const LINE_MARKERS = Object.keys(MARKERS) as LineMarkerName[];

/** Joins fragments into one needle. */
const j = (...parts: string[]): string => parts.join("");

// Sample hits, each for a known marker.
const TEN = j("ten", "ant");
const CMD = j("pro", "_tier"); // proCommands
const CMD2 = j("pro", "_signout"); // proCommands
const WORD = j("P", "ro"); // proWording
const PRODUCT = j("NodeSpace", " ", "P", "ro"); // productName and proWording
const BOUND = j("bound ", TEN); // cloudAccountWording
const DAEMON = j("the ", WORD, " daemon"); // cloudWording and proWording

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "check-pro-boundary-test-"));
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

// Git tells a hook where its repository is (GIT_DIR in a linked worktree's
// pre-push). A fixture `git init` or `git add` must never see that.
function gitEnv(): Record<string, string | undefined> {
  const env = { ...process.env };
  for (const name of ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"]) delete env[name];
  return env;
}

function git(cwd: string, ...args: string[]): { status: number; stdout: string } {
  const result = spawnSync("git", ["-C", cwd, "-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", ...args], {
    encoding: "utf8",
    env: gitEnv(),
  });
  return { status: result.status ?? -1, stdout: result.stdout };
}

function initRepo(at: string = dir): void {
  expect(git(at, "init", "--quiet").status).toBe(0);
}

function write(relativePath: string, content: string, at: string = dir): void {
  const full = join(at, relativePath);
  mkdirSync(dirname(full), { recursive: true });
  writeFileSync(full, content);
}

function withEnv<T>(vars: Record<string, string>, fn: () => T): T {
  const saved = Object.fromEntries(Object.keys(vars).map((name) => [name, process.env[name]]));
  Object.assign(process.env, vars);
  try {
    return fn();
  } finally {
    for (const [name, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

// A repository whose `origin/main` is its first commit, as in a PR worktree.
function repoWithOriginMain(): void {
  initRepo();
  write(".gitignore", "ignored/\n");
  write("packages/a/old.ts", "x\n");
  write("packages/a/edited.ts", "x\n");
  expect(git(dir, "add", "-A").status).toBe(0);
  expect(git(dir, "commit", "--quiet", "-m", "base").status).toBe(0);
  expect(git(dir, "update-ref", "refs/remotes/origin/main", "HEAD").status).toBe(0);
}

function noHits(): MarkerHits {
  return Object.fromEntries(MARKER_NAMES.map((name) => [name, []])) as unknown as MarkerHits;
}

function zeroCounts(): MarkerCounts {
  return Object.fromEntries(MARKER_NAMES.map((name) => [name, 0])) as MarkerCounts;
}

// ---------------------------------------------------------------------------
// Pattern tables. A fixture with regex-special characters is built in code,
// as a string, never through a shell.
// ---------------------------------------------------------------------------

// The Pro Tauri commands of ADR-081 section 4, every one by name.
const PRO_COMMAND_NAMES = [
  "accept_invite",
  "activate_database",
  "approve_admission",
  "approve_request",
  j("bind_", TEN),
  "create_invite",
  "current_person",
  "current_status",
  "enable_sync",
  "initiate_admission",
  "initiate_oauth",
  "join_collection",
  "leave_collection",
  "list_invites",
  "list_joinable_collections",
  "list_members",
  "list_requests",
  j("list_", TEN, "_members"),
  j("list_", TEN, "_memberships"),
  j("remove_from_", TEN),
  "remove_member",
  "request_join",
  "revoke_invite",
  "set_member",
  "signout",
  "subscribe_sync_status",
  "tier",
].map((suffix) => j("pro", "_", suffix));

const PATTERN_CASES: Record<LineMarkerName, { match: string[]; noMatch: string[] }> = {
  proCommands: {
    match: [
      ...PRO_COMMAND_NAMES,
      j("invoke('", "pro", "_list_members')"),
      j("commands::", "pro", "_sync::", "pro", "_tier,"),
      j("pub mod ", "pro", "_client;"),
    ],
    // Whole identifiers only: a core name that merely starts with the prefix is not a hit.
    noMatch: ["pro_env", j("pro", "_tier_extra"), j("pro", "_sync_status"), "approve_request", "macro_rules!", "use nodespace_proto::socket;"],
  },
  proSyncModule: {
    match: [
      j("import { pro", "Sync } from '$lib/stores/pro", "-sync.svelte'"),
      j("resolvePro", "SyncVariant()"),
      j("isPro", "SyncActive()"),
      j("type Pro", "SyncState = {};"),
    ],
    noMatch: ["project-sync", "prosync"],
  },
  proProtocol: {
    match: [
      j('tonic::include_proto!("nodespace', '.pro.v1")'),
      j("Cloud", "Sync", "ServiceClient"),
      j("app.try_state::<", "Pro", "Client>()"),
      j("let tier: ", "Pro", "Tier = x;"),
      j('"https://pro', '.nodespace.ai"'),
      j("http://127.0.0.1", ":8787/v1"),
      j("// see nodespace", "_pro.proto"),
      j("use nodespace", "_pro::x;"),
    ],
    noMatch: ["use nodespace_proto::socket;", "nodespace_protocol", "ProcessTier", "ProClientele"],
  },
  proEvents: {
    match: [j("listen('pro", ":tier-detected', cb)"), j('app.emit("sync', ':status", p)'), j("listen(`sync", ":error`, cb)"), j('emit("sync', ':error")')],
    noMatch: ["sync:status updates", "the sync:error path", "sync:statuses"],
  },
  membershipService: {
    match: [j("membership", "Service.list()"), j("class Membership", "Service {}"), j("from '$lib/services/membership", "-service'")],
    noMatch: ["collection membership", "member_of"],
  },
  editionBranching: {
    match: [
      j("is_", "pro_build()"),
      j("fn f(is_debug: bool, is_", "pro: bool)"),
      j('option_env!("NODESPACE', '_PRO_SERVER_URL")'),
      j("NODESPACED", "_PRO_ANON_KEY"),
      j('cfg!(feature = "', 'pro")'),
      j('"daemon', '-pro.sock"'),
      j('"daemon-dev', '-pro.sock"'),
      j('"ui', '-pro.pid"'),
      j('"incompatible-database', '-pro.json"'),
      j('"incompatible-database-dev', '-pro.json"'),
      j('format!("{}.daemon', '.pro")'),
      j("app.nodespace.daemon", ".pro"),
      j("app.nodespace.daemon.dev", ".pro"),
      j("tauri.pro", ".conf.json"),
      j("nodespaced --edi", "tion"),
      j("nodespaced", "-pro-aarch64"),
      j("PRO_DAEMON", "_BINARY_NAME"),
    ],
    // ADR-084 section 5: core's preinstall honours the force variable, so it is not a marker.
    noMatch: [
      "rustfmt --edition 2021",
      'edition = "2021"',
      "rustfmt --edition=2024",
      '"daemon-dev.sock"',
      "app.nodespace.daemon.dev",
      "is_production",
      "incompatible-database-protocol",
      "NODESPACE_FORCE_COMMUNITY=1",
    ],
  },
  proDataModel: {
    match: [
      j("bound_", TEN, "_schema"),
      j("bound", "Ten", "antCollection"),
      j("setBound", "Ten", "ant()"),
      j("sync", "_enabled"),
      j("labsFlags.sync", "Enabled"),
      j("isSync", "Enabled"),
      j("dataSync", "Enabled"),
      j("auth", "_status"),
      j("auth", "Status"),
      j("restrictedTo", "Members"),
      j("restricted_to", "_members"),
      j("personal_collection", "_id"),
      j("play-core-ai-chat", "-privacy"),
      j("AI_CHAT_PRIVACY", "_PLAY_ID"),
      j("ConflictKind::Superseded", "Edit"),
      j('"superseded', '_edit"'),
      j("Duplicate", "ReactiveCreate"),
      j("duplicate_reactive", "_create"),
      j("node.sync", "_seq"),
      j("apply_remote", "_embeddings"),
      j("embeddings_modified", "_since"),
      j("db_sync", "_enabled"),
      j("seed_personal_ai_chat", "_collection_if_needed"),
      j("set_ai_chat_personal", "_collection_default"),
      j("idx_emb", "_modified"),
      j("upsert_embeddings", "_with_origin"),
      j("subscribe_for", "_push"),
      j("set_push_excluded", "_origin"),
      j("get_multi_membership", "_edges"),
    ],
    // Anchored: a word that merely ends in the same letters is not a hit.
    noMatch: [
      "synced",
      "authenticate",
      "personal_collection_ids",
      "sync_sequence",
      "UniqueFieldCollision",
      "local_only",
      "fsync_enabled",
      "async_enabled",
      "oauth_status",
      "OAuthStatus",
      "member_of_edges",
      "subscribe_to_events_excluding_origin",
    ],
  },
  cloudWording: {
    match: [
      j("Supa", "base"),
      j("supa", "base"),
      j("SUPA", "BASE_URL"),
      j("Postgres R", "LS"),
      j("pg", "vector"),
      j("PG", "VECTOR"),
      j("nodespace", "-sync"),
      j("nodespace", "_sync::"),
      j("NodeSpace", "-Sync"),
      j("NODESPACE", "_SYNC_DIR"),
      DAEMON,
      j("the pro", " daemon"),
    ],
    noMatch: ["URLS", "CURLS", "sync service"],
  },
  cloudAccountWording: {
    match: [
      BOUND,
      j("cloud ", TEN),
      j("sync ", TEN),
      j("workspace ", TEN),
      j("per-", TEN),
      j("one schema per ", TEN),
      j(TEN, " schema"),
      j(TEN, "_collection"),
      j(TEN, "Id"),
      j(TEN, " root"),
      j(TEN, " binding"),
      j(TEN, " admission"),
      j(TEN, " members"),
      j("TEN", "ANT_ADMIN_ROLES"),
      j("Syncs to ", TEN),
      // The word after a determiner, and as a provisioning or URL noun.
      j("the ", TEN),
      j("each ", TEN, " gets its own schema"),
      j("a ", TEN, "'s members"),
      j("signed in to a ", TEN),
      j("operator-only ", TEN, " provisioning"),
      j("the ", TEN, " URL"),
      j("Copy ", TEN, " URL"),
      j("the ", TEN, " pays rent"),
      // The word inside an identifier.
      j(TEN, "_slug"),
      j("Ten", "antRole"),
      j("list_", TEN, "s"),
      j("struct ", "Ten", "ants {"),
      j(TEN, "-scoped"),
      j("multi-", TEN),
      j("TEN", "ANT_ID"),
    ],
    // The agent's lease-agreement fixture uses the word in its ordinary sense.
    noMatch: [j("the landlord finds a replacement ", TEN, " sooner"), j("a replacement ", TEN), "maintenance"],
  },
  syncVocabulary: {
    match: [j("cloud", " sync"), j("cloud", "-sync"), j("cloud", "_sync"), j("Cloud", "Sync"), j("cloud", " push"), j("cloud", "-pull"), j("upload to", " cloud"), j("pro", "-gated")],
    noMatch: ["sync to disk", "cloudflare", "to cloudflare", "progated", j("iCloud", " sync")],
  },
  proWording: {
    match: [j("// the ", WORD, " build"), PRODUCT, j(WORD, "-only"), WORD],
    noMatch: ["M2 Pro", "DeepSeek V4 Pro", "MacBook Pro", "iPad Pro", "iPhone Pro", "Protocol", "Provider", "pro", "NodeSpaceProduct"],
  },
  productName: {
    match: [PRODUCT, j("This database needs ", PRODUCT), j(PRODUCT, "'s sync")],
    // Case-sensitive and whole-word.
    noMatch: ["NodeSpace prompt", "NodeSpace Protocol", "NodeSpace Product", "nodespace pro", "NodeSpaceProduct"],
  },
};

describe("MARKERS patterns", () => {
  test("every marker has a pattern table", () => {
    expect(Object.keys(PATTERN_CASES)).toEqual(LINE_MARKERS);
  });

  for (const name of LINE_MARKERS) {
    describe(name, () => {
      for (const line of PATTERN_CASES[name].match) {
        test(`matches ${JSON.stringify(line)}`, () => {
          expect(MARKERS[name].pattern.test(line)).toBe(true);
        });
      }
      for (const line of PATTERN_CASES[name].noMatch) {
        test(`does not match ${JSON.stringify(line)}`, () => {
          expect(MARKERS[name].pattern.test(line)).toBe(false);
        });
      }
    });
  }

  test("the command table names all 27 Pro Tauri commands", () => {
    expect(new Set(PRO_COMMAND_NAMES).size).toBe(27);
  });

  test("no pattern is global or sticky, so a single test() per line is stateless", () => {
    for (const name of LINE_MARKERS) {
      expect(MARKERS[name].pattern.global).toBe(false);
      expect(MARKERS[name].pattern.sticky).toBe(false);
    }
  });

  test("every marker has a one-line summary", () => {
    for (const name of LINE_MARKERS) {
      expect(MARKERS[name].summary.trim()).not.toBe("");
      expect(MARKERS[name].summary).not.toContain("\n");
    }
  });

  // ADR-081 sections 2b and 2d, ADR-084 sections 1 and 5: the coexistence
  // checks and the contact entry point name no other product, so they need
  // no allowlist entry. These lines must stay clean under every marker.
  test("the copy core keeps at the boundary matches no marker", () => {
    const lines = [
      "Want team collaboration? Contact us",
      "mailto:developer@nodespace.ai",
      "For team collaboration, contact [developer@nodespace.ai](mailto:developer@nodespace.ai).",
      'product=$(/usr/libexec/PlistBuddy -c "Print :NodeSpaceProduct" "$APP/Contents/Info.plist")',
      'if [ "$product" != "community" ] && [ "${NODESPACE_FORCE_COMMUNITY:-}" != "1" ]; then',
      "Another NodeSpace product is installed on this Mac. Uninstall it with its own uninstaller first. Your databases stay on this Mac.",
      "The NodeSpace app on this Mac is a different NodeSpace product, or an older NodeSpace that does not say which product it is.",
      "Use that product's own uninstaller, or update NodeSpace first; this command removes only the free NodeSpace.",
    ];
    for (const line of lines) {
      for (const name of LINE_MARKERS) expect({ line, name, hit: MARKERS[name].pattern.test(line) }).toEqual({ line, name, hit: false });
    }
  });
});

describe("isProNamedFile", () => {
  for (const path of [
    j("pro", "-plugin.ts"),
    j("pro", "_sync.rs"),
    j("tauri.pro", ".conf.json"),
    j("nodespace", "_pro.proto"),
    j("first-pro", "-consent-modal.svelte"),
    j("pro", "-sync.svelte.ts"),
    j("packages/desktop-app/src/lib/plugins/pro", "-plugin.ts"),
    "pro",
  ]) {
    test(`matches ${path}`, () => {
      expect(isProNamedFile(path)).toBe(true);
    });
  }

  for (const path of [
    "provider.ts",
    "process.rs",
    "node_service.proto",
    "protocol.ts",
    "project-sync.ts",
    "approve.ts",
    "repro.ts",
    // Only the basename counts: a directory with a pro segment does not.
    "packages/pro/lib.ts",
    "packages/pro-tools/index.ts",
  ]) {
    test(`does not match ${path}`, () => {
      expect(isProNamedFile(path)).toBe(false);
    });
  }
});

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

describe("listScannedFiles", () => {
  test("lists tracked files and untracked files that are not ignored", () => {
    initRepo();
    write("packages/a/b-tracked.ts", "x\n");
    expect(git(dir, "add", "packages/a/b-tracked.ts").status).toBe(0);
    write("packages/a/a-untracked.ts", "x\n");
    write("packages/a/z-untracked.ts", "x\n");
    // Sorted, whichever of the tracked and untracked groups git prints first.
    expect(listScannedFiles(dir)).toEqual(["packages/a/a-untracked.ts", "packages/a/b-tracked.ts", "packages/a/z-untracked.ts"]);
  });

  test("leaves out gitignored paths: a directory, and an extensionless binary name", () => {
    initRepo();
    // As in the real repo: the ignore file sits in src-tauri/, and its
    // patterns are relative to that directory.
    const binary = j("binaries/nodespaced", "-pro-x");
    write("packages/desktop-app/src-tauri/.gitignore", j("resources/skill/\n", "binaries/nodespaced", "-pro-*\n"));
    write("packages/desktop-app/src-tauri/resources/skill/SKILL.md", `${PRODUCT}\n`);
    write(`packages/desktop-app/src-tauri/${binary}`, "binary\n");
    write("packages/desktop-app/src-tauri/src/lib.rs", "// clean\n");
    const files = listScannedFiles(dir);
    expect(files).toEqual(["packages/desktop-app/src-tauri/.gitignore", "packages/desktop-app/src-tauri/src/lib.rs"]);
    const { counts } = countMarkers(files, dir);
    expect(counts.productName).toBe(0);
    expect(counts.proNamedFiles).toBe(0);
  });

  test("only scans packages/, scripts/ and the root README", () => {
    initRepo();
    write("README.md", "x\n");
    write("CLAUDE.md", "x\n");
    write("docs/notes.md", "x\n");
    write(".claude/skills/a/SKILL.md", "x\n");
    write("packages/a/lib.rs", "x\n");
    write("scripts/a.ts", "x\n");
    write("packages/README.md", "x\n");
    write("nested/README.md", "x\n");
    expect(listScannedFiles(dir)).toEqual(["README.md", "packages/README.md", "packages/a/lib.rs", "scripts/a.ts"]);
  });

  test("a CLAUDE.md full of markers counts 0", () => {
    initRepo();
    write("CLAUDE.md", `${CMD} ${BOUND} ${PRODUCT} ${j("Supa", "base")} ${j("is_pro", "_build")} ${j("pro", "Sync")}\n`);
    const { counts } = countMarkers(listScannedFiles(dir), dir);
    expect(Object.values(counts).every((n) => n === 0)).toBe(true);
  });

  test("scans .proto, .md, .json, extensionless files and dotfiles, and skips other extensions", () => {
    initRepo();
    const scanned = [
      "packages/a/x.proto",
      "packages/a/x.md",
      "packages/a/x.json",
      "packages/a/x.toml",
      "packages/a/x.rs",
      "packages/a/x.ts",
      "packages/a/x.svelte",
      "packages/a/x.js",
      "packages/a/x.sh",
      "packages/a/x.plist",
      "packages/a/x.css",
      "packages/a/x.html",
      "packages/a/x.py",
      "packages/a/x.d.ts",
      "scripts/pkg-resources/preinstall",
      "packages/a/.gitignore",
      "packages/a/.eslintrc.json",
    ];
    const skipped = ["packages/a/x.png", "packages/a/x.lock", "packages/a/x.txt", "packages/a/x.svg", "packages/a/x."];
    for (const path of [...scanned, ...skipped]) write(path, "x\n");
    expect(listScannedFiles(dir)).toEqual([...scanned].sort());
  });

  test("skips the checker's own two files", () => {
    initRepo();
    write("scripts/check-pro-boundary.ts", `${CMD}\n`);
    write("scripts/check-pro-boundary.test.ts", `${CMD}\n`);
    write("scripts/other.ts", "x\n");
    expect(EXCLUDED_FILES).toEqual(["scripts/check-pro-boundary.ts", "scripts/check-pro-boundary.test.ts"]);
    expect(listScannedFiles(dir)).toEqual(["scripts/other.ts"]);
  });

  test("a file deleted from the working tree but still in the index does not throw", () => {
    initRepo();
    const proNamed = j("packages/a/pro", "-gone.ts");
    write("packages/a/gone.ts", `${CMD}\n`);
    write(proNamed, "x\n");
    write("packages/a/kept.ts", `${CMD2}\n`);
    expect(git(dir, "add", "packages").status).toBe(0);
    unlinkSync(join(dir, "packages/a/gone.ts"));
    unlinkSync(join(dir, proNamed));
    const files = listScannedFiles(dir);
    expect(files).toContain("packages/a/gone.ts");
    const { counts, hits } = countMarkers(files, dir);
    expect(counts.proCommands).toBe(1);
    expect(hits.proCommands).toEqual([`packages/a/kept.ts:1: ${CMD2}`]);
    expect(counts.proNamedFiles).toBe(0);
  });

  test("a directory that is not a git repository throws a clear error, never an empty list", () => {
    write("packages/a/lib.rs", `${CMD}\n`);
    withEnv({ GIT_CEILING_DIRECTORIES: dirname(dir) }, () => {
      expect(() => listScannedFiles(dir)).toThrow(/needs a git checkout/);
    });
  });

  test("ignores the repository location git exports to a hook", () => {
    initRepo();
    write("packages/a/lib.rs", "x\n");
    const hookEnv = {
      GIT_DIR: join(dir, "not-a-repository.git"),
      GIT_INDEX_FILE: join(dir, "not-an-index"),
      GIT_WORK_TREE: join(dir, "elsewhere"),
    };
    withEnv(hookEnv, () => {
      expect(listScannedFiles(dir)).toEqual(["packages/a/lib.rs"]);
    });
  });
});

// ---------------------------------------------------------------------------
// Counting
// ---------------------------------------------------------------------------

describe("countMarkers", () => {
  test("a line counts once per marker, however many times it matches", () => {
    write("packages/a/lib.rs", `${CMD}(); ${CMD}(); ${CMD2}();\n`);
    const { counts } = countMarkers(["packages/a/lib.rs"], dir);
    expect(counts.proCommands).toBe(1);
  });

  test("one line can count under several markers, once each", () => {
    write("packages/a/lib.rs", `invoke('${CMD}'); // the ${BOUND} sees ${PRODUCT}\n`);
    const { counts } = countMarkers(["packages/a/lib.rs"], dir);
    expect(counts.proCommands).toBe(1);
    expect(counts.cloudAccountWording).toBe(1);
    expect(counts.proWording).toBe(1);
    expect(counts.productName).toBe(1);
    expect(counts.cloudWording).toBe(0);
  });

  test("counts one hit per matching line across lines and files", () => {
    write("packages/a/one.rs", `${CMD}\nclean\n${CMD2}\n`);
    write("packages/a/two.rs", `${CMD}\n`);
    const { counts } = countMarkers(["packages/a/one.rs", "packages/a/two.rs"], dir);
    expect(counts.proCommands).toBe(3);
  });

  test("hits are repo-relative path:line: trimmed text", () => {
    write("packages/a/lib.rs", `clean\n    ${CMD}();   \n// ${BOUND}\n`);
    const { hits } = countMarkers(["packages/a/lib.rs"], dir);
    expect(hits.proCommands).toEqual([`packages/a/lib.rs:2: ${CMD}();`]);
    expect(hits.cloudAccountWording).toEqual([`packages/a/lib.rs:3: // ${BOUND}`]);
  });

  test("proNamedFiles counts Pro-named paths, and its hits are the paths", () => {
    const plugin = j("packages/a/pro", "-plugin.ts");
    const overlay = j("packages/a/tauri.pro", ".conf.json");
    write(plugin, "clean\n");
    write("packages/a/provider.ts", "clean\n");
    write(overlay, "{}\n");
    const { counts, hits } = countMarkers([plugin, "packages/a/provider.ts", overlay], dir);
    expect(counts.proNamedFiles).toBe(2);
    expect(hits.proNamedFiles).toEqual([plugin, overlay]);
  });

  test("returns every marker, at 0 for a clean file", () => {
    write("packages/a/lib.rs", "fn main() {}\n");
    const { counts, hits } = countMarkers(["packages/a/lib.rs"], dir);
    expect(Object.keys(counts)).toEqual([...MARKER_NAMES]);
    expect(Object.keys(hits)).toEqual([...MARKER_NAMES]);
    expect(counts).toEqual(zeroCounts());
  });
});

// ---------------------------------------------------------------------------
// The allowlist
// ---------------------------------------------------------------------------

describe("countMarkers — allowlist", () => {
  const MODULE = "packages/x/src/extension-names.ts";
  const entry: AllowlistEntry = { file: MODULE, exempt: PRODUCT };

  test("an entry removes its exact string from its file's lines before the markers are tested", () => {
    write(MODULE, `  pro: "${PRODUCT}",\n`);
    const { counts } = countMarkers([MODULE], dir, [entry]);
    expect(counts).toEqual(zeroCounts());
  });

  test("every other marker on the same line still counts", () => {
    write(MODULE, `"${PRODUCT}" via ${DAEMON} or ${CMD}\n`);
    const { counts } = countMarkers([MODULE], dir, [entry]);
    expect(counts.productName).toBe(0);
    expect(counts.cloudWording).toBe(1);
    expect(counts.proWording).toBe(1);
    expect(counts.proCommands).toBe(1);
  });

  test("the removal is exact: a different spelling in the same file still counts", () => {
    write(MODULE, `"${PRODUCT}"\n${j("NodeSpace", " ", WORD, "!")}\n${j("nodespace ", WORD)}\n`);
    const { hits } = countMarkers([MODULE], dir, [{ file: MODULE, exempt: `"${PRODUCT}"` }]);
    expect(hits.productName).toEqual([`${MODULE}:2: ${j("NodeSpace", " ", WORD, "!")}`]);
    expect(hits.proWording).toHaveLength(2);
  });

  test("an entry applies only to its own file", () => {
    write(MODULE, `"${PRODUCT}"\n`);
    write("packages/x/src/other.ts", `"${PRODUCT}"\n`);
    const { hits } = countMarkers([MODULE, "packages/x/src/other.ts"], dir, [entry]);
    expect(hits.productName).toEqual([`packages/x/src/other.ts:1: "${PRODUCT}"`]);
  });

  test("without the entry the same file counts", () => {
    write(MODULE, `  pro: "${PRODUCT}",\n`);
    const { counts } = countMarkers([MODULE], dir, []);
    expect(counts.productName).toBe(1);
    expect(counts.proWording).toBe(1);
  });
});

describe("allowlistProblems", () => {
  const MODULE = "packages/x/src/extension-names.ts";
  const valid: AllowlistEntry = { file: MODULE, exempt: PRODUCT };

  function fixture(files: Record<string, string>): void {
    initRepo();
    for (const [path, content] of Object.entries(files)) write(path, content);
  }

  test("an empty allowlist has no problem, and needs no git checkout", () => {
    withEnv({ GIT_CEILING_DIRECTORIES: dirname(dir) }, () => {
      expect(allowlistProblems([], dir)).toEqual([]);
    });
  });

  test("a live entry has no problem", () => {
    fixture({ [MODULE]: `pro: "${PRODUCT}",\n` });
    expect(allowlistProblems([valid], dir)).toEqual([]);
  });

  test("several entries for the one file are allowed", () => {
    fixture({ [MODULE]: `pro: "${PRODUCT}", // ${WORD}\n` });
    expect(allowlistProblems([valid, { file: MODULE, exempt: WORD }], dir)).toEqual([]);
  });

  test("entries for a second file are reported: ADR-081 section 8 allows exactly one", () => {
    fixture({ [MODULE]: `"${PRODUCT}"\n`, "packages/x/src/other.ts": `"${PRODUCT}"\n` });
    const problems = allowlistProblems([valid, { file: "packages/x/src/other.ts", exempt: PRODUCT }], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("names 2 files");
    expect(problems[0]).toContain("exactly one");
  });

  test("reports an empty string", () => {
    fixture({ [MODULE]: `"${PRODUCT}"\n` });
    const problems = allowlistProblems([{ file: MODULE, exempt: "" }], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("empty");
  });

  test("reports a string that matches no marker", () => {
    fixture({ [MODULE]: `"${PRODUCT}" and community\n` });
    const problems = allowlistProblems([{ file: MODULE, exempt: "community" }], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("matches no marker");
  });

  test("reports a file that is missing from the scan: absent, ignored, or out of scope", () => {
    fixture({ ".gitignore": "packages/x/ignored/\n", "packages/x/ignored/names.ts": `"${PRODUCT}"\n`, "docs/names.ts": `"${PRODUCT}"\n` });
    for (const file of [MODULE, "packages/x/ignored/names.ts", "docs/names.ts"]) {
      const problems = allowlistProblems([{ file, exempt: PRODUCT }], dir);
      expect({ file, problems: problems.length }).toEqual({ file, problems: 1 });
      expect(problems[0]).toContain("missing from the scan");
    }
  });

  test("reports a string that no longer occurs in its file as stale", () => {
    fixture({ [MODULE]: "export const names = {};\n" });
    const problems = allowlistProblems([valid], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("stale");
  });
});

// ---------------------------------------------------------------------------
// The absence-test convention: a test that proves Pro code is gone must not
// contain the marker it looks for.
// ---------------------------------------------------------------------------

describe("absence-test convention", () => {
  // Source lines as an absence test writes them: the needle is assembled at run time.
  const FRAGMENT_TS = [
    'const needle = ["pro", "tier"].join("_");',
    'const re = new RegExp(["bound", "ten" + "ant", "schema"].join("_"));',
    'const event = "sync:" + "status";',
  ].join("\n");
  const FRAGMENT_RS = ['let flag = concat!("--edi", "tion");', 'let var = ["NODESPACE", "PRO", "SERVER_URL"].join("_");'].join("\n");
  // The same lines with each needle written out, as a test that ignores the convention would.
  const LITERAL_TS = [`const needle = "${CMD}";`, `const re = new RegExp("${j("bound_", TEN, "_schema")}");`, `const event = "${j("sync", ":status")}";`].join("\n");
  const LITERAL_RS = [`let flag = "${j("--edi", "tion")}";`, `let var = "${j("NODESPACE", "_PRO_SERVER_URL")}";`].join("\n");

  function scan(ts: string, rs: string): MarkerCounts {
    write("packages/x/tests/absence.test.ts", `${ts}\n`);
    write("packages/x/tests/it/absence.rs", `${rs}\n`);
    return countMarkers(["packages/x/tests/absence.test.ts", "packages/x/tests/it/absence.rs"], dir, []).counts;
  }

  test("needles built from fragments count 0 for every marker", () => {
    expect(scan(FRAGMENT_TS, FRAGMENT_RS)).toEqual(zeroCounts());
  });

  test("the same needles written out count (the control that keeps the fragment test honest)", () => {
    const counts = scan(LITERAL_TS, LITERAL_RS);
    expect(counts.proCommands).toBeGreaterThan(0);
    expect(counts.proDataModel).toBeGreaterThan(0);
    expect(counts.proEvents).toBeGreaterThan(0);
    expect(counts.editionBranching).toBeGreaterThan(0);
  });

  test("each fragment form, alone, is invisible to every marker", () => {
    for (const line of [...FRAGMENT_TS.split("\n"), ...FRAGMENT_RS.split("\n")]) {
      for (const name of LINE_MARKERS) {
        expect(MARKERS[name].pattern.test(line)).toBe(false);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// changedFilesSinceMain
// ---------------------------------------------------------------------------

describe("changedFilesSinceMain", () => {
  test("lists what was committed since origin/main, edited, staged or left untracked, and nothing else", () => {
    repoWithOriginMain();
    write("packages/a/committed.ts", "x\n");
    expect(git(dir, "add", "packages/a/committed.ts").status).toBe(0);
    expect(git(dir, "commit", "--quiet", "-m", "branch work").status).toBe(0);
    write("packages/a/edited.ts", "changed\n");
    write("packages/a/staged.ts", "x\n");
    expect(git(dir, "add", "packages/a/staged.ts").status).toBe(0);
    write("packages/a/untracked.ts", "x\n");
    write("ignored/build.ts", "x\n");
    expect([...changedFilesSinceMain(dir)].sort()).toEqual([
      "packages/a/committed.ts",
      "packages/a/edited.ts",
      "packages/a/staged.ts",
      "packages/a/untracked.ts",
    ]);
  });

  test("diffs from the merge-base, so files that only origin/main changed since the branch forked are not listed", () => {
    repoWithOriginMain();
    const fork = git(dir, "rev-parse", "HEAD").stdout.trim();
    // main moves on after the branch forked...
    write("packages/a/main-only.ts", "x\n");
    expect(git(dir, "add", "packages/a/main-only.ts").status).toBe(0);
    expect(git(dir, "commit", "--quiet", "-m", "main moves on").status).toBe(0);
    expect(git(dir, "update-ref", "refs/remotes/origin/main", "HEAD").status).toBe(0);
    // ...while the branch adds its own commit on top of the fork point.
    expect(git(dir, "checkout", "--quiet", "--detach", fork).status).toBe(0);
    write("packages/a/branch-only.ts", "x\n");
    expect(git(dir, "add", "packages/a/branch-only.ts").status).toBe(0);
    expect(git(dir, "commit", "--quiet", "-m", "branch work").status).toBe(0);
    expect(changedFilesSinceMain(dir)).toEqual(["packages/a/branch-only.ts"]);
  });

  test("lists nothing on a clean checkout of origin/main", () => {
    repoWithOriginMain();
    expect(changedFilesSinceMain(dir)).toEqual([]);
  });

  test("returns [] when origin/main does not exist, and outside a git checkout", () => {
    initRepo();
    write("packages/a/x.ts", "x\n");
    expect(changedFilesSinceMain(dir)).toEqual([]);
    const elsewhere = mkdtempSync(join(tmpdir(), "check-pro-boundary-nogit-"));
    try {
      withEnv({ GIT_CEILING_DIRECTORIES: dirname(elsewhere) }, () => {
        expect(changedFilesSinceMain(elsewhere)).toEqual([]);
      });
    } finally {
      rmSync(elsewhere, { recursive: true, force: true });
    }
  });

  test('a marker added on a branch is listed under "In files this branch changed" in the failure', () => {
    repoWithOriginMain();
    write("packages/a/new.ts", `await invoke('${CMD}');\n`);
    const { hits } = countMarkers(listScannedFiles(dir), dir, []);
    const message = hitFailures(hits, changedFilesSinceMain(dir))[0];
    expect(message).toContain("proCommands");
    expect(message).toContain(`In files this branch changed:\npackages/a/new.ts:1: await invoke('${CMD}');`);
  });
});

// ---------------------------------------------------------------------------
// hitFailures: the hard ban
// ---------------------------------------------------------------------------

describe("hitFailures", () => {
  test("is empty when there is no hit", () => {
    expect(hitFailures(noHits())).toEqual([]);
  });

  test("a single hit under any marker fails, naming that marker only", () => {
    for (const name of MARKER_NAMES) {
      const hit = name === "proNamedFiles" ? "packages/a/x.ts" : "packages/a/x.ts:1: x";
      const failures = hitFailures({ ...noHits(), [name]: [hit] });
      expect({ name, failures: failures.length }).toEqual({ name, failures: 1 });
      expect(failures[0].startsWith(`${name} (`)).toBe(true);
      expect(failures[0]).toContain("1 hit; the hard ban allows none.");
    }
  });

  test("names the marker, its summary and the count, and points at ADR-081, fragments and a pattern fix", () => {
    const message = hitFailures({ ...noHits(), proCommands: ["packages/a/x.ts:1: a", "packages/a/y.ts:2: b"] })[0];
    expect(message).toContain(`proCommands (${MARKERS.proCommands.summary}): 2 hits`);
    expect(message).toContain("ADR-081");
    expect(message).toContain("Pro repository");
    expect(message).toContain("fragments");
    expect(message).toContain("narrow, tested fix to the marker's pattern");
  });

  test("offers no way around the ban: no baseline to raise and no exemption", () => {
    const message = hitFailures({ ...noHits(), proWording: ["packages/a/x.ts:1: a"] })[0].toLowerCase();
    for (const word of ["baseline", "raise", "exemption", "allowlist"]) expect(message).not.toContain(word);
  });

  test("lists hits in changed files first, then every hit", () => {
    const all = ["packages/a/old.ts:1: a", "packages/a/new.ts:7: b", "packages/a/other.ts:3: c"];
    const message = hitFailures({ ...noHits(), proCommands: all }, ["packages/a/new.ts"])[0];
    const changedAt = message.indexOf("In files this branch changed:");
    const allAt = message.indexOf("All hits:");
    expect(changedAt).toBeGreaterThan(-1);
    expect(allAt).toBeGreaterThan(changedAt);
    const changedSection = message.slice(changedAt, allAt);
    expect(changedSection).toContain("packages/a/new.ts:7: b");
    expect(changedSection).not.toContain("old.ts");
    expect(changedSection).not.toContain("other.ts");
    for (const hit of all) expect(message.slice(allAt)).toContain(hit);
  });

  test("omits the changed-files section when no hit is in a changed file", () => {
    const message = hitFailures({ ...noHits(), proCommands: ["packages/a/old.ts:1: a"] }, ["packages/b/x.ts"])[0];
    expect(message).not.toContain("In files this branch changed:");
    expect(message).toContain("All hits:");
  });

  test("matches proNamedFiles hits, which are bare paths, against changed files", () => {
    const [oldFile, newFile] = [j("packages/a/pro", "-old.ts"), j("packages/a/pro", "-new.ts")];
    const message = hitFailures({ ...noHits(), proNamedFiles: [oldFile, newFile] }, [newFile])[0];
    const changedSection = message.slice(message.indexOf("In files this branch changed:"), message.indexOf("All hits:"));
    expect(changedSection).toContain(newFile);
    expect(changedSection).not.toContain(oldFile);
  });

  test("one message per marker with hits, in marker order", () => {
    const failures = hitFailures({ ...noHits(), proNamedFiles: ["packages/a/x"], proWording: ["packages/a/x.ts:1: a"], proCommands: ["packages/a/x.ts:2: b"] });
    expect(failures.map((failure) => failure.slice(0, failure.indexOf(" (")))).toEqual(["proCommands", "proWording", "proNamedFiles"]);
  });
});

describe("hard-ban shape", () => {
  test("the checker exports no baselines, no exemptions and no tightening helpers", () => {
    const exported = Object.keys(checker);
    for (const name of ["BASELINES", "EXEMPTIONS", "EXEMPTIBLE_NON_TEST_FILES", "TEST_PATH", "formatBaselines", "baselineFailures", "baselineNotices", "exemptionProblems"]) {
      expect(exported).not.toContain(name);
    }
  });

  test("markers: every line marker, then proNamedFiles", () => {
    expect([...MARKER_NAMES]).toEqual([...LINE_MARKERS, "proNamedFiles"]);
  });
});

// ---------------------------------------------------------------------------
// The checker as a command
// ---------------------------------------------------------------------------

describe("CLI", () => {
  function run(...args: string[]): { status: number; stdout: string; stderr: string } {
    const result = spawnSync("bun", ["run", CHECKER_PATH, ...args], { encoding: "utf8", env: gitEnv() });
    return { status: result.status ?? -1, stdout: result.stdout, stderr: result.stderr };
  }

  // The checker locates its repository from its own path, so a copy placed
  // in a fixture repository scans that repository. The copy's allowlist is
  // empty: the real entry names a file a fixture does not have, which would
  // fail every fixture run as a missing allowlisted file.
  function runInFixture(...args: string[]): { status: number; stdout: string; stderr: string } {
    return runInFixtureWithAllowlist([], ...args);
  }

  // The same, with the copy's ALLOWLIST replaced by `entries`.
  function runInFixtureWithAllowlist(entries: readonly AllowlistEntry[], ...args: string[]): { status: number; stdout: string; stderr: string } {
    mkdirSync(join(dir, "scripts"), { recursive: true });
    const declaration = /export const ALLOWLIST: readonly \{ file: string; exempt: string \}\[\] = \[[^\]]*\];/;
    const source = readFileSync(CHECKER_PATH, "utf8");
    expect(source).toMatch(declaration);
    const replaced = `export const ALLOWLIST: readonly { file: string; exempt: string }[] = ${JSON.stringify(entries)};`;
    writeFileSync(join(dir, "scripts/check-pro-boundary.ts"), source.replace(declaration, () => replaced));
    return runCopy(args);
  }

  function runCopy(args: readonly string[]): { status: number; stdout: string; stderr: string } {
    // The ceiling keeps git from finding a repository above a fixture that has none.
    const env = { ...gitEnv(), GIT_CEILING_DIRECTORIES: dirname(dir) };
    const result = spawnSync("bun", ["run", join(dir, "scripts/check-pro-boundary.ts"), ...args], { encoding: "utf8", env });
    return { status: result.status ?? -1, stdout: result.stdout, stderr: result.stderr };
  }

  test("--changed prints a line per marker", () => {
    const { status, stdout } = run("--changed");
    expect(status).toBe(0);
    for (const name of MARKER_NAMES) expect(stdout).toMatch(new RegExp(`^${name}: \\d+$`, "m"));
  });

  test("with no hit: prints every marker at 0, exits 0, and prints nothing on stderr", () => {
    repoWithOriginMain();
    write("packages/a/x.ts", "export const clean = true;\n");
    const { status, stdout, stderr } = runInFixture();
    expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
    for (const name of MARKER_NAMES) expect(stdout).toMatch(new RegExp(`^${name} +0$`, "m"));
    expect(stdout).toContain("No Pro marker in core");
  });

  test("a single hit exits 1, listing the line under \"In files this branch changed\", with no paste block", () => {
    repoWithOriginMain();
    write("packages/a/x.ts", `// ${PRODUCT}\n`);
    const { status, stdout, stderr } = runInFixture();
    expect(status).toBe(1);
    expect(stdout).toMatch(/^productName +1$/m);
    expect(stderr).toContain(`In files this branch changed:\npackages/a/x.ts:1: // ${PRODUCT}`);
    expect(`${stdout}${stderr}`.toLowerCase()).not.toContain("paste");
  });

  test("a single Pro-named file exits 1", () => {
    repoWithOriginMain();
    write(j("packages/a/pro", "-plugin.ts"), "export {};\n");
    const { status, stderr } = runInFixture();
    expect(status).toBe(1);
    expect(stderr).toContain("proNamedFiles");
  });

  test("an allowlisted string passes end to end, and every other hit in that file still fails", () => {
    repoWithOriginMain();
    const names = "packages/a/extension-names.ts";
    write(names, `export const NAMES = { pro: "${PRODUCT}" };\n`);
    expect(runInFixtureWithAllowlist([{ file: names, exempt: PRODUCT }]).status).toBe(0);
    write(names, `export const NAMES = { pro: "${PRODUCT}" };\n// ${DAEMON}\n`);
    const { status, stderr } = runInFixtureWithAllowlist([{ file: names, exempt: PRODUCT }]);
    expect(status).toBe(1);
    expect(stderr).toContain(`${names}:2: // ${DAEMON}`);
    expect(stderr).not.toContain(`${names}:1:`);
  });

  test("an allowlist problem fails the command even with no marker hit", () => {
    repoWithOriginMain();
    write("packages/a/extension-names.ts", "export const NAMES = {};\n");
    const { status, stderr } = runInFixtureWithAllowlist([{ file: "packages/a/extension-names.ts", exempt: PRODUCT }]);
    expect(status).toBe(1);
    expect(stderr).toContain("stale");
  });

  test("--list <marker> prints a fixture's hits grouped by file", () => {
    repoWithOriginMain();
    write("packages/a/x.ts", `clean\n${CMD}();\n${CMD2}();\n`);
    write("packages/a/y.ts", `${CMD}();\n`);
    const { status, stdout } = runInFixture("--list", "proCommands");
    expect(status).toBe(0);
    expect(stdout).toBe(`packages/a/x.ts\n  2: ${CMD}();\n  3: ${CMD2}();\npackages/a/y.ts\n  1: ${CMD}();\n`);
  });

  test("--list proNamedFiles prints bare paths", () => {
    repoWithOriginMain();
    const plugin = j("packages/a/pro", "-plugin.ts");
    write(plugin, "clean\n");
    const { status, stdout } = runInFixture("--list", "proNamedFiles");
    expect(status).toBe(0);
    expect(stdout).toBe(`${plugin}\n`);
  });

  test("--changed prints only the hits in files this branch changed", () => {
    repoWithOriginMain();
    write("packages/a/old.ts", `${CMD2}();\n`);
    expect(git(dir, "add", "packages/a/old.ts").status).toBe(0);
    expect(git(dir, "commit", "--quiet", "-m", "before the fork").status).toBe(0);
    expect(git(dir, "update-ref", "refs/remotes/origin/main", "HEAD").status).toBe(0);
    write("packages/a/new.ts", `${CMD}();\n${PRODUCT}\n`);
    const { status, stdout } = runInFixture("--changed");
    expect(status).toBe(0);
    expect(stdout).toContain(`proCommands: 1\n  packages/a/new.ts:1: ${CMD}();\n`);
    expect(stdout).toContain(`productName: 1\n  packages/a/new.ts:2: ${PRODUCT}\n`);
    expect(stdout).not.toContain(CMD2);
    expect(stdout).toMatch(/^cloudAccountWording: 0$/m);
  });

  test("an unknown marker or option prints usage and exits 2", () => {
    for (const args of [["--list", "bogus"], ["--list"], ["--nope"], ["--changed", "extra"], ["--list", "proCommands", "extra"], [""], ["--list", ""]]) {
      const { status, stderr } = run(...args);
      expect(status).toBe(2);
      expect(stderr).toContain("Usage:");
    }
  });

  test("a usage error is reported before the repository is scanned, so it needs no git checkout", () => {
    // The fixture is not a git repository, so scanning it throws. Exit 2 with
    // the usage text proves the command line was validated first.
    for (const args of [["--nope"], ["--list", "bogus"], ["--list"], ["--changed", "extra"]]) {
      const { status, stderr } = runInFixture(...args);
      expect({ args, status }).toEqual({ args, status: 2 });
      expect(stderr).toContain("Usage:");
      expect(stderr).not.toContain("needs a git checkout");
    }
  });

  test("a valid command outside a git checkout fails with the clear error, not a silent pass", () => {
    const { status, stderr } = runInFixture();
    expect(status).not.toBe(0);
    expect(stderr).toContain("needs a git checkout");
  });
});

// ---------------------------------------------------------------------------
// The real repository
// ---------------------------------------------------------------------------

describe("real-repo hard ban", () => {
  test("no Pro marker anywhere in scope, and the allowlist is valid", () => {
    // The enforcement path (bun test scripts/ -> test:scripts -> the merge
    // gate). A bare toEqual() would fail with a count diff and no hint of
    // what to do, so this throws the message the CLI prints.
    const { hits } = countMarkers(listScannedFiles());
    const messages = [...allowlistProblems(), ...hitFailures(hits, changedFilesSinceMain())];
    if (messages.length > 0) throw new Error(messages.join("\n\n"));
  });

  test("the command exits 0 with every marker at 0", () => {
    const result = spawnSync("bun", ["run", CHECKER_PATH], { encoding: "utf8", env: gitEnv() });
    expect({ status: result.status, stderr: result.stderr }).toEqual({ status: 0, stderr: "" });
    for (const name of MARKER_NAMES) expect(result.stdout).toMatch(new RegExp(`^${name} +0$`, "m"));
  });

  // ADR-081 section 8: exactly one file, the display-name module for the
  // Pro-database refusal, and in it only the product name.
  const NAMES_MODULE = "packages/proto/src/extension_names.rs";

  test("the allowlist is the one entry ADR-081 section 8 allows", () => {
    expect(ALLOWLIST).toEqual([{ file: NAMES_MODULE, exempt: PRODUCT }]);
  });

  test("the entry is what keeps the display-name module clean: without it the module counts", () => {
    expect(countMarkers([NAMES_MODULE]).counts).toEqual(zeroCounts());
    const without = countMarkers([NAMES_MODULE], undefined, []).counts;
    expect(without.productName).toBeGreaterThan(0);
  });

  test("in the display-name module the entry hides only the product name: any other marker still counts", () => {
    write(NAMES_MODULE, `"${PRODUCT}"\n// ${WORD}\n"${PRODUCT}" via ${DAEMON}\nfn ${CMD}() {}\n`);
    const { hits } = countMarkers([NAMES_MODULE], dir, ALLOWLIST);
    expect(hits.productName).toEqual([]);
    expect(hits.proWording).toEqual([`${NAMES_MODULE}:2: // ${WORD}`, `${NAMES_MODULE}:3: "${PRODUCT}" via ${DAEMON}`]);
    expect(hits.cloudWording).toEqual([`${NAMES_MODULE}:3: "${PRODUCT}" via ${DAEMON}`]);
    expect(hits.proCommands).toEqual([`${NAMES_MODULE}:4: fn ${CMD}() {}`]);
  });

  test("the entry applies to no other file", () => {
    write("packages/proto/src/lib.rs", `"${PRODUCT}"\n`);
    const { hits } = countMarkers(["packages/proto/src/lib.rs"], dir, ALLOWLIST);
    expect(hits.productName).toEqual([`packages/proto/src/lib.rs:1: "${PRODUCT}"`]);
  });

  test("scans packages/agent and README.md, and leaves out CLAUDE.md and the checker's own files", () => {
    const files = listScannedFiles();
    expect(files.some((file) => file.startsWith("packages/agent/"))).toBe(true);
    expect(files).toContain("README.md");
    expect(files).not.toContain("CLAUDE.md");
    for (const excluded of EXCLUDED_FILES) expect(files).not.toContain(excluded);
  });

  test("gitignored build output never enters the scan", () => {
    for (const file of listScannedFiles()) {
      expect(file).not.toMatch(/(^|\/)(node_modules|target|\.svelte-kit)\//);
      expect(file).not.toContain("src-tauri/resources/skill/");
      expect(file).not.toContain(j("nodespaced", "-pro-"));
    }
  });
});
