// Covers the extension API's version check (scripts/check-extension-api-version.ts).
// The rule is tested on synthetic states; the git side builds a throwaway
// repository per test, because what is under test there is what git reports a
// branch changed.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import {
  checkExtensionApiVersion,
  isWatched,
  parseRustVersion,
  parseTsVersion,
  RUST_VERSION_FILE,
  TS_VERSION_FILE,
  versionProblems,
  WATCHED_PATHS,
  type ApiVersion,
  type VersionState,
} from "./check-extension-api-version";

// Each git test spawns a dozen git processes; the merge gate runs these on a
// machine it shares with Rust builds, where Bun's 5s default is tight.
setDefaultTimeout(30_000);

const SNAPSHOT = "packages/desktop-app/src/tests/extension-api/extension-api-surface.json";
const RUST_FIXTURE = "packages/desktop-app/app-lib/src/extensions/fixture_tests.rs";
const SKILL_FIXTURE = "scripts/fixtures/skill-extension/references/fixture-guide.md";

const rustSource = (v: ApiVersion): string =>
  `/// The version.\npub const EXTENSION_API_VERSION: (u32, u32) = (${v.major}, ${v.minor});\n`;
const tsSource = (v: ApiVersion): string =>
  `export const EXTENSION_API_VERSION = { major: ${v.major}, minor: ${v.minor} } as const;\n`;

describe("parsing the constants", () => {
  test("reads the Rust declaration", () => {
    expect(parseRustVersion(rustSource({ major: 2, minor: 1 }))).toEqual({ major: 2, minor: 1 });
    expect(parseRustVersion("pub const EXTENSION_API_VERSION:(u32,u32)=( 10 , 0 );")).toEqual({ major: 10, minor: 0 });
  });

  test("reads the TypeScript declaration", () => {
    expect(parseTsVersion(tsSource({ major: 3, minor: 4 }))).toEqual({ major: 3, minor: 4 });
  });

  test("reads numbers of more than one digit", () => {
    expect(parseRustVersion(rustSource({ major: 12, minor: 34 }))).toEqual({ major: 12, minor: 34 });
    expect(parseTsVersion(tsSource({ major: 12, minor: 34 }))).toEqual({ major: 12, minor: 34 });
  });

  test("finds no version in a mention that is not the declaration", () => {
    expect(parseRustVersion("// pub const EXTENSION_API_VERSION: (u32, u32) = (1, 0);")).toBeNull();
    expect(parseRustVersion("assert_eq!(EXTENSION_API_VERSION, (2, 1));")).toBeNull();
    expect(parseTsVersion("expect(EXTENSION_API_VERSION).toEqual({ major: 2, minor: 1 });")).toBeNull();
  });

  // The constants are read from the real files, in each tier that runs this test.
  test("reads both constants in this checkout, and they agree", () => {
    const repo = join(import.meta.dir, "..");
    const rust = parseRustVersion(readFileSync(join(repo, RUST_VERSION_FILE), "utf8"));
    const ts = parseTsVersion(readFileSync(join(repo, TS_VERSION_FILE), "utf8"));
    expect(rust).not.toBeNull();
    expect(rust).toEqual(ts);
  });
});

describe("WATCHED_PATHS", () => {
  test("matches a file entry exactly and a directory entry by prefix", () => {
    expect(isWatched(RUST_FIXTURE)).toBe(true);
    expect(isWatched(`${RUST_FIXTURE}.orig`)).toBe(false);
    expect(isWatched(SKILL_FIXTURE)).toBe(true);
    expect(isWatched("scripts/fixtures/skill-extension-other/SKILL.md")).toBe(false);
  });

  test("watches the surface snapshot, the Rust fixtures and the skill-extension fixture", () => {
    expect(WATCHED_PATHS).toEqual(
      expect.arrayContaining([
        SNAPSHOT,
        RUST_FIXTURE,
        "packages/desktop-app/app-lib/tests/it/extension_hooks_test.rs",
        "packages/daemon/tests/it/extension_points_fixture.rs",
        "scripts/fixtures/skill-extension/",
      ])
    );
  });

  test("every entry names something in this checkout", () => {
    const repo = join(import.meta.dir, "..");
    expect(WATCHED_PATHS.filter((entry) => !existsSync(join(repo, entry)))).toEqual([]);
  });
});

describe("versionProblems", () => {
  const v21 = { major: 2, minor: 1 };
  const state = (overrides: Partial<VersionState>): VersionState => ({
    changedFiles: [],
    base: v21,
    head: { rust: v21, ts: v21 },
    unmatchedWatched: [],
    ...overrides,
  });

  test("passes a change that touches nothing watched", () => {
    expect(versionProblems(state({ changedFiles: ["packages/core/src/lib.rs", "scripts/release.ts"] }))).toEqual([]);
  });

  test("fails a changed Rust fixture without a bump, naming the file and the rule", () => {
    const problems = versionProblems(state({ changedFiles: ["packages/core/src/lib.rs", RUST_FIXTURE] }));
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain(`  ${RUST_FIXTURE}`);
    expect(problems[0]).not.toContain("packages/core/src/lib.rs");
    expect(problems[0]).toContain("still 2.1");
    expect(problems[0]).toContain("ADR-082 section 8");
    expect(problems[0]).toContain("UPDATE_EXTENSION_API_SURFACE=1");
  });

  test("fails a changed surface snapshot without a bump", () => {
    expect(versionProblems(state({ changedFiles: [SNAPSHOT] }))).toHaveLength(1);
  });

  test("fails a changed skill-extension fixture without a bump", () => {
    expect(versionProblems(state({ changedFiles: [SKILL_FIXTURE] }))).toHaveLength(1);
  });

  test("passes a watched change with a minor or a major bump", () => {
    for (const bumped of [{ major: 2, minor: 2 }, { major: 3, minor: 0 }]) {
      expect(versionProblems(state({ changedFiles: [RUST_FIXTURE, SNAPSHOT], head: { rust: bumped, ts: bumped } }))).toEqual([]);
    }
  });

  test("fails a version that went backwards, even with nothing watched changed", () => {
    const older = { major: 2, minor: 0 };
    const problems = versionProblems(state({ head: { rust: older, ts: older } }));
    expect(problems).toEqual(["EXTENSION_API_VERSION went from 2.1 back to 2.0; it only moves forward."]);
  });

  test("does not take a backwards move for a bump", () => {
    const older = { major: 1, minor: 9 };
    const problems = versionProblems(state({ changedFiles: [RUST_FIXTURE], head: { rust: older, ts: older } }));
    expect(problems.some((p) => p.includes("back to 1.9"))).toBe(true);
  });

  test("fails when the Rust and TypeScript constants disagree", () => {
    const problems = versionProblems(state({ changedFiles: [RUST_FIXTURE], head: { rust: { major: 2, minor: 2 }, ts: v21 } }));
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain(`2.2 in ${RUST_VERSION_FILE} but 2.1 in ${TS_VERSION_FILE}`);
  });

  test("fails when a constant can't be read at HEAD", () => {
    expect(versionProblems(state({ head: { rust: null, ts: v21 } }))[0]).toContain(RUST_VERSION_FILE);
    expect(versionProblems(state({ head: { rust: v21, ts: null } }))[0]).toContain(TS_VERSION_FILE);
  });

  test("fails a watched entry that matches no file", () => {
    const problems = versionProblems(state({ unmatchedWatched: [RUST_FIXTURE] }));
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain(`lists ${RUST_FIXTURE}, which matches no file`);
  });

  test("asks for no bump when the merge-base declares no version", () => {
    expect(versionProblems(state({ base: null, changedFiles: [RUST_FIXTURE] }))).toEqual([]);
  });
});

describe("checkExtensionApiVersion on a git repository", () => {
  let dir: string;

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "check-extension-api-version-test-"));
  });

  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  // A hook gets GIT_DIR (a linked worktree's pre-push does); a fixture repo must not see it.
  function git(...args: string[]): string {
    const env = { ...process.env };
    for (const name of ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"]) delete env[name];
    const result = spawnSync(
      "git",
      ["-C", dir, "-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", ...args],
      { encoding: "utf8", env }
    );
    if (result.status !== 0) throw new Error(`git ${args.join(" ")}: ${result.stderr}`);
    return result.stdout;
  }

  function write(path: string, content: string): void {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), content);
  }

  function setVersion(v: ApiVersion): void {
    write(RUST_VERSION_FILE, rustSource(v));
    write(TS_VERSION_FILE, tsSource(v));
  }

  function commit(message: string): void {
    git("add", "-A");
    git("commit", "--quiet", "-m", message);
  }

  /** Every watched file and both constants at 2.1, committed, with origin/main at that commit. */
  function repoOnMain(): void {
    git("init", "--quiet");
    for (const entry of WATCHED_PATHS) write(entry.endsWith("/") ? `${entry}SKILL.md` : entry, "fixture\n");
    write(SKILL_FIXTURE, "guide\n");
    write("packages/core/src/lib.rs", "// core\n");
    setVersion({ major: 2, minor: 1 });
    commit("main");
    git("update-ref", "refs/remotes/origin/main", "HEAD");
  }

  test("passes a branch with no changes", () => {
    repoOnMain();
    expect(checkExtensionApiVersion(dir)).toEqual([]);
  });

  test("fails a committed fixture change without a bump", () => {
    repoOnMain();
    write(RUST_FIXTURE, "fixture, with a new extension point\n");
    commit("use a new extension point");
    const problems = checkExtensionApiVersion(dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain(`  ${RUST_FIXTURE}`);
  });

  test("passes a fixture change that bumps the version in the same commit", () => {
    repoOnMain();
    write(RUST_FIXTURE, "fixture, with a new extension point\n");
    setVersion({ major: 2, minor: 2 });
    commit("use a new extension point");
    expect(checkExtensionApiVersion(dir)).toEqual([]);
  });

  test("passes a bump in a later commit on the branch", () => {
    repoOnMain();
    write(SKILL_FIXTURE, "guide, reworded\n");
    commit("reword the skill fixture");
    setVersion({ major: 2, minor: 2 });
    commit("bump");
    expect(checkExtensionApiVersion(dir)).toEqual([]);
  });

  test("passes an unrelated change", () => {
    repoOnMain();
    write("packages/core/src/lib.rs", "// core, changed\n");
    commit("core change");
    expect(checkExtensionApiVersion(dir)).toEqual([]);
  });

  test("passes a snapshot re-recorded with its bump, alongside fixtures, as an API change lands", () => {
    repoOnMain();
    write(SNAPSHOT, '{"version":{"major":2,"minor":2}}\n');
    write(RUST_FIXTURE, "fixture asserting 2.2\n");
    write("packages/desktop-app/src/tests/fixtures/extension.ts", "export default {};\n");
    setVersion({ major: 2, minor: 2 });
    commit("add contribution kinds");
    expect(checkExtensionApiVersion(dir)).toEqual([]);
  });

  test("judges what is committed: an uncommitted bump is not pushed", () => {
    repoOnMain();
    write(RUST_FIXTURE, "changed\n");
    commit("fixture change");
    setVersion({ major: 2, minor: 2 });
    expect(checkExtensionApiVersion(dir)).toHaveLength(1);
  });

  test("diffs from the merge-base, so main's own versioned changes are not this branch's", () => {
    repoOnMain();
    const fork = git("rev-parse", "HEAD").trim();
    write(RUST_FIXTURE, "main's new extension point\n");
    setVersion({ major: 2, minor: 2 });
    commit("main moves on");
    git("update-ref", "refs/remotes/origin/main", "HEAD");
    git("checkout", "--quiet", "--detach", fork);
    write("packages/core/src/lib.rs", "// branch work\n");
    commit("branch work");
    expect(checkExtensionApiVersion(dir)).toEqual([]);
  });

  test("fails a watched file moved away without updating the list", () => {
    repoOnMain();
    git("mv", RUST_FIXTURE, "packages/desktop-app/app-lib/src/extensions/fixtures.rs");
    setVersion({ major: 2, minor: 2 });
    commit("move the fixture");
    const problems = checkExtensionApiVersion(dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain(`lists ${RUST_FIXTURE}, which matches no file`);
  });

  test("counts a watched file moved away, without a bump, as changed at its old path", () => {
    repoOnMain();
    git("mv", RUST_FIXTURE, "packages/desktop-app/app-lib/src/extensions/fixtures.rs");
    commit("move the fixture");
    const problems = checkExtensionApiVersion(dir);
    expect(problems).toHaveLength(2);
    expect(problems[1]).toContain(`  ${RUST_FIXTURE}`);
  });

  test("fails, rather than passing unchecked, when there is no origin/main", () => {
    repoOnMain();
    git("update-ref", "-d", "refs/remotes/origin/main");
    const problems = checkExtensionApiVersion(dir);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("git fetch origin");
    // git's own reason, which tells a missing ref from a broken one.
    expect(problems[0]).toContain("fatal:");
  });
});
