// Covers the pure parts of scripts/merge-pr.ts. The orchestration itself
// (rebase, gate, status, merge) drives git and GitHub and is exercised by
// running `bun run merge <PR#> --dry-run`.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { isPrBranch, parseArgs, statusDescription } from "./merge-pr";

describe("isPrBranch", () => {
  test("matches the PR branch and its EnterWorktree-prefixed local name", () => {
    expect(isPrBranch("issue-12-fix", "issue-12-fix")).toBe(true);
    expect(isPrBranch("worktree-issue-12-fix", "issue-12-fix")).toBe(true);
  });

  test("does not match an unrelated branch that merely ends the same way", () => {
    expect(isPrBranch("other-issue-12-fix", "issue-12-fix")).toBe(false);
    expect(isPrBranch("main", "issue-12-fix")).toBe(false);
  });
});

describe("parseArgs", () => {
  test("reads the PR number", () => {
    expect(parseArgs(["3088"])).toEqual({ pr: 3088, dryRun: false });
  });

  test("reads --dry-run in either position", () => {
    expect(parseArgs(["--dry-run", "12"])).toEqual({ pr: 12, dryRun: true });
    expect(parseArgs(["12", "--dry-run"])).toEqual({ pr: 12, dryRun: true });
  });

  test.each([[[]], [["abc"]], [["0"]], [["-3"]], [["1.5"]], [["1", "2"]]])(
    "rejects %p with the usage line",
    (argv) => {
      expect(() => parseArgs(argv)).toThrow("usage: bun run merge <PR#>");
    }
  );
});

describe("statusDescription", () => {
  test("names the outcome and the machine", () => {
    expect(statusDescription("success", "studio")).toBe("Full pre-merge gate passed on studio");
    expect(statusDescription("failure", "studio")).toBe("Full pre-merge gate failed on studio");
  });

  test("fits GitHub's 140-character limit", () => {
    expect(statusDescription("success", "h".repeat(300)).length).toBe(140);
  });
});
