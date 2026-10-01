#!/usr/bin/env bun
/**
 * Shared "clone, write, commit, push" mechanics for pushing an automated
 * update to a generated-only public repo this org owns.
 *
 * Used by update-homebrew-cask.ts (NodeSpaceAI/homebrew-nodespace,
 * Casks/nodespace.rb), update-homebrew-formula.ts (same repo,
 * Formula/nodespace-cli.rb), and publish-skill-repo.ts
 * (NodeSpaceAI/nodespace-skill, skills/nodespace/) -- pulled out here so the
 * credential-scrubbing safety net (covering every step, not just the clone --
 * see pushFilesToRepo's inner try/catch) can't drift out of sync between
 * three copies of the same ~35 lines. Each caller still owns its own
 * rendering, digest resolution, and drift-check logic; this file is only the
 * network/git plumbing they share.
 *
 * Originally homebrew-tap-push.ts, hardcoded to the tap repo alone -- now
 * generalized (`repo` is a parameter, not a module constant) now that a
 * second, unrelated target repo needs the identical mechanics.
 */

import { $ } from "bun";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

export interface RepoFile {
  /** Path relative to the target repo root, e.g. "Casks/nodespace.rb" or
   * "skills/nodespace/SKILL.md". */
  relPath: string;
  content: string;
}

/** Clones `repo` (e.g. "NodeSpaceAI/homebrew-nodespace") fresh, writes each
 * of `files`, and pushes a single commit to main if anything actually
 * changed. Returns whether a push happened, so callers can print an
 * accurate "already in sync" vs "pushed" message without a second network
 * round trip to check.
 *
 * `managedDir` (relative to the repo root, e.g. "skills/nodespace") names a
 * directory the caller owns outright: every tracked file under it is removed
 * before `files` are written, so one that `files` no longer lists is deleted
 * from the repo instead of staying published. A file written back with the
 * same content drops out of the staged change, so only real edits and
 * removals are committed. Nothing outside `managedDir` is removed, and
 * without it nothing is removed at all. */
export async function pushFilesToRepo(
  repo: string,
  files: RepoFile[],
  commitMessage: string,
  token: string,
  managedDir?: string,
): Promise<boolean> {
  const workDir = mkdtempSync(join(tmpdir(), "nodespace-external-repo-push-"));
  try {
    // Defense in depth, covering every git/fs step below (not just the
    // clone): git itself redacts credentials from its own stderr on an
    // auth failure (verified against a real bad-token clone), and none of
    // the commands after the clone reference `authUrl` directly (they act
    // on the already-cloned `workDir`, resolving "origin" from its
    // .git/config) -- but scrub `token` out of ANY error that escapes this
    // block regardless, in case a wrapper error ever echoes more than
    // expected. This must never surface the raw token.
    try {
      const authUrl = `https://x-access-token:${token}@github.com/${repo}.git`;
      await $`git clone --depth 1 ${authUrl} ${workDir}`.quiet();

      if (managedDir !== undefined) {
        // --ignore-unmatch: the first publish into a repo that has no
        // `managedDir` yet removes nothing rather than failing.
        await $`git -C ${workDir} rm -r --quiet --ignore-unmatch -- ${managedDir}`.quiet();
      }

      for (const file of files) {
        const dest = join(workDir, file.relPath);
        mkdirSync(dirname(dest), { recursive: true });
        writeFileSync(dest, file.content);
        await $`git -C ${workDir} add ${file.relPath}`.quiet();
      }

      const staged = await $`git -C ${workDir} diff --cached --quiet`.quiet().nothrow();
      if (staged.exitCode === 0) {
        return false;
      }

      await $`git -C ${workDir} -c user.name="nodespace-release-bot" -c user.email="release-bot@nodespace.app" commit -m ${commitMessage}`.quiet();
      await $`git -C ${workDir} push origin HEAD:main`.quiet();
      return true;
    } catch (err) {
      const message = (err instanceof Error ? err.message : String(err)).replaceAll(
        token,
        "***",
      );
      throw new Error(`push to ${repo} failed: ${message}`);
    }
  } finally {
    // Also closes the residual window where a temp credential URL sits in
    // workDir/.git/config in cleartext -- removed as soon as this function
    // returns or throws, on every path.
    rmSync(workDir, { recursive: true, force: true });
  }
}
