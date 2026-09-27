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
import { BASELINES, SCAN_ROOTS, countReferences } from "./check-code-references";

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
      ].join("\n") + "\n",
    );
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(5);
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
});

describe("countReferences — file discovery", () => {
  test("only scans the requested extensions (.rs/.ts/.svelte/.js)", () => {
    writeFixture("scripts/a.ts", "core#1\n");
    writeFixture("scripts/a.rs", "core#2\n");
    writeFixture("scripts/a.svelte", "core#3\n");
    writeFixture("scripts/a.js", "core#4\n");
    writeFixture("scripts/a.md", "core#5\n"); // not scanned
    writeFixture("scripts/a.json", "core#6\n"); // not scanned
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(4);
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
    expect(result.issueNumberFiles.some((f) => f.includes("packages/agent"))).toBe(false);
  });

  test("tolerates a root that doesn't exist", () => {
    const result = countReferences(["does-not-exist"], fixtureDir);
    expect(result.issueNumberReferences).toBe(0);
    expect(result.docPathReferences).toBe(0);
  });
});

describe("countReferences — file lists", () => {
  test("issueNumberFiles/docPathFiles list each matching file once, even with multiple hits", () => {
    writeFixture("scripts/a.ts", "core#1\ncore#2\n");
    const result = countReferences(["scripts"], fixtureDir);
    expect(result.issueNumberReferences).toBe(2);
    expect(result.issueNumberFiles.length).toBe(1);
  });
});

describe("real-repo ratchet", () => {
  test("current repo counts do not exceed the checked-in baselines", () => {
    // Exercises the real repo (default roots/repoRoot) against the ratchet:
    // a decrease (someone pays down the backlog) passes silently; only an
    // increase — new drift — fails this test. This is the check's actual
    // enforcement path (wired in via bun test scripts/ -> test:scripts ->
    // test:all -> the pre-push gate) — unlike the `if (import.meta.main)`
    // block below, a bare toBeLessThanOrEqual() here would fail with only
    // "Expected: <= N, Received: N+1" and no indication of what that means
    // or how to fix it, so this throws the same actionable message the CLI
    // entry point prints instead of asserting silently.
    const counts = countReferences();
    if (counts.issueNumberReferences > BASELINES.issueNumberReferences) {
      throw new Error(
        `${counts.issueNumberReferences} issue-number references in code (#NNNN, core#NNNN, Issue #NNNN), ` +
          `up from the ${BASELINES.issueNumberReferences}-reference baseline in scripts/check-code-references.ts. ` +
          "Describe the behavior/constraint directly and cite an ADR instead, per CLAUDE.md.",
      );
    }
    if (counts.docPathReferences > BASELINES.docPathReferences) {
      throw new Error(
        `${counts.docPathReferences} nodespace-docs/ path references in code, up from the ` +
          `${BASELINES.docPathReferences}-reference baseline in scripts/check-code-references.ts. ` +
          "Inline the essential fact, or cite an ADR, instead of a path into a separate repo.",
      );
    }
  });

  test("scan roots include packages/agent and packages/nlp-engine", () => {
    expect(SCAN_ROOTS).toContain("packages/agent");
    expect(SCAN_ROOTS).toContain("packages/nlp-engine");
  });
});
