#!/usr/bin/env bun
// `bun run merge <PR#>` — queues a PR in the team-wide merge queue
// (./merge-queue.ts) and waits for it to land or be ejected. Whichever
// machine takes the queue's lock runs the next round, for every queued PR.
//
// Why merge time and not push time (ADR-047): most pushes are WIP or
// review-fix pushes, and running the full pyramid on each paid for it several
// times per PR. It also tested the wrong thing — the branch on its own base,
// not what lands. Two branches can each pass alone and still break main
// together. So a push only lints, and the full pyramid runs here, on exactly
// what lands.
//
// A round:
//
// - Stacks every queued PR, in queue order, onto current main in one
//   persistent gate checkout (`.claude/worktrees/_gate`), whose target/ stays
//   warm so a round recompiles only what changed since the last. A PR that
//   conflicts with main or with a PR ahead of it is ejected, with a PR
//   comment, and the stack carries on without it.
// - Runs the full gate once on the top of the stack.
// - On a pass, lands the PRs in order. After the batch, main's tree is exactly
//   the tree that was tested. (As in any batching queue, the intermediate
//   commits between PRs of one batch aren't tested on their own.)
// - On a failure, retests the first half of the batch alone, and so on down
//   to one PR, which is ejected with the gate's output — so every PR that
//   passes lands, and a batch of N costs about log2(N) extra runs to find the
//   one that broke it.
//
// main moves only through a queue landing, so a round is never invalidated by
// another merge. Landing still checks, before every PR, that main hasn't
// moved outside the queue (a release's version bump is pushed straight to
// main), that the PR's head hasn't moved and that this process still holds
// the lock — and stops at the first that fails. Whatever it didn't land stays
// queued for the next round, which tests it afresh.
//
// It tests the PR as pushed: commit and push first. A PR leaves the queue when
// it lands, when it's ejected, or when the `bun run merge` that queued it
// stops waiting (Ctrl-C, or MAX_WAIT_MS).
//
//   bun run merge <PR#>             queue, then wait for it to land
//   bun run merge <PR#> --dry-run   gate this PR alone on current main; no queue, push or merge

import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { $ } from "bun";
import { acquireGateLock, DISABLE_ENV_VAR, formatDuration, MERGE_LOCK_PATH, registerLockRelease, statusLogger } from "./gate-lock";
import { bisectBatch, describeLock, HEARTBEAT_MS, lockInfoHere, MergeQueue, StaleWatch } from "./merge-queue";

/** How long `bun run merge` waits for its PR before giving up its place. */
const MAX_WAIT_MS = 3 * 60 * 60 * 1000;

/** How often a waiting merge looks at the queue. */
const POLL_MS = 15 * 1000;

/** Merge attempts, 3s apart, while GitHub catches up with a push. */
const MERGE_TRIES = 10;

/**
 * How long a round waits for this machine's gate checkout. It's only ever
 * held by another round (impossible while this one holds the queue's lock)
 * or a --dry-run, so the cap is generous.
 */
const GATE_CHECKOUT_WAIT_MS = 60 * 60 * 1000;

/** Lines of gate output quoted in an ejected PR's comment. */
const EJECT_TAIL_LINES = 40;

/** The persistent gate checkout, relative to the main repository root. */
export const GATE_CHECKOUT = join(".claude", "worktrees", "_gate");

/** git options that disable repo hooks; see `git()` for why the gate needs them. */
const NO_HOOKS = ["-c", "core.hooksPath=/dev/null"];

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
  state: string;
  baseRefName: string;
}

/** How replaying a PR's commits onto main went. */
export type ReplayResult =
  | { kind: "ok" }
  | { kind: "conflict"; paths: string[] }
  | { kind: "error"; message: string };

/**
 * Cherry-pick `commits` onto the checkout in `cwd`. On any failure no
 * cherry-pick is left in progress, and a conflict restores HEAD; a change
 * that was already in the checkout before the pick is not this function's to
 * discard (the gate resets its checkout at the start of every round).
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
  const pick = await $`git ${NO_HOOKS} cherry-pick --keep-redundant-commits ${commits}`.cwd(cwd).quiet().nothrow();
  if (pick.exitCode === 0) return { kind: "ok" };

  const unmerged = (await $`git diff --name-only --diff-filter=U`.cwd(cwd).quiet().nothrow().text())
    .split("\n")
    .filter((p) => p !== "");
  await $`git ${NO_HOOKS} cherry-pick --abort`.cwd(cwd).quiet().nothrow();
  // Clear any sequencer state an abort left behind, or every later merge
  // would fail with "cherry-pick already in progress".
  await $`git ${NO_HOOKS} cherry-pick --quit`.cwd(cwd).quiet().nothrow();
  if (unmerged.length > 0) return { kind: "conflict", paths: unmerged };
  const message = `${pick.stderr.toString()}${pick.stdout.toString()}`.trim();
  return { kind: "error", message: message || `git cherry-pick exited with code ${pick.exitCode}` };
}

/**
 * Runs git in `cwd` with hooks disabled and returns its trimmed stdout.
 *
 * The repo's post-checkout hook runs `bun install`, which rewrites bun.lock
 * whenever the checked-out lockfile differs from what the installed Bun would
 * write. Fired by the gate's own checkouts, that left bun.lock dirty, and the
 * replay then refused to cherry-pick any PR commit touching it. The gate runs
 * its own `bun install` once the tree is final, so the hook adds nothing here.
 * Every git command the gate runs goes without hooks, for the same reason; the
 * few that bypass this helper pass `NO_HOOKS` themselves.
 */
async function git(cwd: string, ...args: string[]): Promise<string> {
  return (await $`git ${NO_HOOKS} ${args}`.cwd(cwd).quiet().text()).trim();
}

/**
 * The PR branch's head as the remote has it right now, or null when it isn't
 * there. GitHub's API can lag a fresh push by a few seconds, so the head is
 * read from origin itself, when it's needed — never from the API, never once
 * at startup.
 */
async function remoteHead(cwd: string, branch: string): Promise<string | null> {
  const ref = `refs/heads/${branch}`;
  const out = await $`git ${NO_HOOKS} ls-remote origin ${ref}`.cwd(cwd).quiet().nothrow();
  if (out.exitCode !== 0) return null;
  // ls-remote matches refs by suffix; take only the exact branch.
  const sha = out.stdout
    .toString()
    .split("\n")
    .map((line) => line.split(/\s+/))
    .find(([, name]) => name === ref)?.[0];
  return sha !== undefined && /^[0-9a-f]{40}$/.test(sha) ? sha : null;
}

function fail(message: string): never {
  console.error(`\n✗ ${message}\n`);
  process.exit(1);
}

async function viewPr(pr: number): Promise<PullRequest | null> {
  const out = await $`gh pr view ${pr} --json headRefName,state,baseRefName`.quiet().nothrow();
  return out.exitCode === 0 ? (JSON.parse(out.stdout.toString()) as PullRequest) : null;
}

/**
 * The gate checkout, created on first use. Detached, so it never holds a
 * branch another worktree might want. Each round runs `bun install` there
 * once the stack is built — a no-op when nothing changed, and it picks up a
 * changed lockfile when something did.
 */
async function prepareGateCheckout(repoRoot: string): Promise<string> {
  const path = join(repoRoot, GATE_CHECKOUT);
  if (!existsSync(path)) {
    console.log(`\n▶ Creating the persistent gate checkout at ${path}`);
    await $`git ${NO_HOOKS} worktree add --detach ${path} origin/main`.cwd(repoRoot).quiet();
  }
  return path;
}

/** A PR as a round sees it. */
export interface QueuedPr {
  pr: number;
  headRefName: string;
  /** The PR branch's head on origin when the round read it. */
  head: string;
}

/** One PR replayed onto the stack below it. */
export interface StackEntry extends QueuedPr {
  /** The PR's own commits, as replayed. */
  commits: string[];
  /** The tree of main plus this PR and every PR below it. */
  tree: string;
}

export interface StackResult {
  stack: StackEntry[];
  ejected: { pr: number; reason: string }[];
}

/**
 * Resets the gate checkout at `cwd` to `mainSha` — a clean slate, with no
 * leftovers from the previous round. Not `clean -x`: target/ and
 * node_modules/ are the warm state this checkout exists to keep. Ignored build
 * output the gate itself produces or reads is removed, though: a file a PR
 * deleted could survive there and mask a failure.
 */
async function resetGateCheckout(cwd: string, mainSha: string): Promise<void> {
  await git(cwd, "checkout", "--quiet", "--force", "--detach", mainSha);
  await git(cwd, "clean", "-fdq");
  await git(cwd, "clean", "-fdqX", "--", ...STALE_OUTPUT_PATHS);
}

/**
 * Replays each PR's commits, in order, onto `mainSha` and the PRs below it,
 * in the checkout at `cwd` (already reset to `mainSha`). A PR that conflicts
 * with main or with a PR below it — or that adds nothing beyond them — is
 * left out with the reason, and the stack carries on without it. The checkout
 * ends at the top of the stack.
 *
 * Replaying rather than checking out each PR's own base: that detour rewrote
 * every file main had changed since the PR branched, then rewrote it back, and
 * cargo, which judges staleness by modification time, recompiled all of them
 * on a checkout that exists to stay warm. Starting from main touches only the
 * files the PRs change.
 */
export async function buildStack(cwd: string, mainSha: string, prs: QueuedPr[]): Promise<StackResult> {
  const stack: StackEntry[] = [];
  const ejected: { pr: number; reason: string }[] = [];
  for (const item of prs) {
    const below = await git(cwd, "rev-parse", "HEAD");
    // The commit set rebase would replay: linear (merge commits dropped,
    // their changes arriving through the commits around them), in graph
    // order, and skipping any commit whose patch main already has.
    const commits = (
      await git(cwd, "rev-list", "--reverse", "--topo-order", "--no-merges", "--cherry-pick", "--right-only", `${mainSha}...${item.head}`)
    )
      .split("\n")
      .filter((c) => c !== "");
    if (commits.length === 0) {
      ejected.push({ pr: item.pr, reason: "it has no commits of its own beyond main." });
      continue;
    }
    const replay = await replayCommits(cwd, commits);
    if (replay.kind === "conflict") {
      const ahead = stack.length > 0 ? ` or with ${stack.map((e) => `#${e.pr}`).join(", ")}, queued ahead of it,` : "";
      ejected.push({
        pr: item.pr,
        reason: `it conflicts with main${ahead} in: ${replay.paths.join(", ")}. Rebase onto origin/main, push, and re-run \`bun run merge ${item.pr}\`.`,
      });
      continue;
    }
    if (replay.kind === "error") {
      ejected.push({ pr: item.pr, reason: `replaying it onto main failed, and not on a conflict:\n${replay.message}` });
      continue;
    }
    // Commits that became empty are kept, so a PR the stack already contains
    // replays "successfully" onto the same tree. Leave it out rather than
    // squash-merge an empty diff.
    const tree = await git(cwd, "rev-parse", "HEAD^{tree}");
    if (tree === (await git(cwd, "rev-parse", `${below}^{tree}`))) {
      await git(cwd, "reset", "--quiet", "--hard", below);
      ejected.push({ pr: item.pr, reason: "it has no changes beyond main: main already contains everything it does." });
      continue;
    }
    stack.push({ ...item, commits, tree });
  }
  return { stack, ejected };
}

/**
 * Runs the full gate in `cwd`, streaming its output through and keeping the
 * tail for an ejected PR's comment.
 */
async function runGate(cwd: string): Promise<{ passed: boolean; tail: string }> {
  await $`bun install`.cwd(cwd).quiet();
  const proc = Bun.spawn(["bun", "run", "scripts/test-gate.ts", "--mode=merge"], {
    cwd,
    stdout: "pipe",
    stderr: "pipe",
    stdin: "ignore",
  });
  const lines: string[] = [];
  const keep = (text: string) => {
    lines.push(...text.split("\n"));
    if (lines.length > EJECT_TAIL_LINES * 2) lines.splice(0, lines.length - EJECT_TAIL_LINES);
  };
  const pump = async (stream: typeof proc.stdout, out: { write(text: string): unknown }) => {
    const decoder = new TextDecoder();
    for await (const chunk of stream) {
      const text = decoder.decode(chunk, { stream: true });
      out.write(text);
      keep(text);
    }
  };
  await Promise.all([pump(proc.stdout, process.stdout), pump(proc.stderr, process.stderr), proc.exited]);
  return { passed: proc.exitCode === 0, tail: lines.slice(-EJECT_TAIL_LINES).join("\n").trim() };
}

/** Takes `pr` out of the queue and says why on the PR, where its author will see it. */
async function eject(queue: MergeQueue, pr: number, reason: string): Promise<void> {
  console.log(`\n✗ #${pr} left the queue: ${reason.split("\n")[0]}`);
  await queue.dequeue(pr);
  await $`gh pr comment ${pr} --body ${`**Merge queue:** #${pr} was taken out of the queue — ${reason}`}`.quiet().nothrow();
}

/**
 * Squash-merges each stacked PR in order, checking before each that nothing
 * the round relied on has changed. Returns the PRs landed. Stops at the first
 * check that fails; the rest stay queued for the next round.
 */
async function land(
  gate: string,
  queue: MergeQueue,
  repo: string,
  stack: StackEntry[],
  baseTree: string,
  stillHolding: () => boolean
): Promise<number[]> {
  const landed: number[] = [];
  let expectedTree = baseTree;
  const stop = (why: string) => {
    const rest = stack.slice(landed.length).map((e) => `#${e.pr}`);
    console.log(`\n⚠ Stopped landing: ${why}. ${rest.join(", ")} stay${rest.length === 1 ? "s" : ""} queued for the next round.`);
    if (landed.length > 0) {
      console.log("  main now holds part of a tested batch; the next round tests the rest on top of it.");
    }
  };
  for (const entry of stack) {
    if (!stillHolding()) return (stop("this machine no longer holds the queue's lock"), landed);
    if (!(await queue.isQueued(entry.pr))) return (stop(`#${entry.pr} left the queue`), landed);
    await git(gate, "fetch", "--quiet", "origin", "main");
    const mainSha = await git(gate, "rev-parse", "origin/main");
    if ((await git(gate, "rev-parse", `${mainSha}^{tree}`)) !== expectedTree) {
      return (stop("main moved outside the queue"), landed);
    }
    if ((await remoteHead(gate, entry.headRefName)) !== entry.head) return (stop(`#${entry.pr}'s head moved`), landed);

    // The PR's commits onto main as it now is. Main's tree is the tree below
    // this PR in the stack, so replaying gives exactly the tested tree.
    await git(gate, "checkout", "--quiet", "--force", "--detach", mainSha);
    const replay = await replayCommits(gate, entry.commits);
    if (replay.kind !== "ok" || (await git(gate, "rev-parse", "HEAD^{tree}")) !== entry.tree) {
      return (stop(`replaying #${entry.pr} onto main didn't reproduce the tested tree`), landed);
    }
    const tip = await git(gate, "rev-parse", "HEAD");
    if (tip !== entry.head) {
      // The gate ran the full pyramid on this tree, lint included, so the
      // pre-push hook would only repeat part of it.
      const pushed = await $`git ${NO_HOOKS} push --quiet --no-verify --force-with-lease=${entry.headRefName}:${entry.head} origin HEAD:${entry.headRefName}`
        .cwd(gate)
        .quiet()
        .nothrow();
      if (pushed.exitCode !== 0) return (stop(`#${entry.pr}'s head moved`), landed);
    }

    // --match-head-commit: GitHub refuses the merge if the PR's head is no
    // longer the commit just pushed. Right after a force-push GitHub can
    // briefly still report the old head, so retry for a few seconds.
    let merged = false;
    for (let tries = 1; tries <= MERGE_TRIES && !merged; tries++) {
      merged = (await $`gh pr merge ${entry.pr} --squash --match-head-commit ${tip}`.quiet().nothrow()).exitCode === 0;
      if (!merged) await Bun.sleep(3000);
    }
    if (!merged) return (stop(`GitHub refused to merge #${entry.pr} at ${tip.slice(0, 8)}`), landed);

    landed.push(entry.pr);
    expectedTree = entry.tree;
    console.log(`✓ #${entry.pr} landed.`);
    await queue.dequeue(entry.pr);
    // Through the API rather than `gh pr merge --delete-branch`, which also
    // tries to delete the local branch — checked out in the PR's worktree.
    await $`gh api -X DELETE repos/${repo}/git/refs/heads/${entry.headRefName}`.quiet().nothrow();
  }
  await git(gate, "fetch", "--quiet", "origin", "main");
  if ((await git(gate, "rev-parse", "origin/main^{tree}")) !== expectedTree) {
    console.log("\n⚠ main's tree after landing differs from the tested tree — check main.");
  }
  return landed;
}

/** Releases the queue's lock if this process holds it — for a signal mid-round. */
let releaseHeldLock: (() => Promise<void>) | null = null;

/**
 * Runs one round while holding the queue's lock at `lockSha`: every queued PR,
 * stacked, gated, landed or bisected. Releases the lock when done.
 */
async function runRound(queue: MergeQueue, lockSha: string, repoRoot: string, repo: string): Promise<void> {
  let current = lockSha;
  let holding = true;
  let prs: number[] = [];
  // Renewals are chained, never concurrent: a renewal still in flight when
  // the round releases the lock would otherwise re-take it for a process
  // that is done, and every waiter would sit out the stale window.
  let renewals: Promise<void> = Promise.resolve();
  const renew = () => {
    renewals = renewals.then(async () => {
      if (!holding) return;
      const next = await queue.renew(current, lockInfoHere(prs));
      if (next === null) holding = false;
      else current = next;
    });
    return renewals;
  };
  const release = async () => {
    await renewals;
    if (holding) await queue.release(current);
    holding = false;
  };
  releaseHeldLock = release;
  const heartbeat = setInterval(() => void renew(), HEARTBEAT_MS);

  // This machine's gate checkout, shared with --dry-run, which doesn't take
  // the queue's lock.
  const checkout = await acquireGateLock({ lockPath: MERGE_LOCK_PATH, what: "gate checkout", maxWaitMs: GATE_CHECKOUT_WAIT_MS });
  try {
    if (!checkout.held) {
      console.error("\n✗ This machine's gate checkout stayed busy; releasing the queue for another machine.");
      return;
    }
    const gate = await prepareGateCheckout(repoRoot);

    // Every queued PR that can still land, as origin has it now.
    let candidates: QueuedPr[] = [];
    for (const pr of await queue.queued()) {
      const info = await viewPr(pr);
      if (info === null || info.state !== "OPEN") {
        await queue.dequeue(pr);
        continue;
      }
      if (info.baseRefName !== "main") {
        await eject(queue, pr, `it targets ${info.baseRefName}; the queue merges into main only.`);
        continue;
      }
      const head = await remoteHead(gate, info.headRefName);
      if (head === null) {
        await eject(queue, pr, `its branch ${info.headRefName} isn't on origin.`);
        continue;
      }
      candidates.push({ pr, headRefName: info.headRefName, head });
    }

    while (candidates.length > 0 && holding) {
      await git(gate, "fetch", "--quiet", "origin", "main", ...candidates.map((c) => c.headRefName));
      const mainSha = await git(gate, "rev-parse", "origin/main");
      const baseTree = await git(gate, "rev-parse", `${mainSha}^{tree}`);
      await resetGateCheckout(gate, mainSha);
      const { stack, ejected } = await buildStack(gate, mainSha, candidates);
      for (const { pr, reason } of ejected) await eject(queue, pr, reason);
      if (stack.length === 0) return;

      prs = stack.map((e) => e.pr);
      await renew();
      const names = prs.map((p) => `#${p}`).join(", ");
      console.log(`\n▶ Merge gate on ${names} (stacked on ${mainSha.slice(0, 8)}) in ${gate}`);
      const gate_ = await runGate(gate);
      if (!holding) {
        console.error("\n✗ This machine lost the queue's lock during the gate (another machine took it over); nothing landed.");
        return;
      }
      if (gate_.passed) {
        await land(gate, queue, repo, stack, baseTree, () => holding);
        return;
      }
      if (stack.length === 1) {
        await eject(queue, stack[0].pr, `the merge gate failed on it, rebased onto main. Reproduce with \`git rebase origin/main\` and \`bun run test:changed\`, fix, push, and re-run \`bun run merge ${stack[0].pr}\`.\n\n\`\`\`\n${gate_.tail}\n\`\`\``);
        return;
      }
      candidates = bisectBatch(stack).map(({ pr, headRefName, head }) => ({ pr, headRefName, head }));
      console.log(`\n⟳ The batch failed; retesting ${candidates.map((c) => `#${c.pr}`).join(", ")} on their own.`);
    }
  } finally {
    clearInterval(heartbeat);
    checkout.release();
    await release();
    releaseHeldLock = null;
  }
}

/** `--dry-run`: gates this one PR on current main in the gate checkout. */
async function dryRun(pr: number, info: PullRequest, repoRoot: string): Promise<void> {
  const checkout = await acquireGateLock({ lockPath: MERGE_LOCK_PATH, what: "gate checkout", maxWaitMs: GATE_CHECKOUT_WAIT_MS });
  if (!checkout.held) fail("This machine's gate checkout stayed busy. Re-run when it is free.");
  registerLockRelease(checkout);
  const gate = await prepareGateCheckout(repoRoot);
  const head = await remoteHead(gate, info.headRefName);
  if (head === null) fail(`${info.headRefName} was not found on origin.`);
  await git(gate, "fetch", "--quiet", "origin", "main", info.headRefName);
  const mainSha = await git(gate, "rev-parse", "origin/main");
  await resetGateCheckout(gate, mainSha);
  const { stack, ejected } = await buildStack(gate, mainSha, [{ pr, headRefName: info.headRefName, head }]);
  if (stack.length === 0) fail(`PR #${pr} can't be gated: ${ejected[0]?.reason ?? "nothing to test."}`);
  console.log(`\n▶ Merge gate on #${pr} (stacked on ${mainSha.slice(0, 8)}) in ${gate}`);
  const result = await runGate(gate);
  if (!result.passed) fail(`The merge gate failed on #${pr}, rebased onto main.`);
  console.log(`\n✓ Dry run: the merge gate passed on #${pr}. Nothing queued, pushed or merged.\n`);
}

async function main(): Promise<void> {
  let args: MergeArgs;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (err) {
    fail(err instanceof Error ? err.message : String(err));
  }
  const { pr } = args;

  const here = process.cwd();
  const repo = (await $`gh repo view --json nameWithOwner --jq .nameWithOwner`.quiet().text()).trim();
  const info = await viewPr(pr);
  if (info === null) fail(`Could not read PR #${pr}.`);
  if (info.state !== "OPEN") fail(`PR #${pr} is ${info.state.toLowerCase()}, not open.`);
  if (info.baseRefName !== "main") fail(`PR #${pr} targets ${info.baseRefName}; this command merges into main only.`);

  // Unpushed work in the caller's checkout is not what gets tested. Say so
  // rather than let a green gate imply it covered local commits.
  // EnterWorktree names the local branch `worktree-<name>` for remote <name>.
  const localBranch = await git(here, "rev-parse", "--abbrev-ref", "HEAD");
  const pushedHead = await remoteHead(here, info.headRefName);
  if (pushedHead === null) fail(`${info.headRefName} was not found on origin.`);
  if (isPrBranch(localBranch, info.headRefName)) {
    const localHead = await git(here, "rev-parse", "HEAD");
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

  // The gate checkout must never be shared, so the machine-local lock on it
  // can't be opted out of.
  if (process.env[DISABLE_ENV_VAR]) fail(`${DISABLE_ENV_VAR} is set; a merge always takes its locks. Unset it and re-run.`);
  const repoRoot = resolve(dirname(await git(here, "rev-parse", "--path-format=absolute", "--git-common-dir")));

  if (args.dryRun) return dryRun(pr, info, repoRoot);

  const queue = new MergeQueue(here);
  await queue.enqueue(pr, pushedHead);
  console.log(`\n▶ #${pr} queued. Whichever machine holds the queue's lock gates and lands it.`);

  // Stopping waiting gives up the PR's place: a queued PR nobody is waiting on
  // would otherwise land later, unannounced.
  let settled = false;
  const giveUp = async (signal: "SIGINT" | "SIGTERM" | "SIGHUP") => {
    // Mid-round, the lock goes too, rather than make every waiter sit out the
    // stale window.
    await releaseHeldLock?.();
    if (!settled) await queue.dequeue(pr);
    process.kill(process.pid, signal);
  };
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"] as const) process.once(signal, () => void giveUp(signal));

  const started = Date.now();
  const watch = new StaleWatch();
  const status = statusLogger((m) => console.log(m), Date.now);
  for (;;) {
    const now = await viewPr(pr);
    if (now?.state === "MERGED") {
      settled = true;
      console.log(
        `\n✓ PR #${pr} merged.\n` +
          "  Next: ExitWorktree({action: \"remove\", discard_changes: true}), then from the primary checkout\n" +
          "  `git pull origin main` and `bun run gh:status <issue#> \"Done\"`.\n"
      );
      return;
    }
    if (!(await queue.isQueued(pr))) {
      settled = true;
      fail(`PR #${pr} left the queue without landing — the reason is in a comment on the PR.`);
    }
    if (Date.now() - started > MAX_WAIT_MS) {
      await queue.dequeue(pr);
      settled = true;
      fail(`PR #${pr} didn't land within ${formatDuration(MAX_WAIT_MS)}; it has left the queue. Re-run when the queue is shorter.`);
    }

    const sha = await queue.lockSha();
    const stale = watch.observe(sha);
    if (sha === null || stale) {
      if (stale) {
        const holder = await queue.lockInfo(sha as string);
        console.log(`\n⚠ The queue's lock hasn't moved in 10 minutes${holder ? ` (${describeLock(holder)})` : ""}; taking it over.`);
      }
      const held = await queue.tryAcquire(lockInfoHere(), sha ?? "");
      if (held !== null) {
        await runRound(queue, held, repoRoot, repo);
        continue;
      }
    }
    const holder = sha === null ? null : await queue.lockInfo(sha);
    const queued = await queue.queued();
    status(
      `${sha ?? ""}:${queued.length}`,
      `  queued (#${pr}, ${queued.length} in the queue${holder ? `; running: ${describeLock(holder)}` : ""}) — waited ${formatDuration(Date.now() - started)}`
    );
    await Bun.sleep(POLL_MS);
  }
}

if (import.meta.main) {
  await main();
}
