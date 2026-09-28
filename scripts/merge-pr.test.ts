// Covers the pure parts of scripts/merge-pr.ts, plus the replay of a PR onto
// main against real temporary repositories — that step's failure modes are
// git's, so only real git exercises them. The rest of the orchestration
// (gate, status, merge) drives GitHub and is exercised by running
// `bun run merge <PR#> --dry-run`.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { $ } from "bun";
import { buildStack, isPrBranch, parseArgs, replayCommits } from "./merge-pr";

describe("replayCommits", () => {
  const repos: string[] = [];
  afterEach(() => {
    for (const dir of repos.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  async function git(cwd: string, ...args: string[]): Promise<string> {
    return (await $`git ${args}`.cwd(cwd).quiet().text()).trim();
  }

  async function commit(cwd: string, file: string, content: string, message: string): Promise<string> {
    writeFileSync(join(cwd, file), content);
    await git(cwd, "add", file);
    await git(cwd, "commit", "--quiet", "-m", message);
    return git(cwd, "rev-parse", "HEAD");
  }

  /**
   * A repo with a base commit, then `main` and `pr` diverging from it. Leaves
   * HEAD detached on main — where the gate starts a replay.
   */
  async function diverged(
    onMain: (cwd: string) => Promise<void>,
    onPr: (cwd: string) => Promise<void>
  ): Promise<{ cwd: string; main: string; prCommits: string[] }> {
    const cwd = mkdtempSync(join(tmpdir(), "merge-pr-replay-"));
    repos.push(cwd);
    await git(cwd, "init", "--quiet", "-b", "main");
    await git(cwd, "config", "user.email", "test@example.com");
    await git(cwd, "config", "user.name", "Test");
    await git(cwd, "config", "commit.gpgsign", "false");
    const base = await commit(cwd, "shared.txt", "one\n", "base");

    await git(cwd, "checkout", "--quiet", "-b", "pr");
    await onPr(cwd);
    const prCommits = (await git(cwd, "rev-list", "--reverse", `${base}..pr`)).split("\n").filter((c) => c);

    await git(cwd, "checkout", "--quiet", "main");
    await onMain(cwd);
    const main = await git(cwd, "rev-parse", "HEAD");
    await git(cwd, "checkout", "--quiet", "--detach", main);
    return { cwd, main, prCommits };
  }

  async function clean(cwd: string): Promise<void> {
    expect(await git(cwd, "status", "--porcelain")).toBe("");
    expect(existsSync(join(cwd, ".git", "sequencer"))).toBe(false);
    expect(existsSync(join(cwd, ".git", "CHERRY_PICK_HEAD"))).toBe(false);
  }

  test("replays onto main, touching only the PR's own files", async () => {
    const { cwd, main, prCommits } = await diverged(
      (c) => commit(c, "main-only.txt", "m\n", "main change").then(() => {}),
      (c) => commit(c, "pr-only.txt", "p\n", "pr change").then(() => {})
    );

    expect(await replayCommits(cwd, prCommits)).toEqual({ kind: "ok" });
    expect(await git(cwd, "rev-parse", "HEAD~1")).toBe(main);
    expect(readFileSync(join(cwd, "pr-only.txt"), "utf8")).toBe("p\n");
    expect(readFileSync(join(cwd, "main-only.txt"), "utf8")).toBe("m\n");
    await clean(cwd);
  });

  // The case `--empty=drop` existed for, on a git that lacks it: main already
  // carries the change under a different patch, so the pick comes out empty.
  test("keeps a commit that becomes empty against main instead of failing", async () => {
    const { cwd, prCommits } = await diverged(
      async (c) => {
        writeFileSync(join(c, "shared.txt"), "one\ntwo\n");
        writeFileSync(join(c, "other.txt"), "x\n");
        await git(c, "add", ".");
        await git(c, "commit", "--quiet", "-m", "main has the change and more");
      },
      (c) => commit(c, "shared.txt", "one\ntwo\n", "pr change").then(() => {})
    );

    expect(await replayCommits(cwd, prCommits)).toEqual({ kind: "ok" });
    expect(readFileSync(join(cwd, "shared.txt"), "utf8")).toBe("one\ntwo\n");
    await clean(cwd);
  });

  test("keeps a commit that was empty to begin with", async () => {
    const { cwd, prCommits } = await diverged(
      (c) => commit(c, "main-only.txt", "m\n", "main change").then(() => {}),
      (c) => git(c, "commit", "--quiet", "--allow-empty", "-m", "empty").then(() => {})
    );

    expect(await replayCommits(cwd, prCommits)).toEqual({ kind: "ok" });
    await clean(cwd);
  });

  test("reports a real conflict with its paths and leaves the checkout clean", async () => {
    const { cwd, main, prCommits } = await diverged(
      (c) => commit(c, "shared.txt", "main's line\n", "main edit").then(() => {}),
      (c) => commit(c, "shared.txt", "pr's line\n", "pr edit").then(() => {})
    );

    expect(await replayCommits(cwd, prCommits)).toEqual({ kind: "conflict", paths: ["shared.txt"] });
    expect(await git(cwd, "rev-parse", "HEAD")).toBe(main);
    await clean(cwd);
  });

  // The bug this guards: a usage error from an unsupported flag was reported
  // as "the rebase onto main conflicts", sending people after a conflict that
  // did not exist.
  test("reports a failure that is not a conflict as an error, with git's message", async () => {
    const { cwd } = await diverged(
      (c) => commit(c, "main-only.txt", "m\n", "main change").then(() => {}),
      (c) => commit(c, "pr-only.txt", "p\n", "pr change").then(() => {})
    );

    const missing = "0123456789abcdef0123456789abcdef01234567";
    const result = await replayCommits(cwd, [missing]);
    expect(result.kind).toBe("error");
    // git's own message, naming the object it could not find — not ours.
    if (result.kind === "error") {
      expect(result.message).toContain("fatal");
      expect(result.message).toContain(missing);
    }
    await clean(cwd);
  });
});

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

describe("buildStack", () => {
  let cwd: string;
  afterEach(() => {
    rmSync(cwd, { recursive: true, force: true });
  });

  async function git(...args: string[]): Promise<string> {
    return (await $`git ${args}`.cwd(cwd).quiet().text()).trim();
  }

  /** A branch off `from` with one commit writing `file`; returns its head. */
  async function branch(name: string, from: string, file: string, content: string): Promise<string> {
    await git("checkout", "--quiet", "-b", name, from);
    writeFileSync(join(cwd, file), content);
    await git("add", file);
    await git("commit", "--quiet", "-m", name);
    return git("rev-parse", "HEAD");
  }

  test("stacks PRs in order, and leaves out one that conflicts with a PR ahead of it or adds nothing", async () => {
    cwd = mkdtempSync(join(tmpdir(), "merge-pr-stack-"));
    await git("init", "--quiet", "-b", "main");
    await git("config", "user.email", "test@example.com");
    await git("config", "user.name", "Test");
    await git("config", "commit.gpgsign", "false");
    writeFileSync(join(cwd, "base.txt"), "base\n");
    await git("add", "base.txt");
    await git("commit", "--quiet", "-m", "base");
    const base = await git("rev-parse", "HEAD");
    const main = await branch("main-change", base, "main.txt", "main\n");

    const pr1 = await branch("pr1", base, "a.txt", "one\n");
    const pr2 = await branch("pr2", base, "a.txt", "two\n"); // conflicts with #1, not with main
    const pr3 = await branch("pr3", base, "b.txt", "three\n");
    const pr4 = await branch("pr4", base, "main.txt", "main\n"); // main already has its patch
    const pr5 = await branch("pr5", base, "b.txt", "three\n"); // #3, queued ahead, already did it

    await git("checkout", "--quiet", "--detach", main);
    const item = (pr: number, head: string) => ({ pr, headRefName: `pr${pr}`, head });
    const { stack, ejected } = await buildStack(cwd, main, [item(1, pr1), item(2, pr2), item(3, pr3), item(4, pr4), item(5, pr5)]);

    expect(stack.map((e) => e.pr)).toEqual([1, 3]);
    expect(ejected.map((e) => e.pr)).toEqual([2, 4, 5]);
    expect(ejected[0].reason).toContain("#1");
    expect(ejected[0].reason).toContain("a.txt");
    expect(ejected[1].reason).toContain("no commits of its own beyond main");
    expect(ejected[2].reason).toContain("no changes beyond main");

    // The checkout is the top of the stack, clean, with both PRs on main.
    expect(await git("rev-parse", "HEAD^{tree}")).toBe(stack[1].tree);
    expect(await git("status", "--porcelain")).toBe("");
    expect(readFileSync(join(cwd, "a.txt"), "utf8")).toBe("one\n");
    expect(readFileSync(join(cwd, "b.txt"), "utf8")).toBe("three\n");
    expect(readFileSync(join(cwd, "main.txt"), "utf8")).toBe("main\n");
    // Each entry's tree is main plus it and everything below it.
    await git("checkout", "--quiet", "--detach", main);
    await git("cherry-pick", ...stack[0].commits);
    expect(await git("rev-parse", "HEAD^{tree}")).toBe(stack[0].tree);
  });
});
