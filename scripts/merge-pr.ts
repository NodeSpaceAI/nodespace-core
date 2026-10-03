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
import {
  acquireGateLock,
  DISABLE_ENV_VAR,
  formatDuration,
  isPidAlive,
  MERGE_LOCK_PATH,
  readHolder,
  registerLockRelease,
  statusLogger,
} from "./gate-lock";
import {
  bisectBatch,
  changesDependencies,
  changesGate,
  describeLock,
  fenced,
  gateVerdict,
  HEARTBEAT_MS,
  type Lander,
  landStack,
  lockInfoHere,
  MergeQueue,
  StaleWatch,
  stripAnsi,
} from "./merge-queue";

/** How long `bun run merge` waits for its PR before giving up its place. */
const MAX_WAIT_MS = 3 * 60 * 60 * 1000;

/** How often a waiting merge looks at the queue. */
const POLL_MS = 15 * 1000;

/**
 * The longest a waiter backs off after rounds that got nowhere (a broken gate
 * checkout, a machine out of disk): long enough not to hammer origin and
 * GitHub, short enough that a healthy machine soon takes the round instead.
 */
const MAX_BACKOFF_MS = 4 * 60 * 1000;

/** Merge attempts, 3s apart, while GitHub catches up with a push. */
const MERGE_TRIES = 10;

/**
 * How long a waiter or --dry-run waits for this machine's gate checkout. A
 * waiter takes it before the queue's lock, so the wait never holds up another
 * machine; it's held only by a round or a --dry-run here.
 */
const GATE_CHECKOUT_WAIT_MS = 60 * 60 * 1000;

/** How long a waiter tries for this machine's gate checkout before sitting the round out. */
const WAITER_CHECKOUT_WAIT_MS = 5 * 1000;

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
  title: string;
  headRefName: string;
  state: string;
  baseRefName: string;
  isDraft: boolean;
  /** MERGEABLE, CONFLICTING, or UNKNOWN while GitHub computes it. */
  mergeable: string;
  /** A PR from a fork: its branch isn't origin's to push to or land from. */
  isCrossRepository: boolean;
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
 * Merges `head` into the checkout in `cwd` as one commit holding the PR's net
 * change — what `git merge` would produce, without the merge commit. On any
 * failure the checkout is back at its HEAD.
 *
 * This is how a branch that merged main lands. Its merge commits hold what the
 * author did while merging — conflict resolutions, fix-ups for what main
 * changed — and a commit-by-commit replay drops them. A three-way merge of
 * the head sees them.
 *
 * The commit's subject is `title`, the PR's: GitHub titles the squash of a
 * one-commit PR after that commit, so this is what main's history shows.
 *
 * `--allow-empty`: a head the checkout already contains merges to nothing,
 * and the commit must still succeed so the caller can tell "no changes" from
 * a failure by comparing trees.
 *
 * As in `replayCommits`, only unmerged paths make a failure a conflict.
 */
export async function mergeNetChange(cwd: string, head: string, title: string): Promise<ReplayResult> {
  const merge = await $`git ${NO_HOOKS} merge --squash ${head}`.cwd(cwd).quiet().nothrow();
  if (merge.exitCode === 0) {
    const commit = await $`git ${NO_HOOKS} commit --quiet --allow-empty -m ${title}`.cwd(cwd).quiet().nothrow();
    if (commit.exitCode === 0) return { kind: "ok" };
    await $`git ${NO_HOOKS} reset --quiet --hard HEAD`.cwd(cwd).quiet().nothrow();
    const message = `${commit.stderr.toString()}${commit.stdout.toString()}`.trim();
    return { kind: "error", message: message || `git commit exited with code ${commit.exitCode}` };
  }

  const unmerged = (await $`git diff --name-only --diff-filter=U`.cwd(cwd).quiet().nothrow().text())
    .split("\n")
    .filter((p) => p !== "");
  await $`git ${NO_HOOKS} reset --quiet --hard HEAD`.cwd(cwd).quiet().nothrow();
  if (unmerged.length > 0) return { kind: "conflict", paths: unmerged };
  const message = `${merge.stderr.toString()}${merge.stdout.toString()}`.trim();
  return { kind: "error", message: message || `git merge exited with code ${merge.exitCode}` };
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
  return (await readRemoteHead(cwd, branch)).sha;
}

/** remoteHead, telling "origin couldn't be asked" (`reachable: false`) from "no such branch". */
async function readRemoteHead(cwd: string, branch: string): Promise<{ reachable: boolean; sha: string | null }> {
  const ref = `refs/heads/${branch}`;
  const out = await $`git ${NO_HOOKS} ls-remote origin ${ref}`.cwd(cwd).quiet().nothrow();
  if (out.exitCode !== 0) return { reachable: false, sha: null };
  // ls-remote matches refs by suffix; take only the exact branch.
  const sha = out.stdout
    .toString()
    .split("\n")
    .map((line) => line.split(/\s+/))
    .find(([, name]) => name === ref)?.[0];
  return { reachable: true, sha: sha !== undefined && /^[0-9a-f]{40}$/.test(sha) ? sha : null };
}

function fail(message: string): never {
  console.error(`\n✗ ${message}\n`);
  process.exit(1);
}

async function viewPr(pr: number): Promise<PullRequest | null> {
  const out = await $`gh pr view ${pr} --json title,headRefName,state,baseRefName,isDraft,mergeable,isCrossRepository`.quiet().nothrow();
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
  /** The PR's title: the subject of the commit that holds its net change. */
  title: string;
  headRefName: string;
  /** The PR branch's head on origin when the round read it. */
  head: string;
}

/** One PR replayed onto the stack below it. */
export interface StackEntry extends QueuedPr {
  /** What was replayed: the PR's own commits, or the one commit holding its net change. */
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
 * in the checkout at `cwd` (already reset to `mainSha`). A branch that holds
 * merge commits is merged as its net change instead, so one that merged main
 * lands as it is; so is a PR whose commits don't replay one by one. A PR that
 * conflicts with main or with a PR below it either way — or that adds nothing
 * beyond them — is left out with the reason, and the stack carries on without
 * it. The checkout ends at the top of the stack.
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
    // What the author did in a merge commit — a conflict resolution, a fix-up
    // for something main changed — is in none of the commits listed above.
    const hasMerges = (await git(cwd, "rev-list", "--merges", "--max-count=1", `${mainSha}..${item.head}`)) !== "";
    if (commits.length === 0 && !hasMerges) {
      ejected.push({ pr: item.pr, reason: "it has no commits of its own beyond main." });
      continue;
    }
    // Two ways to put the PR on the stack: its commits one by one, or its net
    // change as one commit. The merge squashes either, so they land the same.
    // A branch with merge commits takes its net change first, since only that
    // carries what the merges hold; any other branch replays first, which also
    // accepts a commit main already has by patch. Each falls back to the other
    // on a conflict. The files named on an ejection are the merge's, which are
    // the ones the author has to resolve.
    let replayed = commits;
    let replay: ReplayResult;
    if (hasMerges) {
      replay = await mergeNetChange(cwd, item.head, item.title);
      if (replay.kind === "ok") replayed = [await git(cwd, "rev-parse", "HEAD")];
      else if (replay.kind === "conflict" && commits.length > 0 && (await replayCommits(cwd, commits)).kind === "ok") replay = { kind: "ok" };
    } else {
      replay = await replayCommits(cwd, commits);
      if (replay.kind === "conflict") {
        replay = await mergeNetChange(cwd, item.head, item.title);
        if (replay.kind === "ok") replayed = [await git(cwd, "rev-parse", "HEAD")];
      }
    }
    if (replay.kind === "conflict") {
      const ahead = stack.length > 0 ? ` or with ${stack.map((e) => `#${e.pr}`).join(", ")}, queued ahead of it,` : "";
      ejected.push({
        pr: item.pr,
        reason: `it conflicts with main${ahead} in: ${replay.paths.join(", ")}. Merge or rebase origin/main, resolve the conflicts, push, and re-run \`bun run merge ${item.pr}\`.`,
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
    stack.push({ ...item, commits: replayed, tree });
  }
  return { stack, ejected };
}

/**
 * Runs the full gate in `cwd` (a stack on `mainSha`), streaming its output
 * through and keeping the tail for an ejected PR's comment.
 *
 * The verdict keeps the two failure domains apart: `failed` is the code under
 * test, `infra` is this machine (the gate's own GATE_INFRA_EXIT: too little
 * disk, no machine slot). An infra failure must never cost a PR its place.
 *
 * A `bun install` that fails twice is tested against main: if main installs,
 * the stack broke it — through package.json, bun.lock, or any of the scripts
 * install runs — and it's a failure to bisect; if main fails too, it's the
 * registry, the network or this machine. That leaves the checkout at main,
 * which the next round resets anyway.
 */
async function runGate(cwd: string, mainSha: string): Promise<{ verdict: "passed" | "failed" | "infra"; tail: string }> {
  const changed = (await git(cwd, "diff", "--name-only", mainSha, "HEAD")).split("\n");
  let install = await $`bun install`.cwd(cwd).quiet().nothrow();
  if (install.exitCode !== 0) install = await $`bun install`.cwd(cwd).quiet().nothrow();
  if (install.exitCode !== 0) {
    const output = `${install.stdout}${install.stderr}`.trim().split("\n").slice(-EJECT_TAIL_LINES).join("\n");
    let stackAtFault = changesDependencies(changed);
    if (!stackAtFault) {
      await git(cwd, "checkout", "--quiet", "--force", "--detach", mainSha);
      stackAtFault = (await $`bun install`.cwd(cwd).quiet().nothrow()).exitCode === 0;
    }
    return { verdict: stackAtFault ? "failed" : "infra", tail: `bun install failed:\n${output}` };
  }
  const proc = Bun.spawn(["bun", "run", "scripts/test-gate.ts", "--mode=merge"], {
    cwd,
    stdout: "pipe",
    stderr: "pipe",
    stdin: "ignore",
  });
  const lines: string[] = [];
  const pump = async (stream: typeof proc.stdout, out: { write(text: string): unknown }) => {
    const decoder = new TextDecoder();
    let partial = "";
    for await (const chunk of stream) {
      const text = decoder.decode(chunk, { stream: true });
      out.write(text);
      // Whole lines only: a line split across two chunks is kept as one.
      const parts = (partial + text).split("\n");
      partial = parts.pop() ?? "";
      lines.push(...parts);
      if (lines.length > EJECT_TAIL_LINES * 2) lines.splice(0, lines.length - EJECT_TAIL_LINES);
    }
    if (partial !== "") lines.push(partial);
  };
  await Promise.all([pump(proc.stdout, process.stdout), pump(proc.stderr, process.stderr), proc.exited]);
  return { verdict: gateVerdict(proc.exitCode, changesGate(changed)), tail: lines.slice(-EJECT_TAIL_LINES).join("\n").trim() };
}

/** Takes `pr` out of the queue and says why on the PR, where its author will see it. */
async function eject(queue: MergeQueue, pr: number, reason: string): Promise<void> {
  console.log(`\n✗ #${pr} left the queue: ${reason.split("\n")[0]}`);
  await queue.dequeue(pr);
  await $`gh pr comment ${pr} --body ${`**Merge queue:** #${pr} was taken out of the queue — ${reason}`}`.quiet().nothrow();
}

/** The Lander (./merge-queue.ts) for the real gate checkout, origin and GitHub. */
function realLander(gate: string, queue: MergeQueue, repo: string, confirmHolding: () => Promise<boolean>): Lander {
  return {
    confirmHolding,
    isQueued: (pr) => queue.isQueued(pr),
    main: async () => {
      await git(gate, "fetch", "--quiet", "origin", "main");
      const sha = await git(gate, "rev-parse", "origin/main");
      return { sha, tree: await git(gate, "rev-parse", `${sha}^{tree}`) };
    },
    headOf: (branch) => remoteHead(gate, branch),
    replayOnto: async (mainSha, commits) => {
      await git(gate, "checkout", "--quiet", "--force", "--detach", mainSha);
      if ((await replayCommits(gate, commits)).kind !== "ok") return null;
      return { tree: await git(gate, "rev-parse", "HEAD^{tree}"), tip: await git(gate, "rev-parse", "HEAD") };
    },
    // The gate ran the full pyramid on this tree, lint included, so the
    // pre-push hook would only repeat part of it.
    push: async (branch, head, tip) =>
      (await $`git ${NO_HOOKS} push --quiet --no-verify --force-with-lease=${branch}:${head} origin ${tip}:refs/heads/${branch}`.cwd(gate).quiet().nothrow())
        .exitCode === 0,
    // --match-head-commit: GitHub refuses the merge if the PR's head is no
    // longer the commit just pushed. Right after a force-push GitHub can
    // briefly still report the old head, so retry for a few seconds.
    merge: async (pr, tip) => {
      let reason = "";
      for (let tries = 1; tries <= MERGE_TRIES; tries++) {
        const out = await $`gh pr merge ${pr} --squash --match-head-commit ${tip}`.quiet().nothrow();
        if (out.exitCode === 0) return { ok: true };
        reason = `${out.stderr}${out.stdout}`.trim() || `gh pr merge exited with code ${out.exitCode}`;
        await Bun.sleep(3000);
      }
      // Only the PR's own state makes a refusal definite. A merge that went
      // through with its response lost is a landing; a 5xx, a rate limit or
      // "base branch was modified" says nothing about the PR.
      const now = await viewPr(pr);
      if (now?.state === "MERGED") return { ok: true };
      if (now !== null && (now.isDraft || now.state === "CLOSED" || now.mergeable === "CONFLICTING")) {
        const why = now.isDraft ? "it's a draft" : now.state === "CLOSED" ? "it was closed" : "it conflicts with main";
        return { ok: false, reason: `${why} (${reason})`, definite: true };
      }
      return { ok: false, reason, definite: false };
    },
    landed: async (pr, branch) => {
      console.log(`✓ #${pr} landed.`);
      await queue.dequeue(pr);
      // Through the API rather than `gh pr merge --delete-branch`, which also
      // tries to delete the local branch — checked out in the PR's worktree.
      await $`gh api -X DELETE repos/${repo}/git/refs/heads/${branch}`.quiet().nothrow();
    },
    eject: (pr, reason) => eject(queue, pr, reason),
  };
}

/** Releases the queue's lock if this process holds it — for a signal mid-round. */
let releaseHeldLock: (() => Promise<void>) | null = null;

/**
 * Runs one round while holding the queue's lock at `lockSha` and this
 * machine's gate checkout: every queued PR, stacked, gated, then landed or
 * bisected. Releases the queue's lock when done. Never throws: an error ends
 * the round (logged), and the queue's lock is released for the next one.
 * Returns whether the queue moved — something landed or was ejected — so a
 * waiter can back off from rounds that get nowhere.
 */
async function runRound(queue: MergeQueue, lockSha: string, repoRoot: string, repo: string): Promise<boolean> {
  let moved = false;
  let current = lockSha;
  let holding = true;
  let prs: number[] = [];
  // Renewals are chained, never concurrent: a renewal still in flight when
  // the round releases the lock would otherwise re-take it for a process
  // that is done, and every waiter would sit out the stale window.
  let renewals: Promise<boolean> = Promise.resolve(true);
  const renew = (): Promise<boolean> => {
    renewals = renewals
      .then(async () => {
        if (!holding) return false;
        const result = await queue.renew(current, lockInfoHere(prs));
        if (result.status === "renewed") current = result.sha;
        if (result.status === "lost") holding = false;
        return result.status === "renewed";
      })
      .catch(() => false);
    return renewals;
  };
  const release = async () => {
    await renewals;
    if (holding) await queue.release(current);
    holding = false;
  };
  releaseHeldLock = release;
  const heartbeat = setInterval(() => void renew(), HEARTBEAT_MS);

  try {
    const gate = await prepareGateCheckout(repoRoot);

    // Every queued PR that can still land, as origin has it now. Only a
    // definite answer changes the queue: a PR GitHub can't be asked about
    // right now just sits this round out.
    let candidates: QueuedPr[] = [];
    for (const pr of await queue.queued()) {
      const info = await viewPr(pr);
      if (info === null) continue;
      if (info.state !== "OPEN") {
        await queue.dequeue(pr);
        moved = true;
        continue;
      }
      if (info.isDraft) {
        await eject(queue, pr, "it's a draft, which GitHub won't merge. Mark it ready for review and re-run `bun run merge`.");
        moved = true;
        continue;
      }
      if (info.baseRefName !== "main") {
        await eject(queue, pr, `it targets ${info.baseRefName}; the queue merges into main only.`);
        moved = true;
        continue;
      }
      if (info.isCrossRepository) {
        await eject(queue, pr, "it comes from a fork; the queue lands branches on origin only.");
        moved = true;
        continue;
      }
      const head = await readRemoteHead(gate, info.headRefName);
      if (!head.reachable) continue;
      if (head.sha === null) {
        await eject(queue, pr, `its branch ${info.headRefName} isn't on origin.`);
        moved = true;
        continue;
      }
      candidates.push({ pr, title: info.title, headRefName: info.headRefName, head: head.sha });
    }

    while (candidates.length > 0 && holding) {
      await git(gate, "fetch", "--quiet", "origin", "main", ...candidates.map((c) => c.headRefName));
      const mainSha = await git(gate, "rev-parse", "origin/main");
      const baseTree = await git(gate, "rev-parse", `${mainSha}^{tree}`);
      await resetGateCheckout(gate, mainSha);
      const { stack, ejected } = await buildStack(gate, mainSha, candidates);
      for (const { pr, reason } of ejected) await eject(queue, pr, reason);
      if (ejected.length > 0) moved = true;
      if (stack.length === 0) return moved;

      prs = stack.map((e) => e.pr);
      await renew();
      console.log(`\n▶ Merge gate on ${prs.map((p) => `#${p}`).join(", ")} (stacked on ${mainSha.slice(0, 8)}) in ${gate}`);
      const result = await runGate(gate, mainSha);
      if (!holding) {
        console.error("\n✗ This machine lost the queue's lock during the gate (another machine took it over); nothing landed.");
        return moved;
      }
      if (result.verdict === "infra") {
        console.error("\n✗ This machine couldn't run the gate (see above); no PR is blamed. Releasing the queue for another round.");
        return moved;
      }
      if (result.verdict === "passed") {
        const outcome = await landStack(stack, baseTree, realLander(gate, queue, repo, renew));
        if (outcome.stopped !== undefined) {
          const rest = stack.filter((e) => !outcome.landed.includes(e.pr) && e.pr !== outcome.ejected).map((e) => `#${e.pr}`);
          console.log(`\n⚠ Stopped landing: ${outcome.stopped}.${rest.length > 0 ? ` ${rest.join(", ")} stay queued for the next round.` : ""}`);
          if (outcome.landed.length > 0) console.log("  main holds part of a tested batch; the next round tests the rest on top of it.");
        }
        return moved || outcome.landed.length > 0 || outcome.ejected !== undefined;
      }
      if (stack.length === 1) {
        await eject(
          queue,
          stack[0].pr,
          `the merge gate failed on it, on top of main. Reproduce by merging or rebasing \`origin/main\` and running \`bun run test:changed\`, fix, push, and re-run \`bun run merge ${stack[0].pr}\`.\n\n${fenced(stripAnsi(result.tail))}`
        );
        return true;
      }
      candidates = bisectBatch(stack).map(({ pr, title, headRefName, head }) => ({ pr, title, headRefName, head }));
      console.log(`\n⟳ The batch failed; retesting ${candidates.map((c) => `#${c.pr}`).join(", ")} on their own.`);
    }
  } catch (err) {
    console.error(`\n✗ The round stopped on an error: ${err instanceof Error ? err.message : String(err)}`);
  } finally {
    clearInterval(heartbeat);
    await release();
    releaseHeldLock = null;
  }
  return moved;
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
  const { stack, ejected } = await buildStack(gate, mainSha, [{ pr, title: info.title, headRefName: info.headRefName, head }]);
  if (stack.length === 0) fail(`PR #${pr} can't be gated: ${ejected[0]?.reason ?? "nothing to test."}`);
  console.log(`\n▶ Merge gate on #${pr} (stacked on ${mainSha.slice(0, 8)}) in ${gate}`);
  const result = await runGate(gate, mainSha);
  if (result.verdict === "infra") fail("This machine couldn't run the gate (see above).");
  if (result.verdict === "failed") fail(`The merge gate failed on #${pr}, on top of main.`);
  console.log(`\n✓ Dry run: the merge gate passed on #${pr}. Nothing queued, pushed or merged.\n`);
}

/**
 * Whether `pr` has merged, asking a few times: GitHub's API can still say
 * OPEN for a few seconds after a merge, and a waiter that saw its PR leave
 * the queue mustn't report a fresh landing as a failure.
 */
async function hasMerged(pr: number): Promise<boolean> {
  for (let tries = 1; tries <= 5; tries++) {
    if ((await viewPr(pr))?.state === "MERGED") return true;
    await Bun.sleep(3000);
  }
  return false;
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
  if (info.isDraft) fail(`PR #${pr} is a draft, which GitHub won't merge. Mark it ready for review first.`);
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

  // Stopping waiting gives up the PR's place: a queued PR nobody is waiting on
  // would otherwise land later, unannounced. Registered before the PR is
  // queued, so no Ctrl-C can leave it queued behind.
  let settled = false;
  const giveUp = async (signal: "SIGINT" | "SIGTERM" | "SIGHUP") => {
    // Mid-round, the lock goes too, rather than make every waiter sit out the
    // stale window.
    await releaseHeldLock?.();
    if (!settled) await queue.dequeue(pr);
    process.kill(process.pid, signal);
  };
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"] as const) process.once(signal, () => void giveUp(signal));

  await queue.enqueue(pr, pushedHead);
  console.log(`\n▶ #${pr} queued. Whichever machine holds the queue's lock gates and lands it.`);

  const started = Date.now();
  const watch = new StaleWatch();
  let idleRounds = 0;
  const status = statusLogger((m) => console.log(m), Date.now);
  for (;;) {
    // One poll. Any error in it — a failed ls-remote, a gh hiccup — is
    // retried at the next poll rather than ending the wait.
    try {
      if ((await viewPr(pr))?.state === "MERGED") {
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
        if (await hasMerged(pr)) {
          console.log(`\n✓ PR #${pr} merged.\n`);
          return;
        }
        fail(`PR #${pr} left the queue without landing — the reason is in a comment on the PR.`);
      }
      if (Date.now() - started > MAX_WAIT_MS) {
        await queue.dequeue(pr);
        settled = true;
        fail(`PR #${pr} didn't land within ${formatDuration(MAX_WAIT_MS)}; it has left the queue. Re-run when the queue is shorter.`);
      }

      const sha = await queue.lockSha();
      const stale = watch.observe(sha);
      // This machine's gate checkout first, then the queue's lock. A local
      // --dry-run holding the checkout makes this machine sit the round out
      // — without waiting on it, so this waiter keeps watching its PR — rather
      // than hold up every machine while it waits.
      const local = readHolder(MERGE_LOCK_PATH);
      const checkoutBusy = local.state === "held" && isPidAlive(local.holder.pid);
      if ((sha === null || stale) && !checkoutBusy) {
        // The read above is only a hint — two waiters here can both see it
        // free — so the loser waits a few seconds, not a whole round.
        const checkout = await acquireGateLock({ lockPath: MERGE_LOCK_PATH, what: "gate checkout", maxWaitMs: WAITER_CHECKOUT_WAIT_MS, quietTimeout: true });
        let ran = false;
        let moved = false;
        try {
          if (checkout.held) {
            if (stale) {
              const holder = await queue.lockInfo(sha as string);
              console.log(`\n⚠ The queue's lock hasn't moved in 10 minutes${holder ? ` (${describeLock(holder)})` : ""}; taking it over.`);
            }
            const held = await queue.tryAcquire(lockInfoHere(), sha ?? "");
            if (held !== null) {
              ran = true;
              moved = await runRound(queue, held, repoRoot, repo);
            }
          }
        } finally {
          checkout.release();
        }
        if (ran) {
          // A round that got nowhere (this machine can't run the gate, say)
          // backs off before trying again, so it neither hammers origin and
          // GitHub nor keeps a healthy machine from taking the round.
          idleRounds = moved ? 0 : idleRounds + 1;
          if (idleRounds > 0) await Bun.sleep(Math.min(POLL_MS * 2 ** idleRounds, MAX_BACKOFF_MS));
          continue;
        }
      }
      const holder = sha === null ? null : await queue.lockInfo(sha);
      const queued = await queue.queued();
      status(
        `${sha ?? ""}:${queued.length}`,
        `  queued (#${pr}, ${queued.length} in the queue${holder ? `; running: ${describeLock(holder)}` : ""}) — waited ${formatDuration(Date.now() - started)}`
      );
    } catch (err) {
      console.log(`  (couldn't read the queue: ${err instanceof Error ? err.message : String(err)} — retrying)`);
    }
    await Bun.sleep(POLL_MS);
  }
}

if (import.meta.main) {
  await main();
}
