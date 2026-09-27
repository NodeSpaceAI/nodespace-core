#!/usr/bin/env bun
// `bun run merge <PR#>` — runs the full pre-merge gate on a PR rebased onto
// current main, then squash-merges exactly the commit that passed.
//
// Why merge time and not push time (ADR-047): most pushes are WIP or
// review-fix pushes, and running the full pyramid on each paid for it several
// times per PR. It also tested the wrong thing — the branch on its own base,
// not what lands. Two branches can each pass alone and still break main
// together. So a push only lints, and the full pyramid runs once, here, on
// the rebased result.
//
// Two things keep a merge cheap and correct:
//
// - One persistent gate checkout (`.claude/worktrees/_gate`). Every merge on
//   this machine tests there, so its target/ stays warm and each merge
//   recompiles only the crates that changed since the last one — instead of
//   a PR's own worktree, which may never have compiled Rust at all. The PR's
//   worktree is never touched; the gate checkout does the rebase and pushes
//   the result.
// - The merge lock is held from before the rebase until after the merge.
//   Merges therefore land strictly one at a time, and each one rebases onto
//   the main the previous one produced — so none is invalidated by another
//   landing while its gate runs. The gate then takes the machine slot
//   (gate-lock.ts) — ahead of any queued test:changed Rust run — so no other
//   heavy run shares the machine with its compiles and tests.
//
// It tests the PR as pushed: commit and push first.
//
//   bun run merge <PR#>             gate, then merge
//   bun run merge <PR#> --dry-run   gate only; no push or merge

import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { $ } from "bun";
import { acquireGateLock, DISABLE_ENV_VAR, MERGE_LOCK_PATH, registerLockRelease } from "./gate-lock";

/** How many times main may move under us before giving up. */
export const MAX_ATTEMPTS = 3;

/** Merge attempts, 3s apart, while GitHub catches up with a push. */
const MERGE_TRIES = 10;

/** How long a merge waits for earlier merges on this machine. */
const MERGE_WAIT_CAP_MS = 2 * 60 * 60 * 1000;

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
  /** Read from origin itself once this merge's turn comes — not from the API. */
  headRefOid: string;
  state: string;
  baseRefName: string;
}

/** How replaying a PR's commits onto main went. */
export type ReplayResult =
  | { kind: "ok" }
  | { kind: "conflict"; paths: string[] }
  | { kind: "error"; message: string };

/**
 * Cherry-pick `commits` onto the checkout in `cwd`, leaving it clean whatever
 * happens.
 *
 * `--keep-redundant-commits` rather than `--empty=drop`: the latter needs Git
 * 2.45, and on older git the whole command is a usage error. A commit that
 * becomes empty against main is kept as an empty commit instead of dropped —
 * harmless, since the merge squashes. A commit already on main by patch never
 * reaches here: the caller's `rev-list --cherry-pick` filters it out.
 *
 * A failure is only a conflict when the pick stopped on unmerged paths.
 * Anything else — an unknown option, a missing commit — is reported with
 * git's own message, so an environment problem never reads as a conflict.
 */
export async function replayCommits(cwd: string, commits: string[]): Promise<ReplayResult> {
  const pick = await $`git cherry-pick --keep-redundant-commits ${commits}`.cwd(cwd).quiet().nothrow();
  if (pick.exitCode === 0) return { kind: "ok" };

  const unmerged = (await $`git diff --name-only --diff-filter=U`.cwd(cwd).quiet().nothrow().text())
    .split("\n")
    .filter((p) => p !== "");
  await $`git cherry-pick --abort`.cwd(cwd).quiet().nothrow();
  // Clear any sequencer state an abort left behind, or every later merge
  // would fail with "cherry-pick already in progress".
  await $`git cherry-pick --quit`.cwd(cwd).quiet().nothrow();
  if (unmerged.length > 0) return { kind: "conflict", paths: unmerged };
  const message = `${pick.stderr.toString()}${pick.stdout.toString()}`.trim();
  return { kind: "error", message: message || `git cherry-pick exited with code ${pick.exitCode}` };
}

/** Runs git in `cwd` and returns its trimmed stdout. */
async function git(cwd: string, ...args: string[]): Promise<string> {
  return (await $`git ${args}`.cwd(cwd).quiet().text()).trim();
}

/**
 * The PR branch's head as the remote has it right now. GitHub's API can lag a
 * fresh push by a few seconds, so reading the head from it — once, at startup
 * — let a push followed at once by `bun run merge` capture the old commit,
 * wait out the whole merge queue, then refuse on finding the new one.
 */
async function remoteHead(cwd: string, branch: string): Promise<string> {
  const ref = `refs/heads/${branch}`;
  let output = "";
  try {
    output = await git(cwd, "ls-remote", "origin", ref);
  } catch (err) {
    fail(`Could not reach origin to read ${branch}'s head: ${err instanceof Error ? err.message : String(err)}`);
  }
  // ls-remote matches refs by suffix; take only the exact branch.
  const sha = output
    .split("\n")
    .map((line) => line.split(/\s+/))
    .find(([, name]) => name === ref)?.[0];
  if (sha === undefined || !/^[0-9a-f]{40}$/.test(sha)) fail(`${branch} was not found on origin.`);
  return sha;
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
    await $`gh pr view ${pr} --json headRefName,state,baseRefName`.quiet().text()
  ) as PullRequest;
  if (info.state !== "OPEN") fail(`PR #${pr} is ${info.state.toLowerCase()}, not open.`);
  if (info.baseRefName !== "main") fail(`PR #${pr} targets ${info.baseRefName}; this command merges into main only.`);

  // Unpushed work in the caller's checkout is not what gets tested. Say so
  // rather than let a green gate imply it covered local commits.
  // EnterWorktree names the local branch `worktree-<name>` for remote <name>.
  const localBranch = await git(here, "rev-parse", "--abbrev-ref", "HEAD");
  if (isPrBranch(localBranch, info.headRefName)) {
    const localHead = await git(here, "rev-parse", "HEAD");
    const pushedHead = await remoteHead(here, info.headRefName);
    if (localHead !== pushedHead) {
      fail(
        `This checkout's HEAD (${localHead.slice(0, 8)}) differs from PR #${pr}'s pushed head (${pushedHead.slice(0, 8)}).\n` +
          "  The gate tests the PR as pushed — push your commits first."
      );
    }
    if ((await git(here, "status", "--porcelain")) !== "") {
      fail("This checkout has uncommitted changes, which the gate would not test. Commit and push them, or discard them.");
    }
  }

  // The merge lock, held from here until this process exits: through the
  // rebase, the gate, and the merge itself. Two merges share one gate
  // checkout, so an unserialized one could check its PR out under another's
  // running tests — and that other merge would then land a tree it never
  // tested. So the lock's usual degrade-and-continue is refused here, as is
  // the no-lock opt-out. Merges wait on each other only, so the cap is long.
  if (process.env[DISABLE_ENV_VAR]) fail(`${DISABLE_ENV_VAR} is set; a merge always takes the merge lock. Unset it and re-run.`);
  const lock = await acquireGateLock({ lockPath: MERGE_LOCK_PATH, what: "merge", maxWaitMs: MERGE_WAIT_CAP_MS });
  if (!lock.held) fail("Could not take the merge lock (see above), so this merge would not be serialized. Re-run when the other merge finishes.");
  registerLockRelease(lock);

  const repoRoot = resolve(dirname(await git(here, "rev-parse", "--path-format=absolute", "--git-common-dir")));
  const gate = await prepareGateCheckout(repoRoot);

  // Read the head now that this merge's turn has come, not at startup: the
  // queue wait can be long, and a push just before it may not have been
  // visible yet. A merge tests the PR as pushed when its turn comes — so a
  // fix pushed while it waited is what gets tested and landed. This is the
  // commit the gate tests and the merge must match; a push during the gate
  // itself is still refused below.
  info.headRefOid = await remoteHead(gate, info.headRefName);

  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
    await git(gate, "fetch", "--quiet", "origin", "main", info.headRefName);
    const prHead = await git(gate, "rev-parse", `origin/${info.headRefName}`);
    if (prHead !== info.headRefOid) {
      fail(`PR #${pr}'s head moved to ${prHead.slice(0, 8)} while this ran. Re-run to test the new head.`);
    }
    const mainSha = await git(gate, "rev-parse", "origin/main");

    // A clean slate every time, starting from current main: no leftovers from
    // the previous merge. Not `clean -x`: target/ and node_modules/ are the
    // warm state this checkout exists to keep.
    await git(gate, "checkout", "--quiet", "--force", "--detach", mainSha);
    await git(gate, "clean", "-fdq");
    // Ignored build output the gate itself produces or reads, which a
    // previous merge may have left: a file a PR deleted could survive there
    // and mask a failure. Removed so this merge rebuilds it from its own tree.
    await git(gate, "clean", "-fdqX", "--", ...STALE_OUTPUT_PATHS);

    // Replay the PR's commits onto main — what a rebase does — without first
    // checking out the PR's own, older base. That detour rewrote every file
    // main had changed since the PR branched, then rewrote it back, and cargo,
    // which judges staleness by modification time, recompiled all of them on
    // a checkout that exists to stay warm. Starting from main touches only the
    // files the PR changes.
    const base = await git(gate, "merge-base", prHead, mainSha);
    if (base === mainSha) {
      // `--force`, matching the `mainSha` checkout above: this checkout can
      // still land on a tracked file whose content differs from what's on
      // disk (observed with `bun.lock`) even right after that reset and a
      // `clean -fdq` — some other tool with a handle on this shared,
      // concurrently-used checkout (a `bun install` from another merge, a
      // build script) can leave an unstaged modification behind between the
      // two commands, and a plain `checkout` refuses to overwrite it rather
      // than silently discarding it. `--force` is the correct choice here,
      // not a bug to route around some other way: this checkout exists
      // solely to be blown away and rebuilt every run (see the comment on
      // the `mainSha` checkout above), so there is never a legitimate local
      // change here worth preserving.
      await git(gate, "checkout", "--quiet", "--force", "--detach", prHead);
    } else {
      console.log(`\n▶ Rebasing PR #${pr} onto origin/main (${mainSha.slice(0, 8)})`);
      // The commit set rebase would replay: linear (merge commits dropped,
      // their changes arriving through the commits around them), in graph
      // order, and skipping any commit whose patch main already has.
      const commits = (
        await git(
          gate,
          "rev-list",
          "--reverse",
          "--topo-order",
          "--no-merges",
          "--cherry-pick",
          "--right-only",
          `${mainSha}...${prHead}`
        )
      )
        .split("\n")
        .filter((c) => c !== "");
      if (commits.length === 0) fail(`PR #${pr} has no commits of its own beyond main.`);
      const replay = await replayCommits(gate, commits);
      if (replay.kind === "conflict") {
        fail(
          `The rebase onto main conflicts in: ${replay.paths.join(", ")}.\n` +
            "  Resolve it in your worktree (git rebase origin/main), push, and re-run."
        );
      }
      if (replay.kind === "error") {
        fail(`Replaying PR #${pr} onto main failed, and not on a conflict:\n${replay.message}`);
      }
    }
    const tested = await git(gate, "rev-parse", "HEAD");

    await $`bun install`.cwd(gate).quiet();
    console.log(`\n▶ Full pre-merge gate on ${tested.slice(0, 8)} in ${gate} (attempt ${attempt} of ${MAX_ATTEMPTS})`);
    const result = await $`bun run scripts/test-gate.ts --mode=merge`.cwd(gate).nothrow();
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
      console.log(`\n✓ Dry run: the merge gate passed on ${tested.slice(0, 8)}. Nothing pushed or merged.\n`);
      return;
    }

    if (tested !== info.headRefOid) {
      // The gate just ran the full pyramid on exactly this commit, lint
      // included, so the pre-push hook would only repeat part of it.
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
    // Attempts that are retried stay quiet; only a final refusal is shown.
    for (let tries = 1; ; tries++) {
      const merged = await $`gh pr merge ${pr} --squash --match-head-commit ${tested}`.quiet().nothrow();
      if (merged.exitCode === 0) break;
      if (tries === 1) console.log("  waiting for GitHub to show the pushed commit as the PR head…");
      if (tries === MERGE_TRIES) {
        console.error(`${merged.stdout.toString()}${merged.stderr.toString()}`.trim());
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
