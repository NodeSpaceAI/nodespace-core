#!/usr/bin/env bun
// `bun run merge <PR#>` — runs the full pre-merge gate on a PR rebased onto
// current main, records the result as a commit status, then squash-merges.
//
// Why merge time and not push time (ADR-047): most pushes are WIP or
// review-fix pushes, and running the full pyramid on each paid for it several
// times per PR. It also tested the wrong thing — the branch on its own base,
// not what lands. Two branches can each pass alone and still break main
// together. So a push runs a scoped check, and the full pyramid runs once,
// here, on the rebased result.
//
// The `nodespace/gate` commit status is the receipt: branch protection on
// main requires it, so GitHub refuses a merge whose exact head never passed
// this gate. There is still no hosted CI — the testing happens on this
// machine, and GitHub only checks the receipt.
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
//   bun run merge <PR#>             gate, record, merge
//   bun run merge <PR#> --dry-run   gate only; no push, status or merge

import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { $ } from "bun";
import { acquireGateLock, HELD_BY_MERGE_ENV_VAR, registerLockRelease } from "./gate-lock";

export const STATUS_CONTEXT = "nodespace/gate";

/** How many times main may move under us before giving up. */
export const MAX_ATTEMPTS = 3;

/** The persistent gate checkout, relative to the main repository root. */
export const GATE_CHECKOUT = join(".claude", "worktrees", "_gate");

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

/** The description GitHub shows beside the status; its limit is 140 chars. */
export function statusDescription(state: "success" | "failure", host: string): string {
  const text = state === "success" ? `Full pre-merge gate passed on ${host}` : `Full pre-merge gate failed on ${host}`;
  return text.slice(0, 140);
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

async function setStatus(repo: string, sha: string, state: "success" | "failure", description: string) {
  await $`gh api -X POST repos/${repo}/statuses/${sha} -f state=${state} -f context=${STATUS_CONTEXT} -f description=${description}`.quiet();
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
  const localHead = await git(here, "rev-parse", "HEAD");
  const localBranch = await git(here, "rev-parse", "--abbrev-ref", "HEAD");
  if (localBranch.endsWith(info.headRefName) && localHead !== info.headRefOid) {
    fail(
      `This checkout's HEAD (${localHead.slice(0, 8)}) differs from PR #${pr}'s pushed head (${info.headRefOid.slice(0, 8)}).\n` +
        "  The gate tests the PR as pushed — push your commits first."
    );
  }

  // Held from here until this process exits: through the rebase, the gate,
  // and the merge itself.
  const lock = await acquireGateLock();
  registerLockRelease(lock);

  const repoRoot = resolve(dirname(await git(here, "rev-parse", "--path-format=absolute", "--git-common-dir")));
  const gate = await prepareGateCheckout(repoRoot);
  const host = (await $`hostname -s`.quiet().text()).trim();

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
      // A status only means something on a commit GitHub has: the PR head.
      if (!dryRun && tested === info.headRefOid) {
        await setStatus(repo, tested, "failure", statusDescription("failure", host)).catch(() => {});
      }
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

    await setStatus(repo, tested, "success", statusDescription("success", host));
    console.log(`\n▶ Recorded ${STATUS_CONTEXT}: success on ${tested.slice(0, 8)}`);

    // --match-head-commit: GitHub refuses the merge if the PR's head is no
    // longer the commit that passed. The branch is deleted through the API
    // rather than --delete-branch, which also tries to delete the local
    // branch — checked out in the PR's worktree.
    await $`gh pr merge ${pr} --squash --match-head-commit ${tested}`;
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
