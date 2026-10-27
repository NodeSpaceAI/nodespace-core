// Covers how scripts/release.ts reports a failed pre-release `test:perf` run:
// the releaser is told which benchmarks failed, not just that the run did.
// Also covers release-notes generation: the "What's New" section must come
// from real data (GitHub's --generate-notes), never a hand-written summary
// that gets frozen at whatever it said the day it was written.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import {
  buildReleaseCreateArgs,
  failingBenchmarks,
  generateReleaseNotes,
  prepareVersionBump
} from "./release";

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
    expect(notes).not.toContain("Extension API");
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

  test("keeps operator-supplied --notes as given", () => {
    const args = buildReleaseCreateArgs({ version: "v1.2.3", notes: "Custom release notes\n" });

    expect(args).not.toContain("--generate-notes");
    expect(args[args.indexOf("--notes") + 1]).toBe("Custom release notes\n");
  });

  test("passes through --draft and --prerelease", () => {
    const args = buildReleaseCreateArgs({
      version: "v1.2.3",
      draft: true,
      prerelease: true
    });

    expect(args).toContain("--draft");
    expect(args).toContain("--prerelease");
  });

  test("normalizes a version without a leading v", () => {
    const args = buildReleaseCreateArgs({ version: "1.2.3" });

    expect(args).toContain("v1.2.3");
  });
});

describe("buildReleaseCreateArgs target", () => {
  test("pins the tag to the bump commit when a target is given", () => {
    const args = buildReleaseCreateArgs({ version: "v1.2.3", target: "abc123" });
    expect(args.slice(args.indexOf("--target"), args.indexOf("--target") + 2)).toEqual([
      "--target",
      "abc123"
    ]);
  });

  test("omits --target when none is given", () => {
    expect(buildReleaseCreateArgs({ version: "v1.2.3" })).not.toContain("--target");
  });
});

describe("prepareVersionBump", () => {
  const SHA = "a".repeat(40);

  /** Fake runner: records commands; `fail` is a command prefix that exits non-zero. */
  function fake(opts: { dirty?: boolean; fail?: string; head?: string; remote?: string } = {}) {
    const calls: string[] = [];
    const run = (cmd: string[]) => {
      const line = cmd.join(" ");
      calls.push(line);
      const ok = (stdout = "") => ({ exitCode: 0, stdout, stderr: "" });
      if (opts.fail && line.startsWith(opts.fail)) {
        return { exitCode: 1, stdout: "", stderr: "boom" };
      }
      if (line === "git status --porcelain") return ok(opts.dirty === false ? "" : " M Cargo.toml\n");
      if (line === "git diff --cached --quiet") return { exitCode: 1, stdout: "", stderr: "" };
      if (line === "git rev-parse HEAD") return ok((opts.head ?? SHA) + "\n");
      if (line === "git rev-parse FETCH_HEAD") return ok((opts.remote ?? SHA) + "\n");
      return ok();
    };
    return { calls, run };
  }

  test("commits, pushes, verifies and returns the pushed sha", () => {
    const { calls, run } = fake();
    expect(prepareVersionBump("1.2.3", run)).toBe(SHA);
    const commit = calls.findIndex((c) => c.startsWith("git commit"));
    const push = calls.indexOf("git push");
    expect(commit).toBeGreaterThan(-1);
    expect(push).toBeGreaterThan(commit);
    expect(calls.indexOf("git fetch origin main")).toBeGreaterThan(push);
  });

  for (const step of ["cargo check", "bun install", "git add", "git commit", "git push", "git fetch"]) {
    test(`aborts with a clear message when ${step} fails`, () => {
      const { calls, run } = fake({ fail: step });
      expect(() => prepareVersionBump("1.2.3", run)).toThrow(/No release or tag was created/);
      expect(() => prepareVersionBump("1.2.3", run)).toThrow(new RegExp(step));
      expect(calls.some((c) => c.startsWith("gh "))).toBe(false);
    });
  }

  test("aborts when HEAD is not origin/main after a push that reported success", () => {
    const { run } = fake({ remote: "b".repeat(40) });
    expect(() => prepareVersionBump("1.2.3", run)).toThrow(/not origin\/main/);
  });

  test("accepts a bump that is already committed and on origin/main", () => {
    const { calls, run } = fake({ dirty: false });
    expect(prepareVersionBump("1.2.3", run)).toBe(SHA);
    expect(calls.some((c) => c.startsWith("git commit") || c === "git push")).toBe(false);
  });

  test("rejects an already-committed bump that is not on origin/main", () => {
    const { run } = fake({ dirty: false, head: "c".repeat(40) });
    expect(() => prepareVersionBump("1.2.3", run)).toThrow(/not origin\/main/);
  });
});
