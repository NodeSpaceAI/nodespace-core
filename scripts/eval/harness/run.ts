#!/usr/bin/env bun
/**
 * Runs the harness scenario (./scenario.ts) in one harness and prints where
 * its logs are. The session's log is read with ./trace.ts.
 *
 *   bun scripts/eval/harness/run.ts claude-code        Claude Code with the plugin
 *   bun scripts/eval/harness/run.ts claude-code-block  Claude Code with the instructions block only
 *   bun scripts/eval/harness/run.ts opencode           OpenCode with the plugin
 *
 * It needs an isolated daemon already running, and the `nodespace` CLI built
 * from this checkout:
 *
 *   cargo build -p nodespace-cli -p nodespace-daemon
 *   NODESPACE_HOME=/tmp/ns-harness NODESPACED_SOCKET=/tmp/ns-harness/.nodespace/daemon.sock target/debug/nodespaced
 *
 * with the same two variables set for this script. A socket path has a short
 * length limit, so the home is a short path.
 */

import { EXIT_FAILED, EXIT_USAGE, EnvironmentError, abortOnEnvironment } from "../preflight.ts";
import { claudeCode } from "./claude-code.ts";
import { opencode } from "./opencode.ts";
import {
  type RunTasks,
  type ScenarioEnv,
  type Turn,
  ENV_USAGE,
  checkout,
  clearRun,
  fixture,
  logger,
  play,
  readEnv,
  seedRun,
} from "./scenario.ts";

const HARNESSES = ["claude-code", "claude-code-block", "opencode"] as const;
const harness = process.argv[2] as (typeof HARNESSES)[number] | undefined;

if (!harness || !HARNESSES.includes(harness)) {
  console.error(`Usage: bun scripts/eval/harness/run.ts <${HARNESSES.join("|")}>\n\n${ENV_USAGE}`);
  process.exit(EXIT_USAGE);
}

let env: ScenarioEnv;

try {
  env = readEnv();
} catch (err) {
  if (err instanceof EnvironmentError) {
    abortOnEnvironment("harness scenario", err);
  }

  throw err;
}

const log = logger(env, harness);
let tasks: RunTasks | null = null;
let cwd = "";

try {
  const held = fixture(env);
  const seeded = seedRun(env, held);

  tasks = seeded;
  cwd = checkout(env, harness);

  const scenario = (turn: Turn) => play(env, held, seeded, log, turn);

  if (harness === "opencode") {
    await opencode(env, cwd, log, scenario);
  } else {
    await claudeCode(env, harness === "claude-code" ? "plugin" : "block", cwd, log, scenario);
  }
} catch (err) {
  log.note(`STOPPED ${String(err)}`);
  console.error(String(err));
  process.exitCode = EXIT_FAILED;
} finally {
  if (tasks) {
    clearRun(env, tasks);
  }
}

console.log(`checkout  ${cwd}\nevents    ${log.events}\nsession   ${log.file}\n\nbun scripts/eval/harness/trace.ts ${log.file}`);
