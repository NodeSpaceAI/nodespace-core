#!/usr/bin/env bun
// `bun run merge <PR#>` — runs the full pre-merge gate on a PR rebased onto
// current main, then squash-merges exactly the commit that passed.
//
// Why merge time and not push time (ADR-047): most pushes are WIP or
// review-fix pushes, and running the full pyramid on each paid for it several
// times per PR. It also tested the wrong thing — the branch on its own base,
// not what lands. Two branches can each pass alone and still break main
// together. So a push runs a scoped check, and the full pyramid runs once,
// here, on the rebased result.
//
// Two things keep a merge cheap and correct:
//
// - One persistent gate checkout (`.claude/worktrees/_gate`). Every merge on
//   this machine tests there, so its target/ stays warm and each merge
//   recompiles only the crates that changed since the last one — instead of
//   a PR's own worktree, which may never have compiled Rust at all. The PR's
//   worktree is never touched; the gate checkout does the rebase and pushes
//   the result.
// - The gate lock is held from before the rebase until after the merge.
//   Merges therefore land strictly one at a time, and each one rebases onto
//   the main the previous one produced — so none is invalidated by another
//   landing while its gate runs.
//
// It tests the PR as pushed: commit and push first.
//
//   bun run merge <PR#>             gate, then merge
//   bun run merge <PR#> --dry-run   gate only; no push or merge

import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { $ } from "bun";
import { acquireGateLock, DISABLE_ENV_VAR, HELD_BY_MERGE_ENV_VAR, registerLockRelease } from "./gate-lock";

/** How many times main may move under us before giving up. */
export const MAX_ATTEMPTS = 3;

/** Merge attempts, 3s apart, while GitHub catches up with a force-push. */
const MERGE_TRIES = 5;

/** The persistent gate checkout, relative to the main repository root. */
export const GATE_CHECKOUT = join(".claude", "worktrees", "_gate");

/**
 * Ignored, generated paths cleared before every merge gate. target/ and
 * node_modules/ are deliberately absent — they are the warm state the gate
 * checkout exists to keep, and cargo and bun track their own staleness.
 */
export const STALE_OUTPUT_PATHS = [
  "packages/skill/dist",
  "packages/desktop-app/src-tauri/resources/skill",
  "packages/desktop-app/src-tauri/binaries",
];

export interface MergeArgs {
  pr: number;
  dryRun: boolean;
}

export function parseArgs(argv: string[]): MergeArgs {
  const dryRun = argv.includes("--dry-run");
  const positional = argv.filter((a) => !a.startsWith("--"));
  const pr = Number(positional[0]);
  if (positional.length !== 1 || !Number.isInteger(pr) || pr <= 0) {
    throw new Error("usage: bun run merge <PR#> [--dry-run]");
  }
  return { pr, dryRun };
}

/** Whether a local branch is the checkout of the PR's remote branch. */
export function isPrBranch(localBranch: string, prBranch: string): boolean {
  return localBranch === prBranch || localBranch === `worktree-${prBranch}`;
}

interface PullRequest {
  headRefName: string;
  headRefOid: string;
  state: string;
  baseRefName: string;
}

/** Runs git in `cwd` and returns its trimmed stdout. */
async function git(cwd: string, ...args: string[]): Promise<string> {
  return (await $`git ${args}`.cwd(cwd).quiet().text()).trim();
}

function fail(message: string): never {
  console.error(`\n✗ ${message}\n`);
  process.exit(1);
}

/**
 * The gate checkout, created on first use. Detached, so it never holds a
 * branch another worktree might want. Each merge runs `bun install` there
 * after checking out the PR — a no-op when nothing changed, and it picks up
 * a changed lockfile when something did.
 */
async function prepareGateCheckout(repoRoot: string): Promise<string> {
  const path = join(repoRoot, GATE_CHECKOUT);
  if (!existsSync(path)) {
    console.log(`\n▶ Creating the persistent gate checkout at ${path}`);
    await $`git worktree add --detach ${path} origin/main`.cwd(repoRoot).quiet();
  }
  return path;
}

async function main(): Promise<void> {
  let args: MergeArgs;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (err) {
    fail(err instanceof Error ? err.message : String(err));
  }
  const { pr, dryRun } = args;

  const here = process.cwd();
  const repo = (await $`gh repo view --json nameWithOwner --jq .nameWithOwner`.quiet().text()).trim();
  const info = JSON.parse(
    await $`gh pr view ${pr} --json headRefName,headRefOid,state,baseRefName`.quiet().text()
  ) as PullRequest;
  if (info.state !== "OPEN") fail(`PR #${pr} is ${info.state.toLowerCase()}, not open.`);
  if (info.baseRefName !== "main") fail(`PR #${pr} targets ${info.baseRefName}; this command merges into main only.`);

  // Unpushed work in the caller's checkout is not what gets tested. Say so
  // rather than let a green gate imply it covered local commits.
  // EnterWorktree names the local branch `worktree-<name>` for remote <name>.
  const localBranch = await git(here, "rev-parse", "--abbrev-ref", "HEAD");
  if (isPrBranch(localBranch, info.headRefName)) {
    const localHead = await git(here, "rev-parse", "HEAD");
    if (localHead !== info.headRefOid) {
      fail(
        `This checkout's HEAD (${localHead.slice(0, 8)}) differs from PR #${pr}'s pushed head (${info.headRefOid.slice(0, 8)}).\n` +
          "  The gate tests the PR as pushed — push your commits first."
      );
    }
    if ((await git(here, "status", "--porcelain")) !== "") {
      fail("This checkout has uncommitted changes, which the gate would not test. Commit and push them, or discard them.");
    }
  }

  // Held from here until this process exits: through the rebase, the gate,
  // and the merge itself. A push check may run without the lock (it only
  // slows things down); a merge may not. Two merges share one gate checkout,
  // so an unserialized one could check its PR out under another's running
  // tests — and that other merge would record a pass for a tree it never
  // tested. So the lock's usual degrade-and-continue is refused here, as is
  // the no-lock opt-out.
  if (process.env[DISABLE_ENV_VAR]) fail(`${DISABLE_ENV_VAR} is set; a merge always takes the gate lock. Unset it and re-run.`);
  const lock = await acquireGateLock();
  if (!lock.held) fail("Could not take the gate lock (see above), so the merge would not be serialized. Re-run when the other gate finishes.");
  registerLockRelease(lock);

  const repoRoot = resolve(dirname(await git(here, "rev-parse", "--path-format=absolute", "--git-common-dir")));
  const gate = await prepareGateCheckout(repoRoot);

  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    await git(gate, "fetch", "--quiet", "origin", "main", info.headRefName);
    const prHead = await git(gate, "rev-parse", `origin/${info.headRefName}`);
    if (prHead !== info.headRefOid) {
      fail(`PR #${pr}'s head moved to ${prHead.slice(0, 8)} while this ran. Re-run to test the new head.`);
    }
    const mainSha = await git(gate, "rev-parse", "origin/main");

    // A clean slate every time: detached at the PR head, no leftovers from
    // the previous merge. Not `clean -x`: target/ and node_modules/ are the
    // warm state this checkout exists to keep.
    await git(gate, "checkout", "--quiet", "--force", "--detach", prHead);
    await git(gate, "clean", "-fdq");
    // Ignored build output the gate itself produces or reads, which a
    // previous merge may have left: a file a PR deleted could survive there
    // and mask a failure. Removed so this merge rebuilds it from its own tree.
    await git(gate, "clean", "-fdqX", "--", ...STALE_OUTPUT_PATHS);
    if ((await git(gate, "merge-base", "HEAD", mainSha)) !== mainSha) {
      console.log(`\n▶ Rebasing PR #${pr} onto origin/main (${mainSha.slice(0, 8)})`);
      const rebase = await $`git rebase ${mainSha}`.cwd(gate).nothrow();
      if (rebase.exitCode !== 0) {
        await $`git rebase --abort`.cwd(gate).quiet().nothrow();
        fail("The rebase onto main conflicts. Resolve it in your worktree (git rebase origin/main), push, and re-run.");
      }
    }
    const tested = await git(gate, "rev-parse", "HEAD");

    await $`bun install`.cwd(gate).quiet();
    console.log(`\n▶ Full pre-merge gate on ${tested.slice(0, 8)} in ${gate} (attempt ${attempt} of ${MAX_ATTEMPTS})`);
    const result = await $`bun run scripts/test-gate.ts --mode=merge`
      .cwd(gate)
      .env({ ...process.env, [HELD_BY_MERGE_ENV_VAR]: "1" })
      .nothrow();
    if (result.exitCode !== 0) {
      fail(
        `The merge gate failed on ${tested.slice(0, 8)}` +
          (tested === info.headRefOid
            ? ". Fix it, push, and re-run."
            : " — the PR rebased onto main. Reproduce with `git rebase origin/main` in your worktree, fix, push, and re-run.")
      );
    }

    // Another machine can still merge while this gate ran (the lock is
    // per-machine): what passed would no longer be what lands.
    await git(gate, "fetch", "--quiet", "origin", "main");
    if ((await git(gate, "rev-parse", "origin/main")) !== mainSha) {
      console.log("\n⟳ main moved while the gate ran — rebasing and testing again.");
      continue;
    }

    if (dryRun) {
      console.log(`\n✓ Dry run: the merge gate passed on ${tested.slice(0, 8)}. Nothing pushed, recorded or merged.\n`);
      return;
    }

    if (tested !== info.headRefOid) {
      // The gate just ran the full pyramid on exactly this commit, so the
      // pre-push hook's scoped check would only repeat a subset of it.
      console.log(`\n▶ Pushing the rebased branch (${tested.slice(0, 8)})`);
      await $`git push --quiet --no-verify --force-with-lease=${info.headRefName}:${info.headRefOid} origin HEAD:${info.headRefName}`.cwd(gate);
    }

    // --match-head-commit: GitHub refuses the merge if the PR's head is no
    // longer the commit that passed. The branch is deleted through the API
    // rather than --delete-branch, which also tries to delete the local
    // branch — checked out in the PR's worktree.
    // Right after a force-push GitHub can briefly still report the old head,
    // and --match-head-commit then refuses. Retry for a few seconds rather
    // than make the caller re-run a gate that already passed.
    for (let tries = 1; ; tries++) {
      const merged = await $`gh pr merge ${pr} --squash --match-head-commit ${tested}`.nothrow();
      if (merged.exitCode === 0) break;
      if (tries === MERGE_TRIES) {
        fail(
          `GitHub refused the merge of ${tested.slice(0, 8)} (see above), though the gate passed on it;\n` +
            `  once GitHub shows that commit as the PR head, merge with: gh pr merge ${pr} --squash --match-head-commit ${tested}`
        );
      }
      await Bun.sleep(3000);
    }
    await $`gh api -X DELETE repos/${repo}/git/refs/heads/${info.headRefName}`.quiet().nothrow();
    console.log(
      `\n✓ PR #${pr} merged.\n` +
        "  Next: ExitWorktree({action: \"remove\", discard_changes: true}), then from the primary checkout\n" +
        "  `git pull origin main` and `bun run gh:status <issue#> \"Done\"`.\n"
    );
    return;
  }
  fail(`main kept moving during ${MAX_ATTEMPTS} gate runs. Re-run when it settles.`);
}

if (import.meta.main) {
  await main();
}
