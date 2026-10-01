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

export interface FakeRemote {
  /** Git environment that redirects `https://x-access-token:<token>@github.com/`
   * to this remote. Set it on `process.env` for in-process calls, or pass it
   * to a child process. */
  env: Record<string, string>;
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
 * whose main branch holds `seed` (path to content) in one commit. */
export async function createFakeRemote(
  repo: string,
  token: string,
  seed: Record<string, string>,
): Promise<FakeRemote> {
  const dir = mkdtempSync(join(tmpdir(), "fake-external-repo-"));
  const env = {
    GIT_CONFIG_COUNT: "1",
    GIT_CONFIG_KEY_0: `url.${pathToFileURL(dir).href}/.insteadOf`,
    GIT_CONFIG_VALUE_0: `https://x-access-token:${token}@github.com/`,
    GIT_ALLOW_PROTOCOL: "file",
    GIT_CONFIG_GLOBAL: devNull,
    GIT_CONFIG_NOSYSTEM: "1",
  };
  const origin = join(dir, `${repo}.git`);
  const work = join(dir, "seed");
  const git = (args: string[], cwd = origin) => $`git ${args}`.cwd(cwd).env({ ...process.env, ...env }).quiet();

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
    env,
    tree: async () => (await git(["ls-tree", "-r", "--name-only", "main"]).text()).split("\n").filter(Boolean).sort(),
    show: async (path) => await git(["show", `main:${path}`]).text(),
    head: async () => (await git(["rev-parse", "main"]).text()).trim(),
    lastChange: async () =>
      (await git(["diff", "--name-status", "main~1", "main"]).text())
        .split("\n")
        .filter(Boolean)
        .map((line) => line.replace("\t", " "))
        .sort(),
    cleanup: () => rmSync(dir, { recursive: true, force: true }),
  };
}
