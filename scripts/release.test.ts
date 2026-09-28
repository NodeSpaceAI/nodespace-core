// Covers how scripts/release.ts reports a failed pre-release `test:perf` run:
// the releaser is told which benchmarks failed, not just that the run did.
// Also covers release-notes generation: the "What's New" section must come
// from real data (GitHub's --generate-notes), never a hand-written summary
// that gets frozen at whatever it said the day it was written.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { buildReleaseCreateArgs, failingBenchmarks, generateReleaseNotes } from "./release";

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
    const notes = generateReleaseNotes("v9.9.9");

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

describe("buildReleaseCreateArgs", () => {
  test("delegates the change list to GitHub's own release-notes generation when no notes are given", () => {
    const args = buildReleaseCreateArgs({ version: "v1.2.3" });

    expect(args).toEqual(
      expect.arrayContaining(["gh", "release", "create", "v1.2.3", "--generate-notes"])
    );
    // The downloads table is still passed as --notes -- gh prepends it to
    // the notes it generates rather than replacing it.
    const notesIndex = args.indexOf("--notes");
    expect(notesIndex).toBeGreaterThan(-1);
    expect(args[notesIndex + 1]).toContain("v1.2.3");
  });

  test("does not override operator-supplied --notes with generated ones", () => {
    const args = buildReleaseCreateArgs({ version: "v1.2.3", notes: "Custom release notes" });

    expect(args).not.toContain("--generate-notes");
    expect(args).toEqual(expect.arrayContaining(["--notes", "Custom release notes"]));
  });

  test("passes through --draft and --prerelease", () => {
    const args = buildReleaseCreateArgs({ version: "v1.2.3", draft: true, prerelease: true });

    expect(args).toContain("--draft");
    expect(args).toContain("--prerelease");
  });

  test("normalizes a version without a leading v", () => {
    const args = buildReleaseCreateArgs({ version: "1.2.3" });

    expect(args).toContain("v1.2.3");
  });
});
