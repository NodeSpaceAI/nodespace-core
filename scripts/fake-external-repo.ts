// Test support for the scripts that push through push-to-external-repo.ts: a
// local bare repository standing in for github.com/<repo>, and the git
// environment that sends that repo's authenticated clone URL to it. The code
// under test runs unchanged; only git's URL lookup is redirected.
//
// GIT_ALLOW_PROTOCOL=file makes git refuse https outright, so a test whose
// redirect misses fails instead of reaching GitHub. The user's global and
// system git config are ignored, so their hooks and signing settings do not
// run on these commits.
import { $ } from "bun";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { devNull, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

// Git tells a hook where its repository is (GIT_DIR in a linked worktree's
// pre-push). Neither the stand-in's git nor the code under test may see that,
// or their `git add` and `git rm` would act on this repository instead.
const HOOK_GIT_VARS = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX"];

export interface FakeRemote {
  /** Points this process's git (and any child process started with
   * `process.env`) at the stand-in: sets the redirect and clears a hook's git
   * variables. Returns a function that restores `process.env`. */
  use(): () => void;
  /** Every file path on main, sorted. */
  tree(): Promise<string[]>;
  /** A file's content on main. */
  show(path: string): Promise<string>;
  /** The commit main points at. */
  head(): Promise<string>;
  /** The latest commit's changes on main, as sorted "<status> <path>" entries. */
  lastChange(): Promise<string[]>;
  cleanup(): void;
}

/** Creates a bare repository for `repo` (e.g. "NodeSpaceAI/nodespace-skill")
 * whose main branch holds `seed` (path to content) in one commit. Git reaches
 * it through `https://x-access-token:<token>@github.com/<repo>.git`, the URL
 * pushFilesToRepo clones. */
export async function createFakeRemote(
  repo: string,
  token: string,
  seed: Record<string, string>,
): Promise<FakeRemote> {
  const dir = mkdtempSync(join(tmpdir(), "fake-external-repo-"));
  const redirect: Record<string, string> = {
    GIT_CONFIG_COUNT: "1",
    GIT_CONFIG_KEY_0: `url.${pathToFileURL(dir).href}/.insteadOf`,
    GIT_CONFIG_VALUE_0: `https://x-access-token:${token}@github.com/`,
    GIT_ALLOW_PROTOCOL: "file",
    GIT_CONFIG_GLOBAL: devNull,
    GIT_CONFIG_NOSYSTEM: "1",
  };
  const env: Record<string, string | undefined> = { ...process.env, ...redirect };
  for (const name of HOOK_GIT_VARS) delete env[name];

  const origin = join(dir, `${repo}.git`);
  const work = join(dir, "seed");
  const git = (args: string[], cwd = origin) => $`git ${args}`.cwd(cwd).env(env).quiet();

  try {
    mkdirSync(origin, { recursive: true });
    await git(["init", "--quiet", "--bare", "--initial-branch=main"]);
    mkdirSync(work);
    await git(["init", "--quiet", "--initial-branch=main"], work);
    for (const [path, content] of Object.entries(seed)) {
      mkdirSync(dirname(join(work, path)), { recursive: true });
      writeFileSync(join(work, path), content);
    }
    await git(["add", "--all"], work);
    await git(["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "--quiet", "-m", "seed"], work);
    await git(["push", "--quiet", origin, "HEAD:main"], work);
  } catch (err) {
    rmSync(dir, { recursive: true, force: true });
    throw err;
  }

  return {
    use: () => {
      const names = [...Object.keys(redirect), ...HOOK_GIT_VARS];
      const saved = Object.fromEntries(names.map((name) => [name, process.env[name]]));
      for (const name of HOOK_GIT_VARS) delete process.env[name];
      Object.assign(process.env, redirect);
      return () => {
        for (const [name, value] of Object.entries(saved)) {
          if (value === undefined) delete process.env[name];
          else process.env[name] = value;
        }
      };
    },
    tree: async () => (await git(["ls-tree", "-r", "--name-only", "main"]).text()).split("\n").filter(Boolean).sort(),
    show: async (path) => await git(["show", `main:${path}`]).text(),
    head: async () => (await git(["rev-parse", "main"]).text()).trim(),
    lastChange: async () =>
      (await git(["diff", "--no-renames", "--name-status", "main~1", "main"]).text())
        .split("\n")
        .filter(Boolean)
        .map((line) => line.replace("\t", " "))
        .sort(),
    cleanup: () => rmSync(dir, { recursive: true, force: true }),
  };
}
