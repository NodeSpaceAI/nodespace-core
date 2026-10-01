// Covers pushFilesToRepo (scripts/push-to-external-repo.ts) against a local
// bare repository standing in for the GitHub repo (scripts/fake-external-repo.ts
// redirects the authenticated URL to it, and makes git refuse https). Each
// test seeds that repository, runs one push through the real clone, write,
// commit and push path, and reads main back.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, describe, expect, setDefaultTimeout, test } from "bun:test";
import { $ } from "bun";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createFakeRemote, type FakeRemote } from "./fake-external-repo";
import { pushFilesToRepo, type RepoFile } from "./push-to-external-repo";

// Each test clones, commits and pushes. Bun's 5s default per-test timeout is
// tight on the loaded machines these tests run on (the merge gate shares them
// with Rust builds), and a timeout here would eject an unrelated PR.
setDefaultTimeout(30_000);

const REPO = "example/generated-only";
const TOKEN = "test-token";
const MANAGED = "skills/example";

let remote: FakeRemote | undefined;
let restoreEnv: (() => void) | undefined;

/** Creates the fake remote and points this process's git at it. */
async function seed(files: Record<string, string>): Promise<FakeRemote> {
  remote = await createFakeRemote(REPO, TOKEN, files);
  restoreEnv = remote.use();
  return remote;
}

afterEach(() => {
  restoreEnv?.();
  restoreEnv = undefined;
  remote?.cleanup();
  remote = undefined;
});

function push(files: RepoFile[], managedDir?: string): Promise<boolean> {
  return pushFilesToRepo(REPO, files, "test publish", TOKEN, managedDir);
}

describe("pushFilesToRepo with a managed directory", () => {
  test("removes a file under the managed directory that the pushed set no longer lists", async () => {
    const r = await seed({
      [`${MANAGED}/SKILL.md`]: "body\n",
      [`${MANAGED}/references/removed.md`]: "old guidance\n",
    });

    expect(await push([{ relPath: `${MANAGED}/SKILL.md`, content: "body\n" }], MANAGED)).toBe(true);

    expect(await r.tree()).toEqual([`${MANAGED}/SKILL.md`]);
    expect(await r.lastChange()).toEqual([`D ${MANAGED}/references/removed.md`]);
  });

  test("keeps every pushed file, committing only the files whose content changed", async () => {
    const r = await seed({
      [`${MANAGED}/SKILL.md`]: "old body\n",
      [`${MANAGED}/references/kept.md`]: "unchanged\n",
    });

    const files = [
      { relPath: `${MANAGED}/SKILL.md`, content: "new body\n" },
      { relPath: `${MANAGED}/references/kept.md`, content: "unchanged\n" },
      { relPath: `${MANAGED}/references/added.md`, content: "added\n" },
    ];
    expect(await push(files, MANAGED)).toBe(true);

    expect(await r.tree()).toEqual(files.map((f) => f.relPath).sort());
    for (const file of files) {
      expect(await r.show(file.relPath)).toBe(file.content);
    }
    expect(await r.lastChange()).toEqual([
      `A ${MANAGED}/references/added.md`,
      `M ${MANAGED}/SKILL.md`,
    ]);
  });

  test("leaves every file outside the managed directory alone, including a sibling sharing its name as a prefix", async () => {
    const outside = {
      "README.md": "hand-written readme\n",
      ".claude-plugin/marketplace.json": "{}\n",
      "skills/other/SKILL.md": "another skill\n",
      [`${MANAGED}-extra/notes.md`]: "sibling directory\n",
    };
    const r = await seed({
      ...outside,
      [`${MANAGED}/SKILL.md`]: "body\n",
      [`${MANAGED}/references/removed.md`]: "old guidance\n",
    });

    expect(await push([{ relPath: `${MANAGED}/SKILL.md`, content: "body\n" }], MANAGED)).toBe(true);

    expect(await r.tree()).toEqual([...Object.keys(outside), `${MANAGED}/SKILL.md`].sort());
    for (const [path, content] of Object.entries(outside)) {
      expect(await r.show(path)).toBe(content);
    }
    expect(await r.lastChange()).toEqual([`D ${MANAGED}/references/removed.md`]);
  });

  test("publishes into a repository that has no managed directory yet", async () => {
    const r = await seed({ "README.md": "readme\n" });

    expect(await push([{ relPath: `${MANAGED}/SKILL.md`, content: "body\n" }], MANAGED)).toBe(true);

    expect(await r.tree()).toEqual(["README.md", `${MANAGED}/SKILL.md`]);
  });

  test("pushes nothing when the managed directory already matches", async () => {
    const r = await seed({ "README.md": "readme\n", [`${MANAGED}/SKILL.md`]: "body\n" });
    const before = await r.head();

    expect(await push([{ relPath: `${MANAGED}/SKILL.md`, content: "body\n" }], MANAGED)).toBe(false);

    expect(await r.head()).toBe(before);
  });
});

describe("pushFilesToRepo without a managed directory", () => {
  // The Homebrew tap holds the cask and the formula, each pushed by its own
  // script with only its own file: neither may remove the other.
  test("removes nothing, so a push of one file keeps the repository's other files", async () => {
    const r = await seed({
      "Casks/nodespace.rb": "old cask\n",
      "Formula/nodespace-cli.rb": "formula\n",
    });

    expect(await push([{ relPath: "Casks/nodespace.rb", content: "new cask\n" }])).toBe(true);

    expect(await r.tree()).toEqual(["Casks/nodespace.rb", "Formula/nodespace-cli.rb"]);
    expect(await r.show("Formula/nodespace-cli.rb")).toBe("formula\n");
    expect(await r.lastChange()).toEqual(["M Casks/nodespace.rb"]);
  });
});

describe("the stand-in remote", () => {
  // The guard that keeps these tests off the network: a GitHub URL the
  // redirect does not cover (another token) is refused, not fetched.
  test("refuses https, so a push its redirect misses never reaches GitHub", async () => {
    await seed({ "README.md": "readme\n" });

    const result = await $`git ls-remote https://x-access-token:other-token@github.com/${REPO}.git`
      .env({ ...process.env, LC_ALL: "C" })
      .quiet()
      .nothrow();

    expect(result.exitCode).not.toBe(0);
    expect(result.stderr.toString()).toContain("transport 'https' not allowed");
  });
});

describe("under a git hook", () => {
  // A hook exports GIT_DIR for its own repository. If the push inherited it,
  // its `git rm` and `git add` would act on that repository, not the clone.
  test("a GIT_DIR in the environment does not redirect the push's git", async () => {
    // Outside this checkout, so a regression that let it through leaves nothing here.
    const hookDir = mkdtempSync(join(tmpdir(), "push-test-hook-git-dir-"));
    const saved = process.env.GIT_DIR;
    process.env.GIT_DIR = join(hookDir, "not-a-repository");
    try {
      const r = await seed({ [`${MANAGED}/references/removed.md`]: "old guidance\n" });

      expect(await push([{ relPath: `${MANAGED}/SKILL.md`, content: "body\n" }], MANAGED)).toBe(true);

      expect(await r.tree()).toEqual([`${MANAGED}/SKILL.md`]);
    } finally {
      restoreEnv?.();
      restoreEnv = undefined;
      if (saved === undefined) delete process.env.GIT_DIR;
      else process.env.GIT_DIR = saved;
      rmSync(hookDir, { recursive: true, force: true });
    }
  });
});
