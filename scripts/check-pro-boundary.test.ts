// Covers the Pro-boundary ratchet (scripts/check-pro-boundary.ts). Pattern
// tests run each marker's regex against tables of lines that must and must
// not match. Discovery, exemption and layout tests build a throwaway git
// repository per test, because the behavior under test is git's: what it
// tracks, what it ignores, and how it replays commits. The real-repo block at
// the bottom is the enforcement path (`bun run test:scripts`, so every merge
// gate) and throws the same actionable message the CLI prints.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import {
  BASELINES,
  EXCLUDED_FILES,
  EXEMPTIBLE_NON_TEST_FILES,
  EXEMPTIONS,
  MARKERS,
  TEST_PATH,
  baselineFailures,
  baselineNotices,
  changedFilesSinceMain,
  countMarkers,
  exemptionProblems,
  formatBaselines,
  isProNamedFile,
  listScannedFiles,
  type Exemption,
  type LineMarkerName,
  type MarkerCounts,
  type MarkerName,
} from "./check-pro-boundary";

// Many tests spawn git or bun processes. Bun's 5s default per-test timeout is
// tight on the loaded machines these tests run on (the merge gate shares them
// with Rust builds), and a timeout here would eject an unrelated PR.
setDefaultTimeout(30_000);

const CHECKER_PATH = join(dirname(new URL(import.meta.url).pathname), "check-pro-boundary.ts");
const LINE_MARKERS = Object.keys(MARKERS) as LineMarkerName[];
const ALL_MARKERS: MarkerName[] = [...LINE_MARKERS, "proNamedFiles"];

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

function countsWith(overrides: Partial<MarkerCounts> = {}): MarkerCounts {
  return { ...BASELINES, ...overrides };
}

// ---------------------------------------------------------------------------
// Pattern tables. A fixture with regex-special characters is built in code,
// as a string, never through a shell.
// ---------------------------------------------------------------------------

const PATTERN_CASES: Record<LineMarkerName, { match: string[]; noMatch: string[] }> = {
  proCommands: {
    match: ["invoke('pro_list_members')", "commands::pro_sync::pro_tier,", "pub mod pro_client;"],
    noMatch: ["is_pro_build()", "approve_request", "macro_rules!"],
  },
  proSyncModule: {
    match: [
      "import { proSync } from '$lib/stores/pro-sync.svelte'",
      "resolveProSyncVariant()",
      "isProSyncActive()",
      "import { x } from './pro-sync.svelte'",
      "type ProSyncState = {};",
    ],
    noMatch: ["project-sync", "prosync"],
  },
  proProtocol: {
    match: [
      'tonic::include_proto!("nodespace.pro.v1")',
      "CloudSyncServiceClient",
      "app.try_state::<ProClient>()",
      "let tier: ProTier = x;",
      '"https://pro.nodespace.ai"',
      "http://127.0.0.1:8787/v1",
      "// see nodespace_pro.proto",
    ],
    noMatch: ["use nodespace_proto::socket;", "ProcessTier", "ProClientele"],
  },
  proEvents: {
    match: ["listen('pro:tier-detected', cb)", 'app.emit("sync:status", p)', "listen(`sync:error`, cb)", "listen('sync:status', cb)", 'emit("sync:error")'],
    noMatch: ["sync:status updates", "the sync:error path", "sync:statuses"],
  },
  membershipService: {
    match: [
      "import { membershipService } from '$lib/services/membership-service'",
      "class MembershipService {}",
      "import { svc } from '$lib/services/membership-service';",
      "const x = membershipService.list();",
    ],
    noMatch: ["collection membership", "member_of"],
  },
  editionBranching: {
    match: [
      "is_pro_build()",
      "fn f(is_debug: bool, is_pro: bool)",
      'option_env!("NODESPACE_PRO_SUPABASE_URL")',
      "NODESPACED_PRO_ANON_KEY",
      'cfg!(feature = "pro")',
      '"daemon-dev-pro.sock"',
      '"ui-pro.pid"',
      '"incompatible-database-pro.json"',
      "app.nodespace.daemon.pro",
      "app.nodespace.daemon.dev.pro",
      "tauri.pro.conf.json",
      "nodespaced --edition",
      "NODESPACE_FORCE_COMMUNITY",
      "nodespaced-pro-aarch64",
      "PRO_DAEMON_BINARY_NAME",
    ],
    noMatch: ["rustfmt --edition 2021", 'edition = "2021"', '"daemon-dev.sock"', "app.nodespace.daemon.dev", "rustfmt --edition=2024", "is_production"],
  },
  cloudBindState: {
    match: ["bound_tenant_schema", "boundTenantCollection", "sync_enabled", "labsFlags.syncEnabled", "auth_status", "authStatus"],
    noMatch: ["synced", "authenticate"],
  },
  cloudSyncHooks: {
    match: ["apply_remote_embeddings", "embeddings_modified_since", "get_multi_membership_edges"],
    noMatch: ["upsert_embeddings"],
  },
  cloudWording: {
    match: ["Supabase", "supabase", "Postgres RLS", "pgvector", "nodespace-sync", "nodespace_sync::", "the Pro daemon", "the pro daemon"],
    noMatch: ["URLS", "sync service"],
  },
  tenantWording: {
    match: ["tenant", "TENANT_ADMIN_ROLES", "boundTenant"],
    noMatch: ["maintenance"],
  },
  proWording: {
    match: ["// the Pro build", "NodeSpace Pro", "Pro-only", "Pro"],
    noMatch: ["M2 Pro", "DeepSeek V4 Pro", "MacBook Pro", "iPad Pro", "iPhone Pro", "Protocol", "Provider", "pro", "NodeSpaceProduct"],
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
});

describe("isProNamedFile", () => {
  for (const path of [
    "pro-plugin.ts",
    "pro_sync.rs",
    "tauri.pro.conf.json",
    "nodespace_pro.proto",
    "first-pro-consent-modal.svelte",
    "pro-sync.svelte.ts",
    "packages/desktop-app/src/lib/plugins/pro-plugin.ts",
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
    // Only the basename counts: a directory named for a Pro segment does not.
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
    write("packages/desktop-app/src-tauri/.gitignore", "resources/skill/\nbinaries/nodespaced-pro-*\n");
    write("packages/desktop-app/src-tauri/resources/skill/SKILL.md", "NodeSpace Pro\n");
    write("packages/desktop-app/src-tauri/binaries/nodespaced-pro-x", "binary\n");
    write("packages/desktop-app/src-tauri/src/lib.rs", "// clean\n");
    const files = listScannedFiles(dir);
    expect(files).toEqual(["packages/desktop-app/src-tauri/.gitignore", "packages/desktop-app/src-tauri/src/lib.rs"]);
    const { counts } = countMarkers(files, dir);
    expect(counts.proWording).toBe(0);
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
    write("CLAUDE.md", "pro_x tenant NodeSpace Pro Supabase is_pro_build proSync\n");
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
      "scripts/pkg-resources/postinstall",
      "packages/a/.gitignore",
      "packages/a/.eslintrc.json",
    ];
    const skipped = ["packages/a/x.png", "packages/a/x.lock", "packages/a/x.txt", "packages/a/x.svg", "packages/a/x."];
    for (const path of [...scanned, ...skipped]) write(path, "x\n");
    expect(listScannedFiles(dir)).toEqual([...scanned].sort());
  });

  test("skips the checker's own two files", () => {
    initRepo();
    write("scripts/check-pro-boundary.ts", "pro_x\n");
    write("scripts/check-pro-boundary.test.ts", "pro_x\n");
    write("scripts/other.ts", "x\n");
    expect(EXCLUDED_FILES).toEqual(["scripts/check-pro-boundary.ts", "scripts/check-pro-boundary.test.ts"]);
    expect(listScannedFiles(dir)).toEqual(["scripts/other.ts"]);
  });

  test("a file deleted from the working tree but still in the index does not throw", () => {
    initRepo();
    write("packages/a/gone.ts", "pro_x\n");
    write("packages/a/pro-gone.ts", "x\n");
    write("packages/a/kept.ts", "pro_y\n");
    expect(git(dir, "add", "packages").status).toBe(0);
    unlinkSync(join(dir, "packages/a/gone.ts"));
    unlinkSync(join(dir, "packages/a/pro-gone.ts"));
    const files = listScannedFiles(dir);
    expect(files).toContain("packages/a/gone.ts");
    const { counts, hits } = countMarkers(files, dir);
    expect(counts.proCommands).toBe(1);
    expect(hits.proCommands).toEqual(["packages/a/kept.ts:1: pro_y"]);
    expect(counts.proNamedFiles).toBe(0);
  });

  test("a directory that is not a git repository throws a clear error, never an empty list", () => {
    write("packages/a/lib.rs", "pro_x\n");
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
    write("packages/a/lib.rs", "pro_a(); pro_b(); pro_c();\n");
    const { counts } = countMarkers(["packages/a/lib.rs"], dir);
    expect(counts.proCommands).toBe(1);
  });

  test("one line can count under several markers, once each", () => {
    write("packages/a/lib.rs", "invoke('pro_x'); // the tenant sees NodeSpace Pro\n");
    const { counts } = countMarkers(["packages/a/lib.rs"], dir);
    expect(counts.proCommands).toBe(1);
    expect(counts.tenantWording).toBe(1);
    expect(counts.proWording).toBe(1);
    expect(counts.cloudWording).toBe(0);
  });

  test("counts one hit per matching line across lines and files", () => {
    write("packages/a/one.rs", "pro_a\nclean\npro_b\n");
    write("packages/a/two.rs", "pro_c\n");
    const { counts } = countMarkers(["packages/a/one.rs", "packages/a/two.rs"], dir);
    expect(counts.proCommands).toBe(3);
  });

  test("hits are repo-relative path:line: trimmed text", () => {
    write("packages/a/lib.rs", "clean\n    pro_x();   \n// tenant\n");
    const { hits } = countMarkers(["packages/a/lib.rs"], dir);
    expect(hits.proCommands).toEqual(["packages/a/lib.rs:2: pro_x();"]);
    expect(hits.tenantWording).toEqual(["packages/a/lib.rs:3: // tenant"]);
  });

  test("proNamedFiles counts Pro-named paths, and its hits are the paths", () => {
    write("packages/a/pro-plugin.ts", "clean\n");
    write("packages/a/provider.ts", "clean\n");
    write("packages/a/tauri.pro.conf.json", "{}\n");
    const { counts, hits } = countMarkers(["packages/a/pro-plugin.ts", "packages/a/provider.ts", "packages/a/tauri.pro.conf.json"], dir);
    expect(counts.proNamedFiles).toBe(2);
    expect(hits.proNamedFiles).toEqual(["packages/a/pro-plugin.ts", "packages/a/tauri.pro.conf.json"]);
  });

  test("returns every marker, at 0 for a clean file", () => {
    write("packages/a/lib.rs", "fn main() {}\n");
    const { counts, hits } = countMarkers(["packages/a/lib.rs"], dir);
    expect(Object.keys(counts)).toEqual(ALL_MARKERS);
    expect(Object.keys(hits)).toEqual(ALL_MARKERS);
    expect(Object.values(counts).every((n) => n === 0)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Exemptions
// ---------------------------------------------------------------------------

describe("EXEMPTIONS", () => {
  test("holds exactly the agent fixture, exempt from tenantWording only", () => {
    expect(EXEMPTIONS).toHaveLength(1);
    expect(EXEMPTIONS[0].file).toBe("packages/agent/tests/it/live_embedding_prefix_measurement.rs");
    expect(EXEMPTIONS[0].markers).toEqual(["tenantWording"]);
    expect(EXEMPTIONS[0].reason.trim()).not.toBe("");
  });
});

describe("countMarkers — exemptions", () => {
  const exempt: Exemption = { file: "packages/x/tests/a.rs", markers: ["tenantWording"], reason: "fixture prose" };

  test("an entry hides only its own markers in its own file", () => {
    write("packages/x/tests/a.rs", "a tenant\nNodeSpace Pro\n");
    write("packages/x/tests/b.rs", "a tenant\n");
    const { counts } = countMarkers(["packages/x/tests/a.rs", "packages/x/tests/b.rs"], dir, [exempt]);
    // a.rs: its tenant line is hidden but its Pro line still counts; b.rs: the same marker still counts.
    expect(counts.tenantWording).toBe(1);
    expect(counts.proWording).toBe(1);
  });

  test("without the entry the same files count the hidden line", () => {
    write("packages/x/tests/a.rs", "a tenant\nNodeSpace Pro\n");
    write("packages/x/tests/b.rs", "a tenant\n");
    const { counts } = countMarkers(["packages/x/tests/a.rs", "packages/x/tests/b.rs"], dir, []);
    expect(counts.tenantWording).toBe(2);
  });

  test("two entries for one file both apply", () => {
    write("packages/x/tests/a.rs", "a tenant\nNodeSpace Pro\n");
    const { counts } = countMarkers(["packages/x/tests/a.rs"], dir, [exempt, { file: exempt.file, markers: ["proWording"], reason: "prose" }]);
    expect(counts.tenantWording).toBe(0);
    expect(counts.proWording).toBe(0);
  });
});

describe("exemptionProblems", () => {
  function fixture(files: Record<string, string>): void {
    initRepo();
    for (const [path, content] of Object.entries(files)) write(path, content);
  }

  const valid: Exemption = { file: "packages/x/tests/a.rs", markers: ["tenantWording"], reason: "fixture prose" };

  test("is empty for a live entry", () => {
    fixture({ "packages/x/tests/a.rs": "a tenant\n" });
    expect(exemptionProblems([valid], dir)).toEqual([]);
  });

  test("reports a file that is missing from the scan", () => {
    fixture({ "packages/x/tests/other.rs": "a tenant\n" });
    const problems = exemptionProblems([valid], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("missing from the scan");
    expect(problems[0]).toContain(valid.file);
  });

  test("reports a file that git ignores as missing from the scan", () => {
    fixture({ ".gitignore": "packages/x/tests/\n", "packages/x/tests/a.rs": "a tenant\n" });
    expect(exemptionProblems([valid], dir).join("\n")).toContain("missing from the scan");
  });

  test("reports a path that is not a test path", () => {
    fixture({ "packages/x/src/lib.rs": "a tenant\n" });
    const problems = exemptionProblems([{ ...valid, file: "packages/x/src/lib.rs" }], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("only test files");
  });

  describe("installer recognition points", () => {
    const RECOGNITION_POINTS = [
      "scripts/update-homebrew-cask.ts",
      "scripts/pkg-resources/preinstall",
      "scripts/pkg-resources/postinstall",
      "scripts/build-pkg.sh",
    ];
    const installer = (file: string): Exemption => ({ file, markers: ["proWording"], reason: "names the other NodeSpace product it refuses to install over" });

    test("the allowlist is exactly the four installer recognition points", () => {
      expect([...EXEMPTIBLE_NON_TEST_FILES].sort()).toEqual([...RECOGNITION_POINTS].sort());
      for (const file of EXEMPTIBLE_NON_TEST_FILES) expect(TEST_PATH.test(file)).toBe(false);
    });

    for (const file of RECOGNITION_POINTS) {
      test(`accepts a live entry on ${file}, which is not a test path`, () => {
        fixture({ [file]: "refuse to install over NodeSpace Pro\n" });
        expect(exemptionProblems([installer(file)], dir)).toEqual([]);
      });

      test(`the entry on ${file} hides only its own marker`, () => {
        write(file, "refuse to install over NodeSpace Pro\na tenant\n");
        const { counts } = countMarkers([file], dir, [installer(file)]);
        expect(counts.proWording).toBe(0);
        expect(counts.tenantWording).toBe(1);
      });
    }

    test("an allowlisted file still needs a live hit, a reason and a marker", () => {
      const file = "scripts/update-homebrew-cask.ts";
      fixture({ [file]: "a plain installer line\n" });
      expect(exemptionProblems([installer(file)], dir).join("\n")).toContain("proWording has no hit left");
      expect(exemptionProblems([{ ...installer(file), reason: "" }], dir).join("\n")).toContain("reason is empty");
      expect(exemptionProblems([{ ...installer(file), markers: [] }], dir).join("\n")).toContain("names no marker");
    });

    test("does not cover a same-named file in another directory: the match is on the whole path", () => {
      const others = ["packages/x/scripts/update-homebrew-cask.ts", "scripts/other/update-homebrew-cask.ts", "scripts/pkg-resources/other/postinstall"];
      fixture(Object.fromEntries(others.map((file) => [file, "refuse to install over NodeSpace Pro\n"])));
      for (const file of others) {
        const problems = exemptionProblems([installer(file)], dir);
        expect({ file, problems: problems.length }).toEqual({ file, problems: 1 });
        expect(problems[0]).toContain("only test files");
      }
    });

    test("any other non-test file is still rejected, including a sibling installer script", () => {
      const others = ["scripts/build-pkg.ts", "scripts/pkg-resources/app.nodespace.daemon.plist", "scripts/refresh-pro-proto.ts", "packages/x/src/lib.rs"];
      fixture(Object.fromEntries(others.map((file) => [file, "refuse to install over NodeSpace Pro\n"])));
      for (const file of others) {
        const problems = exemptionProblems([installer(file)], dir);
        expect({ file, problems: problems.length }).toEqual({ file, problems: 1 });
        expect(problems[0]).toContain("EXEMPTIBLE_NON_TEST_FILES");
      }
    });
  });

  test("reports an empty reason, including a whitespace-only one", () => {
    fixture({ "packages/x/tests/a.rs": "a tenant\n" });
    for (const reason of ["", "   "]) {
      const problems = exemptionProblems([{ ...valid, reason }], dir);
      expect(problems).toHaveLength(1);
      expect(problems[0]).toContain("reason is empty");
    }
  });

  test("reports an entry that names no marker", () => {
    fixture({ "packages/x/tests/a.rs": "a tenant\n" });
    const problems = exemptionProblems([{ ...valid, markers: [] }], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("names no marker");
  });

  test("reports an unknown marker", () => {
    fixture({ "packages/x/tests/a.rs": "a tenant\n" });
    const problems = exemptionProblems([{ ...valid, markers: ["notAMarker" as LineMarkerName] }], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain('"notAMarker" is not a marker that can be exempted');
  });

  test("reports proNamedFiles, which can't be exempted", () => {
    fixture({ "packages/x/tests/a.rs": "a tenant\n" });
    const problems = exemptionProblems([{ ...valid, markers: ["proNamedFiles" as LineMarkerName] }], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain('"proNamedFiles" is not a marker that can be exempted');
  });

  test("reports a marker with no raw hit left in the file", () => {
    fixture({ "packages/x/tests/a.rs": "nothing here\n" });
    const problems = exemptionProblems([valid], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("tenantWording has no hit left");
    expect(problems[0]).toContain("stale");
  });

  test("counts raw hits, so an entry stays live while another exemption hides the line", () => {
    fixture({ "packages/x/tests/a.rs": "a tenant\n" });
    // The stale check reads the raw file, not the exempted counts: it must not
    // call an entry stale merely because exemptions already hide its hit.
    expect(exemptionProblems([valid, { ...valid, reason: "second entry" }], dir)).toEqual([]);
  });

  test("checks each marker of an entry separately", () => {
    fixture({ "packages/x/tests/a.rs": "a tenant\n" });
    const problems = exemptionProblems([{ ...valid, markers: ["tenantWording", "proWording"] }], dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("proWording has no hit left");
  });

  test("an entry with several problems reports each", () => {
    fixture({ "packages/x/src/lib.rs": "nothing\n" });
    const problems = exemptionProblems([{ file: "packages/x/src/lib.rs", markers: ["tenantWording"], reason: "" }], dir);
    expect(problems).toHaveLength(3);
  });
});

describe("TEST_PATH", () => {
  for (const path of [
    "src/tests/stores/x.test.ts",
    "packages/core/tests/it/a.rs",
    "scripts/a.test.ts",
    "src/models/foo_test.rs",
    "src/models/foo_tests.rs",
    "src/x/tests.rs",
    "src/x.spec.ts",
    "src/__tests__/x.ts",
    "packages/agent/tests/it/live_embedding_prefix_measurement.rs",
  ]) {
    test(`matches ${path}`, () => {
      expect(TEST_PATH.test(path)).toBe(true);
    });
  }

  for (const path of ["src/lib/stores/x.svelte.ts", "packages/daemon/src/main.rs", "src/contest.rs", "src/latests.rs", "src/protests/x.ts", "src/x.test.svelte"]) {
    test(`does not match ${path}`, () => {
      expect(TEST_PATH.test(path)).toBe(false);
    });
  }
});

// ---------------------------------------------------------------------------
// The absence-test convention: a test that proves Pro code is gone must not
// contain the marker it looks for.
// ---------------------------------------------------------------------------

describe("absence-test convention", () => {
  const FRAGMENT_TS = [
    'const needle = ["pro", "tier"].join("_");',
    'const re = new RegExp(["ten", "ant"].join(""), "i");',
    'const event = "sync:" + "status";',
  ].join("\n");
  const FRAGMENT_RS = [
    'let flag = concat!("--edi", "tion");',
    'let var = ["NODESPACE", "PRO", "SUPABASE_URL"].join("_");',
  ].join("\n");
  const LITERAL_TS = [
    'const needle = "pro_tier";',
    'const re = new RegExp("tenant", "i");',
    'const event = "sync:status";',
  ].join("\n");
  const LITERAL_RS = ['let flag = "--edition";', 'let var = "NODESPACE_PRO_SUPABASE_URL";'].join("\n");

  function scan(ts: string, rs: string): MarkerCounts {
    write("packages/x/tests/absence.test.ts", `${ts}\n`);
    write("packages/x/tests/it/absence.rs", `${rs}\n`);
    return countMarkers(["packages/x/tests/absence.test.ts", "packages/x/tests/it/absence.rs"], dir, []).counts;
  }

  test("needles built from fragments count 0 for every marker", () => {
    const counts = scan(FRAGMENT_TS, FRAGMENT_RS);
    expect(counts).toEqual(Object.fromEntries(ALL_MARKERS.map((name) => [name, 0])) as MarkerCounts);
  });

  test("the same needles written literally count (the control that keeps the fragment test honest)", () => {
    const counts = scan(LITERAL_TS, LITERAL_RS);
    expect(counts.proCommands).toBeGreaterThan(0);
    expect(counts.tenantWording).toBeGreaterThan(0);
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
// BASELINES layout
// ---------------------------------------------------------------------------

describe("formatBaselines", () => {
  test("the checker's source holds formatBaselines(BASELINES) verbatim", () => {
    expect(readFileSync(CHECKER_PATH, "utf8")).toContain(formatBaselines(BASELINES));
  });

  test("puts exactly one summary comment line before each value line", () => {
    const lines = formatBaselines(countsWith()).split("\n");
    expect(lines[0]).toBe("export const BASELINES = {");
    expect(lines[lines.length - 1]).toBe("} satisfies Record<MarkerName, number>;");
    expect(lines).toHaveLength(2 + 2 * ALL_MARKERS.length);
    ALL_MARKERS.forEach((name, i) => {
      const comment = lines[1 + 2 * i];
      const value = lines[2 + 2 * i];
      expect(comment).toMatch(/^ {2}\/\/ \S.*$/);
      expect(value).toBe(`  ${name}: ${BASELINES[name]},`);
    });
  });

  test("takes each comment from the marker's summary", () => {
    const lines = formatBaselines(countsWith()).split("\n");
    LINE_MARKERS.forEach((name, i) => {
      expect(lines[1 + 2 * i]).toBe(`  // ${MARKERS[name].summary}`);
    });
    expect(lines[1 + 2 * LINE_MARKERS.length]).toBe("  // files whose basename has a pro segment");
  });

  test("renders whatever counts it is given", () => {
    const text = formatBaselines(countsWith({ proCommands: 1, proNamedFiles: 0 }));
    expect(text).toContain("  proCommands: 1,");
    expect(text).toContain("  proNamedFiles: 0,");
    expect(text).toContain(`  proSyncModule: ${BASELINES.proSyncModule},`);
  });

  describe("merge-queue replay (cherry-pick)", () => {
    // Commits the file's text on a detached HEAD forked from `base`.
    function commitFrom(base: string, text: string, message: string): string {
      expect(git(dir, "checkout", "--quiet", "--detach", base).status).toBe(0);
      writeFileSync(join(dir, "baselines.txt"), `${text}\n`);
      expect(git(dir, "commit", "--quiet", "-am", message).status).toBe(0);
      return git(dir, "rev-parse", "HEAD").stdout.trim();
    }

    function setup(render: (counts: MarkerCounts) => string): { base: string } {
      initRepo();
      writeFileSync(join(dir, "baselines.txt"), `${render(countsWith())}\n`);
      expect(git(dir, "add", "baselines.txt").status).toBe(0);
      expect(git(dir, "commit", "--quiet", "-m", "base").status).toBe(0);
      return { base: git(dir, "rev-parse", "HEAD").stdout.trim() };
    }

    function replay(onto: string, pick: string): number {
      expect(git(dir, "checkout", "--quiet", "--detach", onto).status).toBe(0);
      return git(dir, "cherry-pick", pick).status;
    }

    test("PRs that lower different, adjacent markers replay onto each other without conflict", () => {
      const { base } = setup(formatBaselines);
      const lowerCommands = commitFrom(base, formatBaselines(countsWith({ proCommands: BASELINES.proCommands - 1 })), "lower proCommands");
      const lowerSync = commitFrom(base, formatBaselines(countsWith({ proSyncModule: BASELINES.proSyncModule - 1 })), "lower proSyncModule");
      expect(replay(lowerCommands, lowerSync)).toBe(0);
      expect(git(dir, "cherry-pick", "--abort").status).not.toBe(0); // nothing left in progress
      const merged = readFileSync(join(dir, "baselines.txt"), "utf8");
      expect(merged).toContain(`  proCommands: ${BASELINES.proCommands - 1},`);
      expect(merged).toContain(`  proSyncModule: ${BASELINES.proSyncModule - 1},`);
    });

    test("PRs that set the same marker to different values conflict", () => {
      const { base } = setup(formatBaselines);
      const first = commitFrom(base, formatBaselines(countsWith({ proCommands: BASELINES.proCommands - 1 })), "first");
      const second = commitFrom(base, formatBaselines(countsWith({ proCommands: BASELINES.proCommands - 2 })), "second");
      expect(replay(first, second)).not.toBe(0);
    });

    test("control: with the comment lines dropped, adjacent markers do conflict", () => {
      const withoutComments = (counts: MarkerCounts): string =>
        formatBaselines(counts)
          .split("\n")
          .filter((line) => !line.trimStart().startsWith("//"))
          .join("\n");
      const { base } = setup(withoutComments);
      const lowerCommands = commitFrom(base, withoutComments(countsWith({ proCommands: BASELINES.proCommands - 1 })), "lower proCommands");
      const lowerSync = commitFrom(base, withoutComments(countsWith({ proSyncModule: BASELINES.proSyncModule - 1 })), "lower proSyncModule");
      expect(replay(lowerCommands, lowerSync)).not.toBe(0);
    });
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

  test("a marker added on a branch is listed under \"In files this branch changed\" in the failure", () => {
    repoWithOriginMain();
    write("packages/a/new.ts", "await invoke('pro_example');\n");
    const { counts, hits } = countMarkers(listScannedFiles(dir), dir, []);
    const zero = Object.fromEntries(ALL_MARKERS.map((name) => [name, 0])) as MarkerCounts;
    const message = baselineFailures(counts, zero, changedFilesSinceMain(dir), hits)[0];
    expect(message).toContain("proCommands");
    expect(message).toContain("In files this branch changed:\npackages/a/new.ts:1: await invoke('pro_example');");
  });
});

// ---------------------------------------------------------------------------
// baselineFailures
// ---------------------------------------------------------------------------

describe("baselineFailures", () => {
  test("is empty when every count equals its baseline", () => {
    expect(baselineFailures(countsWith())).toEqual([]);
  });

  test("above the baseline: names the marker, the counts, the absence convention and the raise classes", () => {
    const failures = baselineFailures(countsWith({ proCommands: BASELINES.proCommands + 1 }));
    const message = failures[0];
    expect(message).toContain("proCommands");
    expect(message).toContain(`${BASELINES.proCommands + 1}`);
    expect(message).toContain(`baseline of ${BASELINES.proCommands}`);
    expect(message).toContain(MARKERS.proCommands.summary);
    expect(message).toContain("ADR-081");
    expect(message).toContain("nodespace-sync");
    expect(message).toContain("fragments");
    expect(message).toContain("EXEMPTIONS");
    expect(message).toContain("accepted raise classes in CLAUDE.md ('Pro / Sync Boundary')");
    expect(message).toContain("PR description must name them");
    expect(message).toContain(`set \`BASELINES.proCommands\` to ${BASELINES.proCommands + 1}`);
  });

  test("above the baseline: lists hits in changed files first, then every hit", () => {
    const hits = {
      proCommands: ["packages/a/old.ts:1: pro_old()", "packages/a/new.ts:7: pro_new()", "packages/a/other.ts:3: pro_other()"],
    };
    const failures = baselineFailures(countsWith({ proCommands: BASELINES.proCommands + 1 }), BASELINES, ["packages/a/new.ts"], hits);
    const message = failures[0];
    const changedAt = message.indexOf("In files this branch changed:");
    const allAt = message.indexOf("All hits:");
    expect(changedAt).toBeGreaterThan(-1);
    expect(allAt).toBeGreaterThan(changedAt);
    const changedSection = message.slice(changedAt, allAt);
    expect(changedSection).toContain("packages/a/new.ts:7: pro_new()");
    expect(changedSection).not.toContain("old.ts");
    expect(changedSection).not.toContain("other.ts");
    const allSection = message.slice(allAt);
    for (const hit of hits.proCommands) expect(allSection).toContain(hit);
  });

  test("above the baseline: omits the changed-files section when no hit is in a changed file", () => {
    const hits = { proCommands: ["packages/a/old.ts:1: pro_old()"] };
    const message = baselineFailures(countsWith({ proCommands: BASELINES.proCommands + 1 }), BASELINES, ["packages/b/x.ts"], hits)[0];
    expect(message).not.toContain("In files this branch changed:");
    expect(message).toContain("All hits:");
  });

  test("above the baseline: matches proNamedFiles hits, which are bare paths, against changed files", () => {
    const hits = { proNamedFiles: ["packages/a/pro-old.ts", "packages/a/pro-new.ts"] };
    const message = baselineFailures(countsWith({ proNamedFiles: BASELINES.proNamedFiles + 1 }), BASELINES, ["packages/a/pro-new.ts"], hits)[0];
    const changedSection = message.slice(message.indexOf("In files this branch changed:"), message.indexOf("All hits:"));
    expect(changedSection).toContain("packages/a/pro-new.ts");
    expect(changedSection).not.toContain("pro-old.ts");
  });

  test("above a baseline of 0: says the marker is fully removed from core", () => {
    const zero = { ...BASELINES, proEvents: 0 };
    const message = baselineFailures(countsWith({ proEvents: 1 }), zero)[0];
    expect(message).toContain("fully removed from core");
    expect(baselineFailures(countsWith({ proCommands: BASELINES.proCommands + 1 }))[0]).not.toContain("fully removed");
  });

  test("below the baseline never fails, however far below", () => {
    expect(baselineFailures(countsWith({ tenantWording: BASELINES.tenantWording - 3 }))).toEqual([]);
    expect(baselineFailures(countsWith({ tenantWording: 0, proCommands: 0, proNamedFiles: 0 }))).toEqual([]);
  });

  test("appends no paste block: raising a baseline is never suggested as a block to paste", () => {
    const failures = baselineFailures(countsWith({ tenantWording: BASELINES.tenantWording + 3 }));
    expect(failures).toHaveLength(1);
    expect(failures.join("\n")).not.toContain("Paste over BASELINES");
  });

  test("reports one message per marker above its baseline, in MARKERS order, and none for a marker below", () => {
    const counts = countsWith({ proWording: BASELINES.proWording + 2, proCommands: BASELINES.proCommands - 1, proNamedFiles: BASELINES.proNamedFiles + 1 });
    const failures = baselineFailures(counts);
    expect(failures).toHaveLength(2);
    expect(failures[0]).toContain("proWording");
    expect(failures[1]).toContain("proNamedFiles");
    expect(failures.join("\n")).not.toContain("proCommands");
  });

  test("a baseline of 0 is the strict check: any hit fails, none passes", () => {
    const zeros = Object.fromEntries(ALL_MARKERS.map((name) => [name, 0])) as MarkerCounts;
    expect(baselineFailures(zeros, zeros)).toEqual([]);
    for (const name of ALL_MARKERS) {
      const failures = baselineFailures({ ...zeros, [name]: 1 }, zeros);
      expect(failures).toHaveLength(1);
      expect(failures[0]).toContain(name);
      expect(failures[0]).toContain("fully removed from core");
    }
  });

  test("compares against the baselines it is given, not the checked-in ones", () => {
    const custom = { ...BASELINES, proCommands: 5 };
    expect(baselineFailures(countsWith({ proCommands: 5 }), custom)).toEqual([]);
    expect(baselineFailures(countsWith({ proCommands: 6 }), custom)).not.toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// baselineNotices
// ---------------------------------------------------------------------------

describe("baselineNotices", () => {
  test("is empty when every count equals its baseline", () => {
    expect(baselineNotices(countsWith())).toEqual([]);
  });

  test("is empty when counts are above their baselines: raising is a failure's business, not a notice", () => {
    expect(baselineNotices(countsWith({ proCommands: BASELINES.proCommands + 5 }))).toEqual([]);
  });

  test("names the marker and both numbers, and says it is optional", () => {
    const notices = baselineNotices(countsWith({ tenantWording: BASELINES.tenantWording - 3 }));
    expect(notices[0]).toBe(
      `\`tenantWording\` is now ${BASELINES.tenantWording - 3}, below its baseline ${BASELINES.tenantWording}. Optional, not a failure: lower \`BASELINES.tenantWording\` to ${BASELINES.tenantWording - 3} in scripts/check-pro-boundary.ts.`,
    );
  });

  test("ends with the block to paste, which equals formatBaselines of the tightened counts", () => {
    const counts = countsWith({ tenantWording: BASELINES.tenantWording - 3 });
    const notices = baselineNotices(counts);
    expect(notices).toHaveLength(2);
    expect(notices[1]).toBe(`To tighten every lowered baseline at once, paste over BASELINES in scripts/check-pro-boundary.ts:\n${formatBaselines(counts)}`);
  });

  test("one notice per lowered marker, in MARKERS order, then one block", () => {
    const counts = countsWith({ proWording: BASELINES.proWording - 2, proCommands: BASELINES.proCommands - 1, proNamedFiles: BASELINES.proNamedFiles - 1 });
    const notices = baselineNotices(counts);
    expect(notices).toHaveLength(4);
    expect(notices[0]).toContain("`proCommands` is now");
    expect(notices[1]).toContain("`proWording` is now");
    expect(notices[2]).toContain("`proNamedFiles` is now");
    expect(notices[3]).toContain("paste over BASELINES");
  });

  test("the block never raises a baseline: a marker above its baseline keeps it while a lowered one is tightened", () => {
    const counts = countsWith({ proWording: BASELINES.proWording + 4, tenantWording: BASELINES.tenantWording - 3 });
    const block = baselineNotices(counts).at(-1) ?? "";
    expect(block).toContain(`  tenantWording: ${BASELINES.tenantWording - 3},`);
    expect(block).toContain(`  proWording: ${BASELINES.proWording},`);
    expect(block).not.toContain(`  proWording: ${BASELINES.proWording + 4},`);
  });

  test("pasting the block changes only the value lines that moved", () => {
    const counts = countsWith({ tenantWording: BASELINES.tenantWording - 3, proWording: BASELINES.proWording - 1 });
    const block = (baselineNotices(counts).at(-1) ?? "").split("\n").slice(1); // drop the introduction line
    const before = formatBaselines(BASELINES).split("\n");
    const changed = block.filter((line, i) => line !== before[i]);
    expect(changed).toEqual([`  tenantWording: ${counts.tenantWording},`, `  proWording: ${counts.proWording},`]);
  });

  test("compares against the baselines it is given, not the checked-in ones", () => {
    const custom = { ...BASELINES, proCommands: 5 };
    expect(baselineNotices(countsWith({ proCommands: 5 }), custom)).toEqual([]);
    expect(baselineNotices(countsWith({ proCommands: 4 }), custom)).not.toEqual([]);
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

  test("with no arguments prints all 12 markers with count and baseline, and exits 0 when no count is above its baseline", () => {
    const { status, stdout, stderr } = run();
    expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
    // The count column is whatever main holds today: it may sit below the
    // baseline while other changes land, so only the baseline is pinned.
    for (const name of ALL_MARKERS) expect(stdout).toMatch(new RegExp(`^${name} +\\d+ +${BASELINES[name]}$`, "m"));
    expect(ALL_MARKERS).toHaveLength(12);
  });

  test("--list <marker> prints one path per Pro-named file", () => {
    const { status, stdout } = run("--list", "proNamedFiles");
    expect(status).toBe(0);
    const { counts } = countMarkers(listScannedFiles());
    expect(stdout.split("\n").filter((line) => line !== "")).toHaveLength(counts.proNamedFiles);
  });

  test("--changed prints a line per marker", () => {
    const { status, stdout } = run("--changed");
    expect(status).toBe(0);
    for (const name of ALL_MARKERS) expect(stdout).toMatch(new RegExp(`^${name}: \\d+$`, "m"));
  });

  // The checker locates its repository from its own path, so a copy placed
  // in a fixture repository scans that repository. The baselines are the real
  // ones, so a small fixture is far below them and the default run reports notices.
  function runInFixture(...args: string[]): { status: number; stdout: string; stderr: string } {
    mkdirSync(join(dir, "scripts"), { recursive: true });
    copyFileSync(CHECKER_PATH, join(dir, "scripts/check-pro-boundary.ts"));
    // The ceiling keeps git from finding a repository above a fixture that has none.
    const env = { ...gitEnv(), GIT_CEILING_DIRECTORIES: dirname(dir) };
    const result = spawnSync("bun", ["run", join(dir, "scripts/check-pro-boundary.ts"), ...args], { encoding: "utf8", env });
    return { status: result.status ?? -1, stdout: result.stdout, stderr: result.stderr };
  }

  // The one exempted file, so a fixture's exemption is live and only the counts are in question.
  function writeAgentFixture(): void {
    write("packages/agent/tests/it/live_embedding_prefix_measurement.rs", "a replacement tenant\n");
  }

  test("exits 0 below the baselines, printing a notice and the tightened block, and nothing on stderr", () => {
    repoWithOriginMain();
    writeAgentFixture();
    write("packages/a/x.ts", "pro_x();\n");
    const { status, stdout, stderr } = runInFixture();
    expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
    expect(stdout).toMatch(new RegExp(`^proCommands +1 +${BASELINES.proCommands}$`, "m"));
    expect(stdout).toContain(`\`proCommands\` is now 1, below its baseline ${BASELINES.proCommands}. Optional, not a failure`);
    expect(stdout).toContain("paste over BASELINES in scripts/check-pro-boundary.ts:");
    expect(stdout).toContain("  proCommands: 1,");
    expect(stdout).toContain("No count is above its baseline");
    expect(stdout).not.toContain("equals its baseline");
  });

  test("exits 1 on a stale exemption even when every count is below its baseline", () => {
    repoWithOriginMain();
    write("packages/a/x.ts", "pro_x();\n");
    const { status, stderr } = runInFixture();
    expect(status).toBe(1);
    expect(stderr).toContain("EXEMPTIONS entry for packages/agent/tests/it/live_embedding_prefix_measurement.rs");
  });

  test("exits 1 above a baseline, listing the new lines under \"In files this branch changed\"", () => {
    repoWithOriginMain();
    write("packages/a/big.ts", "pro_x();\n".repeat(BASELINES.proCommands + 25));
    const { status, stderr } = runInFixture();
    expect(status).toBe(1);
    expect(stderr).toContain(`above its baseline of ${BASELINES.proCommands}`);
    expect(stderr).toContain("In files this branch changed:\npackages/a/big.ts:1: pro_x();");
  });

  test("--list <marker> prints a fixture's hits grouped by file", () => {
    repoWithOriginMain();
    write("packages/a/x.ts", "clean\npro_x();\npro_y();\n");
    write("packages/a/y.ts", "pro_z();\n");
    const { status, stdout } = runInFixture("--list", "proCommands");
    expect(status).toBe(0);
    expect(stdout).toBe("packages/a/x.ts\n  2: pro_x();\n  3: pro_y();\npackages/a/y.ts\n  1: pro_z();\n");
  });

  test("--list proNamedFiles prints bare paths", () => {
    repoWithOriginMain();
    write("packages/a/pro-plugin.ts", "clean\n");
    const { status, stdout } = runInFixture("--list", "proNamedFiles");
    expect(status).toBe(0);
    expect(stdout).toBe("packages/a/pro-plugin.ts\n");
  });

  test("--changed prints only the hits in files this branch changed", () => {
    repoWithOriginMain();
    write("packages/a/old.ts", "pro_committed();\n");
    expect(git(dir, "add", "packages/a/old.ts").status).toBe(0);
    expect(git(dir, "commit", "--quiet", "-m", "before the fork").status).toBe(0);
    expect(git(dir, "update-ref", "refs/remotes/origin/main", "HEAD").status).toBe(0);
    write("packages/a/new.ts", "pro_added();\nNodeSpace Pro\n");
    const { status, stdout } = runInFixture("--changed");
    expect(status).toBe(0);
    expect(stdout).toContain("proCommands: 1\n  packages/a/new.ts:1: pro_added();\n");
    expect(stdout).toContain("proWording: 1\n  packages/a/new.ts:2: NodeSpace Pro\n");
    expect(stdout).not.toContain("pro_committed");
    expect(stdout).toMatch(/^tenantWording: 0$/m);
  });

  test("an unknown marker or option prints usage and exits 2", () => {
    for (const args of [["--list", "bogus"], ["--list"], ["--nope"], ["--changed", "extra"], ["--list", "proCommands", "extra"]]) {
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

describe("real-repo ratchet", () => {
  test("no count is above its checked-in baseline, and every exemption is live", () => {
    // The enforcement path (bun test scripts/ -> test:scripts -> the merge
    // gate). A bare toBeLessThanOrEqual() would fail with "Expected: <= N,
    // Received: M" and no hint of what to do, so this throws the message the
    // CLI prints. A count below its baseline passes and only logs a notice.
    const { counts, hits } = countMarkers(listScannedFiles());
    const messages = [...exemptionProblems(), ...baselineFailures(counts, BASELINES, changedFilesSinceMain(), hits)];
    if (messages.length > 0) throw new Error(messages.join("\n\n"));
    const notices = baselineNotices(counts);
    if (notices.length > 0) console.log(notices.join("\n\n"));
  });

  test("BASELINES has one entry per marker, in MARKERS order, then proNamedFiles", () => {
    expect(Object.keys(BASELINES)).toEqual(ALL_MARKERS);
  });

  test("scans packages/agent, and leaves out CLAUDE.md and the checker's own files", () => {
    const files = listScannedFiles();
    expect(files.some((file) => file.startsWith("packages/agent/"))).toBe(true);
    expect(files).toContain("README.md");
    expect(files).not.toContain("CLAUDE.md");
    for (const excluded of EXCLUDED_FILES) expect(files).not.toContain(excluded);
  });

  test("gitignored build output never enters the scan", () => {
    const files = listScannedFiles();
    for (const file of files) {
      expect(file).not.toMatch(/(^|\/)(node_modules|target|\.svelte-kit)\//);
      expect(file).not.toContain("src-tauri/resources/skill/");
      expect(file).not.toContain("nodespaced-pro-");
    }
  });
});
