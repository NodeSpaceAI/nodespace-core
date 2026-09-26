#!/usr/bin/env bun
// Decides which stages of the pre-push gate (scripts/test-gate.ts, ADR-047)
// a push can affect.
//
// The gate used to run the whole pyramid on every push, so a one-line Svelte
// change paid for compiling and testing the entire Rust workspace. Scoping is
// safe only if it is conservative, so the rule runs one way: a stage is
// skipped only when no changed file can reach it. A file this module does not
// recognize, or a change to the gate's own machinery, runs everything.
//
// "Changed" is the diff from the merge-base with origin/main to the working
// tree — every commit this branch would push plus anything uncommitted, since
// the tests run against the working tree, not against HEAD.

import { $ } from "bun";

export const FULL_ENV_VAR = "NODESPACE_GATE_FULL";

export interface GateScope {
  /** Why everything runs, when it does; null for a scoped run. */
  fullReason: string | null;
  /** Happy-DOM unit tests and the Chromium browser tier. */
  frontend: boolean;
  /** The Rust workspace's tests (nextest) and nodespace-app's unit tests. */
  rust: boolean;
  /** The skill package's tests. */
  skill: boolean;
  /** Tests of the tooling under scripts/. */
  scripts: boolean;
}

const ALL: Omit<GateScope, "fullReason"> = { frontend: true, rust: true, skill: true, scripts: true };

/** Every stage, for the merge gate, which never scopes. */
export const FULL_SCOPE: GateScope = { fullReason: "merge gate", ...ALL };

/** Crates of the Rust workspace, whole directories: fixtures and SQL count. */
const RUST_DIRS = [
  "packages/core/",
  "packages/agent/",
  "packages/daemon/",
  "packages/cli/",
  "packages/nlp-engine/",
  "packages/proto/",
  "packages/nodespace-types/",
  "packages/desktop-app/src-tauri/",
];

/** Workspace-wide Rust inputs outside any one crate. */
const RUST_ROOT_FILES = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rust-toolchain"];

/**
 * Tooling fixtures Rust tests read: the eval golden files under scripts/ are
 * asserted by packages/agent's golden tests, so a change there must run Rust.
 */
const RUST_READ_SCRIPT_DIRS = ["scripts/eval/"];

/**
 * The gate's own machinery. A change here can alter what any stage does, so
 * it is never allowed to scope itself down.
 */
const GATE_FILES = [
  "scripts/test-gate.ts",
  "scripts/gate-scope.ts",
  "scripts/gate-lock.ts",
  "scripts/test-app-units.ts",
  "scripts/setup-rust-tooling.ts",
  "scripts/merge-pr.ts",
  ".husky/",
  "package.json",
  "bun.lock",
];

/** Prose and agent configuration: no stage reads them. */
function isInert(file: string): boolean {
  if (file.startsWith("packages/skill/")) return false; // SKILL.md is checked
  return file.endsWith(".md") || file.startsWith(".claude/") || file === ".gitignore";
}

/** Maps changed files to the stages they can affect. Pure, for testing. */
export function classify(files: string[]): GateScope {
  if (files.length === 0) {
    // Nothing differs from main — e.g. pushing a fresh branch. Scoping an
    // empty diff would skip everything; run it all instead.
    return { fullReason: "no changes against origin/main", ...ALL };
  }
  const scope: GateScope = { fullReason: null, frontend: false, rust: false, skill: false, scripts: false };
  for (const file of files) {
    const gateFile = GATE_FILES.find((g) => (g.endsWith("/") ? file.startsWith(g) : file === g));
    if (gateFile !== undefined) {
      return { fullReason: `the gate's own machinery changed (${file})`, ...ALL };
    }
    if (isInert(file)) continue;
    if (RUST_ROOT_FILES.includes(file) || file.startsWith(".cargo/") || RUST_DIRS.some((d) => file.startsWith(d))) {
      scope.rust = true;
      continue;
    }
    if (file.startsWith("packages/skill/")) {
      // The CLI's SKILL.md generation tests and the agent's skill rules read
      // these files, so a skill change reaches Rust too.
      scope.skill = true;
      scope.rust = true;
      continue;
    }
    if (file.startsWith("scripts/")) {
      scope.scripts = true;
      if (RUST_READ_SCRIPT_DIRS.some((d) => file.startsWith(d))) scope.rust = true;
      continue;
    }
    if (file.startsWith("packages/desktop-app/") || file.startsWith("packages/dev-tools/")) {
      scope.frontend = true;
      continue;
    }
    return { fullReason: `unrecognized path (${file})`, ...ALL };
  }
  // Some scripts tests read Rust-side outputs (the eval golden fixtures are
  // written by the agent's golden tests). They take seconds, so any Rust
  // change runs them rather than tracking each coupling by hand.
  if (scope.rust) scope.scripts = true;
  return scope;
}

/** Files changed from the merge-base with origin/main to the working tree. */
async function changedFiles(): Promise<string[]> {
  const base = (await $`git merge-base origin/main HEAD`.quiet().text()).trim();
  // --no-renames: a rename is listed by its destination alone otherwise, and
  // the path it left can matter as much as the one it arrived at.
  const committed = await $`git diff --name-only --no-renames ${base} HEAD`.quiet().text();
  const uncommitted = await $`git diff --name-only --no-renames HEAD`.quiet().text();
  const untracked = await $`git ls-files --others --exclude-standard`.quiet().text();
  const all = `${committed}\n${uncommitted}\n${untracked}`.split("\n").map((f) => f.trim());
  return [...new Set(all.filter((f) => f !== ""))];
}

/** The scope for this push. Anything that stops it from being computed runs everything. */
export async function gateScope(): Promise<GateScope> {
  if (process.env[FULL_ENV_VAR]) return { fullReason: `${FULL_ENV_VAR} is set`, ...ALL };
  try {
    return classify(await changedFiles());
  } catch (err) {
    return { fullReason: `could not diff against origin/main (${err instanceof Error ? err.message : String(err)})`, ...ALL };
  }
}

export function describeScope(scope: GateScope): string {
  if (scope.fullReason !== null) return `every tier — ${scope.fullReason}`;
  const on = (Object.keys(ALL) as (keyof typeof ALL)[]).filter((k) => scope[k]);
  const off = (Object.keys(ALL) as (keyof typeof ALL)[]).filter((k) => !scope[k]);
  return `scoped to this push's changes — running: ${on.join(", ") || "lint only"}; skipping: ${off.join(", ") || "nothing"} (${FULL_ENV_VAR}=1 runs everything)`;
}
