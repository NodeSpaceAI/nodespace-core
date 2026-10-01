// Covers the issue-number/doc-path drift-prevention check (CLAUDE.md's rule
// against citing GitHub issue numbers or nodespace-docs/ paths in code —
// "describe the behavior/constraint directly, and reference decisions by
// ADR"). Most tests build an isolated fixture directory so they exercise the
// scanner's actual pattern matching without depending on this repo's real
// (and naturally drifting) reference count; one integration test checks the
// real repo against the ratchet baseline.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SCAN_ROOTS, baselineFailures, countReferences } from "./check-code-references";

let fixtureDir: string;

beforeEach(() => {
  fixtureDir = mkdtempSync(join(tmpdir(), "check-code-references-test-"));
});

afterEach(() => {
  rmSync(fixtureDir, { recursive: true, force: true });
});

function writeFixture(relativePath: string, content: string): void {
  const full = join(fixtureDir, relativePath);
  mkdirSync(join(full, ".."), { recursive: true });
  writeFileSync(full, content);
}

describe("countReferences — issue-number patterns", () => {
  test("matches core#NNNN", () => {
    writeFixture("scripts/a.ts", "// see core#1234 for context\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
  });

  test("matches a trailing (#NNNN)", () => {
    writeFixture("scripts/a.ts", "// Fixed the bug (#5678)\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
  });

  test("matches Issue #NNNN case-insensitively", () => {
    writeFixture("scripts/a.ts", "// per issue #99, this must hold\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
  });

  test("does not match plain prose with a hash but no digits", () => {
    writeFixture("scripts/a.ts", "// use the #hashtag pattern here\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(0);
  });

  test("matches PR#NNNN", () => {
    writeFixture("scripts/a.ts", "// caught by review of PR#2290\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
  });

  test("matches pre-#NNNN and post-#NNNN", () => {
    writeFixture("scripts/a.ts", "// pre-#2132 behavior, changed post-#2088\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
  });

  test("matches pre-issue-NNNN and post-issue-NNNN", () => {
    writeFixture("scripts/a.ts", "// unchanged pre-issue-1689 behavior\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
  });

  test("matches a bare #NNNN in prose", () => {
    writeFixture(
      "scripts/a.ts",
      [
        "// Metal embeddings require Sonoma+, see #990).",
        "// the defect the #2242 audit found",
        "//! load-bearing on this stack: #1931's guidance",
        "// #2182: a repaired call must not read as clean",
        "// hydration (event-driven, #1564/#1566)",
        "it('#2088: does not discard a queued write', () => {});",
        'expect(x, "should not be in properties after #1351");',
      ].join("\n") + "\n",
    );
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(7);
  });

  test("does not match #N shapes that are data, not issue references", () => {
    writeFixture(
      "scripts/a.ts",
      [
        "// promoted into execution (call #2 above)",
        "// same UAX #9 implicit mark",
        "const title = 'Invoice #001';",
        "create_invoice(&executor, \"Invoice #1\");",
        "const fg = isDark ? '#e5e5e5' : '#262626';",
        "{ color: '#333' }",
        'const c = "#999";',
        "  color: #888;",
        "/* matches #252523 */",
        "let s = r#\"raw\"#;",
        ".replace(/'/g, '&#39;');",
      ].join("\n") + "\n",
    );
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(0);
  });

  test("counts one match per line, not per file", () => {
    writeFixture("scripts/a.ts", "// core#1\n// core#2\n// core#3\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(3);
  });
});

describe("countReferences — doc-path patterns", () => {
  test("matches a nodespace-docs/ path", () => {
    writeFixture("scripts/a.ts", "// @see ../nodespace-docs/architecture/foo.md\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.docPathReferences).toBe(1);
  });

  test("does not match an unrelated path", () => {
    writeFixture("scripts/a.ts", "// @see ../other-repo/foo.md\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.docPathReferences).toBe(0);
  });

  test("matches the old pre-move docs/<section>/ shape", () => {
    writeFixture("scripts/a.ts", "// Based on docs/architecture/development/process/ documentation\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.docPathReferences).toBe(1);
  });

  test("does not match a generic docs/*.md fixture path", () => {
    writeFixture("scripts/a.ts", '// See [architecture](./docs/architecture.md) for details\n');
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.docPathReferences).toBe(0);
  });
});

describe("countReferences — file discovery", () => {
  test("only scans the requested extensions (.rs/.ts/.svelte/.js/.proto)", () => {
    writeFixture("scripts/a.ts", "core#1\n");
    writeFixture("scripts/a.rs", "core#2\n");
    writeFixture("scripts/a.svelte", "core#3\n");
    writeFixture("scripts/a.js", "core#4\n");
    writeFixture("scripts/a.proto", "core#7\n");
    writeFixture("scripts/a.md", "core#5\n"); // not scanned
    writeFixture("scripts/a.json", "core#6\n"); // not scanned
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(5);
    expect(result.issueNumberHits).toContain("scripts/a.proto:1: core#7");
  });

  test("skips excluded directory names (node_modules, target)", () => {
    writeFixture("scripts/node_modules/dep/a.ts", "core#1\n");
    writeFixture("scripts/target/debug/a.rs", "core#2\n");
    writeFixture("scripts/real.ts", "core#3\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
  });

  test("recurses into nested subdirectories", () => {
    writeFixture("scripts/a/b/c/deep.ts", "core#1\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
  });

  test("only scans the given roots, not the whole fixture tree", () => {
    writeFixture("scripts/a.ts", "core#1\n");
    writeFixture("packages/agent/b.ts", "core#2\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(1);
    expect(result.issueNumberHits.some((h) => h.includes("packages/agent"))).toBe(false);
  });

  test("tolerates a root that doesn't exist", () => {
    const result = countReferences(["does-not-exist"], fixtureDir);
    expect(result.issueNumberReferences).toBe(0);
    expect(result.docPathReferences).toBe(0);
  });
});

describe("countReferences — hit lists", () => {
  test("list each matching line as a repo-relative path:line: text entry", () => {
    writeFixture("scripts/a.ts", "// clean\n// see core#1\n// @see nodespace-docs/x.md\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberHits).toEqual(["scripts/a.ts:2: // see core#1"]);
    expect(result.docPathHits).toEqual(["scripts/a.ts:3: // @see nodespace-docs/x.md"]);
  });
});

describe("baselineFailures", () => {
  test("names every matching line when a count exceeds its baseline", () => {
    const failures = baselineFailures({
      issueNumberReferences: 10_000,
      docPathReferences: 0,
      issueNumberHits: ["scripts/a.ts:2: // see #1234"],
      docPathHits: [],
    });
    expect(failures.length).toBe(1);
    expect(failures[0]).toContain("scripts/a.ts:2: // see #1234");
  });
});

describe("real-repo ratchet", () => {
  test("current repo counts do not exceed the checked-in baselines", () => {
    // Exercises the real repo (default roots/repoRoot) against the ratchet:
    // a decrease (someone pays down the backlog) passes silently; only an
    // increase — new drift — fails this test. This is the check's actual
    // enforcement path (wired in via bun test scripts/ -> test:scripts ->
    // test:all -> the merge gate) — unlike the `if (import.meta.main)`
    // block below, a bare toBeLessThanOrEqual() here would fail with only
    // "Expected: <= N, Received: N+1" and no indication of what that means
    // or how to fix it, so this throws the same actionable message the CLI
    // entry point prints instead of asserting silently.
    const failures = baselineFailures(countReferences());
    if (failures.length > 0) throw new Error(failures.join("\n\n"));
  });

  test("scan roots include packages/agent and packages/nlp-engine", () => {
    expect(SCAN_ROOTS).toContain("packages/agent");
    expect(SCAN_ROOTS).toContain("packages/nlp-engine");
  });
});
