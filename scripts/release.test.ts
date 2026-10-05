// Covers how scripts/release.ts reports a failed pre-release `test:perf` run:
// the releaser is told which benchmarks failed, not just that the run did.
// Also covers release-notes generation: the "What's New" section must come
// from real data (GitHub's --generate-notes), never a hand-written summary
// that gets frozen at whatever it said the day it was written. The "Extension
// API" section is derived from git the same way (ADR-082 section 8).
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { RUST_VERSION_FILE } from "./check-extension-api-version";
import {
  buildReleaseCreateArgs,
  failingBenchmarks,
  generateReleaseNotes,
  readExtensionApiRelease,
  type ExtensionApiRelease
} from "./release";

// The git tests spawn a dozen git processes on a machine the merge gate shares with Rust builds.
setDefaultTimeout(30_000);

const unchangedApi: ExtensionApiRelease = {
  version: { major: 2, minor: 1 },
  previous: { tag: "v1.2.2", version: { major: 2, minor: 1 } },
  changes: []
};

describe("failingBenchmarks", () => {
  test("names each test from vitest's Failed Tests summary", () => {
    const output = [
      "   ✓ Suite > passing benchmark 12ms",
      "   × Suite > bulk ops complete in <100ms 212ms",
      "⎯⎯⎯⎯⎯⎯⎯ Failed Tests 2 ⎯⎯⎯⎯⎯⎯⎯",
      "",
      " FAIL  src/tests/performance/a.test.ts > Suite > bulk ops complete in <100ms",
      "AssertionError: expected 212 to be less than 100",
      " FAIL  src/tests/performance/b.test.ts > Other > render scales linearly",
      " Test Files  2 failed (2)"
    ].join("\n");

    expect(failingBenchmarks(output)).toEqual([
      "src/tests/performance/a.test.ts > Suite > bulk ops complete in <100ms",
      "src/tests/performance/b.test.ts > Other > render scales linearly"
    ]);
  });

  test("returns nothing when the output names no failing test", () => {
    // e.g. the run died before collecting tests; the caller still prints the tail.
    expect(failingBenchmarks("error: script not found\n")).toEqual([]);
  });
});

describe("generateReleaseNotes", () => {
  test("includes the given version and the downloads table, with no frozen changelog text", () => {
    const notes = generateReleaseNotes("v9.9.9", unchangedApi);

    expect(notes).toContain("v9.9.9");
    expect(notes).toContain("NodeSpace_9.9.9_aarch64.dmg");

    // The old hand-written section was frozen at v0.1.4-alpha content and
    // shipped unchanged on every release that didn't pass --notes-file.
    expect(notes).not.toContain("v0.1.4-alpha");
    expect(notes).not.toContain("What's New");
    expect(notes).not.toContain("Table nodes");
    expect(notes).not.toContain("SurrealDB 3.x upgrade");
  });
});

describe("the Extension API section", () => {
  const section = (api: ExtensionApiRelease): string => {
    const notes = generateReleaseNotes("v1.2.3", api);
    const start = notes.indexOf("### Extension API");
    expect(start).toBeGreaterThan(-1);
    return notes.slice(start);
  };

  test("says the version is unchanged since the previous release", () => {
    expect(section(unchangedApi)).toBe(
      "### Extension API\n\n`EXTENSION_API_VERSION` is 2.1, unchanged since v1.2.2.\n"
    );
  });

  test("gives the previous version and lists the commits that changed it", () => {
    const text = section({
      version: { major: 2, minor: 2 },
      previous: { tag: "v1.2.2", version: { major: 2, minor: 1 } },
      changes: ["Add tree item actions", "Add node types"]
    });
    expect(text).toContain("`EXTENSION_API_VERSION` is 2.2 (v1.2.2 had 2.1).");
    expect(text).toContain("Changed by:\n\n- Add tree item actions\n- Add node types\n");
    expect(text).not.toContain("A major change");
  });

  test("calls out a major change", () => {
    const text = section({
      version: { major: 3, minor: 0 },
      previous: { tag: "v1.2.2", version: { major: 2, minor: 1 } },
      changes: ["Rename a slot"]
    });
    expect(text).toContain("A major change: an extension written for 2 needs updating.");
  });

  test("states the version alone when there is no earlier release", () => {
    expect(section({ version: { major: 1, minor: 0 }, previous: null, changes: [] })).toBe(
      "### Extension API\n\n`EXTENSION_API_VERSION` is 1.0.\n"
    );
  });

  test("says a previous release had no version when it declared none", () => {
    const text = section({
      version: { major: 1, minor: 0 },
      previous: { tag: "v0.1.0", version: null },
      changes: []
    });
    expect(text).toContain("`EXTENSION_API_VERSION` is 1.0 (v0.1.0 had none).");
  });
});

describe("readExtensionApiRelease", () => {
  let dir: string;

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "release-extension-api-test-"));
  });

  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  function git(...args: string[]): void {
    const env = { ...process.env };
    for (const name of ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"]) {
      delete env[name];
    }
    const result = spawnSync(
      "git",
      ["-C", dir, "-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", ...args],
      { encoding: "utf8", env }
    );
    if (result.status !== 0) throw new Error(`git ${args.join(" ")}: ${result.stderr}`);
  }

  function commitVersion(major: number, minor: number, message: string, extra = ""): void {
    const file = join(dir, RUST_VERSION_FILE);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, `${extra}pub const EXTENSION_API_VERSION: (u32, u32) = (${major}, ${minor});\n`);
    git("add", "-A");
    git("commit", "--quiet", "-m", message);
  }

  test("compares with the last release tag and lists only the commits that changed the version", () => {
    git("init", "--quiet");
    commitVersion(2, 0, "start");
    git("tag", "v0.1.0");
    commitVersion(2, 1, "Add a hook");
    commitVersion(2, 1, "Reword a comment", "// The version.\n");
    commitVersion(2, 2, "Add a slot", "// The version.\n");

    expect(readExtensionApiRelease("v0.2.0", dir)).toEqual({
      version: { major: 2, minor: 2 },
      previous: { tag: "v0.1.0", version: { major: 2, minor: 0 } },
      changes: ["Add a hook", "Add a slot"]
    });
  });

  test("reports no previous release when there is no tag", () => {
    git("init", "--quiet");
    commitVersion(1, 0, "start");
    expect(readExtensionApiRelease("v0.1.0", dir)).toEqual({
      version: { major: 1, minor: 0 },
      previous: null,
      changes: []
    });
  });

  test("skips the release's own tag when a re-run finds it already on HEAD", () => {
    git("init", "--quiet");
    commitVersion(2, 0, "start");
    git("tag", "v0.1.0");
    commitVersion(2, 1, "Add a hook");
    git("tag", "v0.2.0");

    expect(readExtensionApiRelease("v0.2.0", dir)).toEqual({
      version: { major: 2, minor: 1 },
      previous: { tag: "v0.1.0", version: { major: 2, minor: 0 } },
      changes: ["Add a hook"]
    });
  });

  test("refuses to release without a readable version", () => {
    git("init", "--quiet");
    const file = join(dir, RUST_VERSION_FILE);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, "// no version here\n");
    expect(() => readExtensionApiRelease("v0.1.0", dir)).toThrow(/No EXTENSION_API_VERSION declaration/);
  });
});

describe("buildReleaseCreateArgs", () => {
  test("delegates the change list to GitHub's own release-notes generation when no notes are given", () => {
    const args = buildReleaseCreateArgs({ version: "v1.2.3", extensionApi: unchangedApi });

    expect(args).toEqual(
      expect.arrayContaining(["gh", "release", "create", "v1.2.3", "--generate-notes"])
    );
    // The downloads table is still passed as --notes -- gh prepends it to
    // the notes it generates rather than replacing it.
    const notesIndex = args.indexOf("--notes");
    expect(notesIndex).toBeGreaterThan(-1);
    expect(args[notesIndex + 1]).toContain("v1.2.3");
  });

  test("keeps operator-supplied --notes, appending the Extension API section", () => {
    const args = buildReleaseCreateArgs({
      version: "v1.2.3",
      extensionApi: unchangedApi,
      notes: "Custom release notes\n"
    });

    expect(args).not.toContain("--generate-notes");
    expect(args[args.indexOf("--notes") + 1]).toBe(
      "Custom release notes\n\n### Extension API\n\n`EXTENSION_API_VERSION` is 2.1, unchanged since v1.2.2.\n"
    );
  });

  test("leaves operator-supplied --notes alone when they already have an Extension API heading", () => {
    for (const notes of [
      "Intro\n\n### Extension API\n\nWritten by hand.\n",
      "Intro\n\n## Extension API\n\nWritten by hand.\n"
    ]) {
      const args = buildReleaseCreateArgs({ version: "v1.2.3", extensionApi: unchangedApi, notes });
      expect(args[args.indexOf("--notes") + 1]).toBe(notes);
    }
  });

  test("passes through --draft and --prerelease", () => {
    const args = buildReleaseCreateArgs({
      version: "v1.2.3",
      extensionApi: unchangedApi,
      draft: true,
      prerelease: true
    });

    expect(args).toContain("--draft");
    expect(args).toContain("--prerelease");
  });

  test("normalizes a version without a leading v", () => {
    const args = buildReleaseCreateArgs({ version: "1.2.3", extensionApi: unchangedApi });

    expect(args).toContain("v1.2.3");
  });
});
