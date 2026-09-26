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
// main can require it, so GitHub refuses a merge whose exact head never
// passed this gate. There is still no hosted CI — the testing happens on this
// machine, and GitHub only checks the receipt.
//
// Run it from the PR's worktree, with the branch checked out and clean.
//
//   bun run merge <PR#>             gate, record, merge
//   bun run merge <PR#> --dry-run   gate only; no push, status or merge

import { $ } from "bun";

export const STATUS_CONTEXT = "nodespace/gate";

/** How many times main may move under us before giving up. */
export const MAX_ATTEMPTS = 3;

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

async function git(...args: string[]): Promise<string> {
  return (await $`git ${args}`.quiet().text()).trim();
}

async function setStatus(repo: string, sha: string, state: "success" | "failure" | "pending", description: string) {
  await $`gh api -X POST repos/${repo}/statuses/${sha} -f state=${state} -f context=${STATUS_CONTEXT} -f description=${description}`.quiet();
}

function fail(message: string): never {
  console.error(`\n✗ ${message}\n`);
  process.exit(1);
}

async function main(): Promise<void> {
  let args: MergeArgs;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (err) {
    fail(err instanceof Error ? err.message : String(err));
  }
  const { pr, dryRun } = args;

  const repo = (await $`gh repo view --json nameWithOwner --jq .nameWithOwner`.quiet().text()).trim();
  const info = JSON.parse(
    await $`gh pr view ${pr} --json headRefName,headRefOid,state,baseRefName`.quiet().text()
  ) as PullRequest;
  if (info.state !== "OPEN") fail(`PR #${pr} is ${info.state.toLowerCase()}, not open.`);
  if (info.baseRefName !== "main") fail(`PR #${pr} targets ${info.baseRefName}; this command merges into main only.`);

  if ((await git("status", "--porcelain")) !== "") {
    fail("The working tree has uncommitted changes. Commit or discard them — the gate must test exactly what merges.");
  }
  await git("fetch", "--quiet", "origin", "main", info.headRefName);
  const head = await git("rev-parse", "HEAD");
  if (head !== info.headRefOid) {
    fail(
      `This checkout's HEAD (${head.slice(0, 8)}) is not PR #${pr}'s head (${info.headRefOid.slice(0, 8)}).\n` +
        "  Run this from the PR's worktree, with its latest commits pushed."
    );
  }

  const host = (await $`hostname -s`.quiet().text()).trim();

  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    const mainSha = await git("rev-parse", "origin/main");
    if ((await git("merge-base", "HEAD", "origin/main")) !== mainSha) {
      console.log(`\n▶ Rebasing onto origin/main (${mainSha.slice(0, 8)})`);
      const rebase = await $`git rebase origin/main`.nothrow();
      if (rebase.exitCode !== 0) {
        await $`git rebase --abort`.quiet().nothrow();
        fail("The rebase onto main conflicts. Resolve it by hand (git rebase origin/main), push, and re-run.");
      }
    }
    const tested = await git("rev-parse", "HEAD");

    console.log(`\n▶ Full pre-merge gate on ${tested.slice(0, 8)} (attempt ${attempt} of ${MAX_ATTEMPTS})`);
    const gate = await $`bun run scripts/test-gate.ts --mode=merge`.nothrow();
    if (gate.exitCode !== 0) {
      if (!dryRun && tested === info.headRefOid) {
        await setStatus(repo, tested, "failure", statusDescription("failure", host)).catch(() => {});
      }
      fail(`The merge gate failed on ${tested.slice(0, 8)}. Fix it, push, and re-run.`);
    }

    // Main moved while the gate ran: what passed is no longer what would
    // land. Rebase and test again rather than merge a stale result.
    await git("fetch", "--quiet", "origin", "main");
    if ((await git("rev-parse", "origin/main")) !== mainSha) {
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
      await $`git push --no-verify --force-with-lease=${info.headRefName}:${info.headRefOid} origin HEAD:${info.headRefName}`;
    }

    await setStatus(repo, tested, "success", statusDescription("success", host));
    console.log(`\n▶ Recorded ${STATUS_CONTEXT}: success on ${tested.slice(0, 8)}`);

    // --match-head-commit: GitHub refuses the merge if the PR's head is no
    // longer the commit that passed. The branch is deleted through the API
    // rather than --delete-branch, which fails inside the worktree that has
    // it checked out.
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
