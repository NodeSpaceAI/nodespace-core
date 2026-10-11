/**
 * The harness scenario (ADR-093 §5, §6): what a coding agent receives from
 * NodeSpace in one harness, observed from a real session.
 *
 * One session in a checkout of a project is sent four prompts, and the graph
 * is changed twice from outside it:
 *
 *   1. `probe`    with no command allowed, name the project and the
 *                 procedures available: what reached the agent unasked.
 *   2. `first`    take the next ready task and work it to review.
 *      outside:   the standard attached to the project is edited.
 *   3. `changed`  with no command allowed, say what changed: whether the
 *                 edit was announced.
 *   4. `second`   take the next ready task. Its checklist runs an 80 second
 *                 command, which is longer than the item watch's interval.
 *      outside:   once the session has started that task, it is finished
 *                 from elsewhere: whether the session stops.
 *
 * Another project's task, more urgent than either, sits in the same queue
 * throughout: a queue run that is not narrowed to the project returns it.
 *
 * The daemon is the caller's, as in the other evals: this starts nothing. It
 * refuses a daemon whose home is the user's own, since every run writes.
 */

import { appendFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO_ROOT } from "../env.ts";

export const PROMPTS = {
  probe:
    'Without running any command or reading any file: which project does this checkout belong to, and which named procedures or standards are available to you for working in it? Say "unknown" for anything you cannot tell from what you were given.',
  first:
    "Pick up the next ready task for this project and work it until it is in review. This checkout is a local fixture: commit locally, do not push, and there is no pull request to record.",
  changed:
    "Before the next one: has anything about how work is done here changed since this session started? Answer from what you have been told so far; run no command.",
  second: "Pick up the next ready task for this project and work it until it is in review, the same way.",
} as const;

export type PromptName = keyof typeof PROMPTS;

/** The fixture checkout's remote, in the scp form: the project holds the HTTPS one. */
const REMOTE = "git@github.com:nodespace-fixture/widget.git";
const REPOSITORY_URL = "https://github.com/nodespace-fixture/widget";

export const ENV_USAGE = `Environment:
  NODESPACE_HOME        The isolated daemon's home (required; never the user's own)
  NODESPACED_SOCKET     The isolated daemon's socket (required)
  NS_BIN                Path to the nodespace CLI (default: target/debug/nodespace)
  HARNESS_SCENARIO_DIR  Where checkouts, installs and logs go (default: /tmp/nodespace-harness-scenario)
  HARNESS_MODEL         Model to ask the harness for (default: the harness's own)`;

export interface ScenarioEnv {
  /** Path to the `nodespace` CLI the session and this script both run. */
  nsBin: string;
  /** Where checkouts, installs and logs go. */
  dir: string;
  model: string | undefined;
}

export function readEnv(): ScenarioEnv {
  const home = process.env.NODESPACE_HOME ?? "";
  const socket = process.env.NODESPACED_SOCKET ?? "";

  if (home === "" || socket === "") {
    throw new Error(`NODESPACE_HOME and NODESPACED_SOCKET must name an isolated daemon.\n${ENV_USAGE}`);
  }

  if (home === (process.env.HOME ?? "") || socket.startsWith(`${process.env.HOME ?? ""}/.nodespace/`)) {
    throw new Error("Refusing to run against the user's own NodeSpace: every run writes tasks and skills.");
  }

  return {
    nsBin: process.env.NS_BIN ?? join(REPO_ROOT, "target/debug/nodespace"),
    dir: process.env.HARNESS_SCENARIO_DIR ?? "/tmp/nodespace-harness-scenario",
    model: process.env.HARNESS_MODEL || undefined,
  };
}

/**
 * The environment of a harness session: the caller's, with a folder first on
 * the path that holds `nodespace` as the CLI under test.
 */
export function sessionEnv(env: ScenarioEnv, extra: Record<string, string> = {}): Record<string, string> {
  const bin = join(env.dir, "bin");

  mkdirSync(bin, { recursive: true });
  rmSync(join(bin, "nodespace"), { force: true });
  writeFileSync(join(bin, "nodespace"), `#!/bin/sh\nexec "${env.nsBin}" "$@"\n`, { mode: 0o755 });

  return { ...(process.env as Record<string, string>), PATH: `${bin}:${process.env.PATH ?? ""}`, ...extra };
}

type Json = Record<string, unknown>;

function run(argv: string[], cwd?: string, extraEnv: Record<string, string> = {}): { ok: boolean; out: string; err: string } {
  const ran = Bun.spawnSync(argv, { cwd, env: { ...(process.env as Record<string, string>), ...extraEnv } });

  return { ok: ran.exitCode === 0, out: ran.stdout.toString(), err: ran.stderr.toString() };
}

function must(argv: string[], cwd?: string, extraEnv: Record<string, string> = {}): string {
  const ran = run(argv, cwd, extraEnv);

  if (!ran.ok) {
    throw new Error(`${argv.join(" ")}\n${ran.out}\n${ran.err}`);
  }

  return ran.out;
}

/**
 * `nodespace --json <args>` from outside the session. No journal names these
 * writes, so a session's plugin reads them as someone else's.
 */
export function ns(env: ScenarioEnv, ...args: string[]): { ok: boolean; json: Json; raw: string } {
  const ran = run([env.nsBin, "--json", ...args]);
  let json: Json = {};

  try {
    json = JSON.parse(ran.out) as Json;
  } catch {
    // Not JSON: `raw` holds what it printed.
  }

  return { ok: ran.ok, json, raw: ran.out + ran.err };
}

function created(env: ScenarioEnv, ...args: string[]): string {
  const made = ns(env, "node", "create", ...args);

  if (!made.ok || typeof made.json.id !== "string") {
    throw new Error(`node create ${args.join(" ")}\n${made.raw}`);
  }

  return made.json.id;
}

/** What the fixture holds, kept between runs against one database. */
interface Fixture {
  project: string;
  standard: string;
  standardBody: string;
}

const STANDARD_USE_FOR =
  "Commit a change to the Widget repository: how the commit message is written. Use before any git commit in the widget checkout.";

function standardText(checker: string): string {
  return `Every commit subject in the Widget repository starts with \`widget:\` and is written in lower case. The commit body ends with the line \`Checked-by: ${checker}\`. One commit per task.`;
}

/**
 * The project, the standard attached to it and the other project's task:
 * made once per database, and found again on later runs.
 */
export function fixture(env: ScenarioEnv): Fixture {
  const file = join(env.dir, "fixture.json");

  if (existsSync(file)) {
    const held = JSON.parse(readFileSync(file, "utf8")) as Fixture;

    if (ns(env, "node", "get", held.project).ok) {
      return held;
    }
  }

  const project = created(
    env, "--type", "project", "--content", "Widget",
    "--properties", JSON.stringify({ status: "active", repository: { title: "widget", url: REPOSITORY_URL } }),
  );
  const standard = created(
    env, "--type", "skill", "--content", "Widget Commit Standard",
    "--properties", JSON.stringify({ use_for: STANDARD_USE_FOR }),
  );
  const standardBody = created(env, "--type", "text", "--parent", standard, "--content", standardText("heron"));

  ns(env, "relationship", "create", "--from", standard, "--type", "attached_to", "--to", project);

  const other = created(
    env, "--type", "project", "--content", "Gadget",
    "--properties", JSON.stringify({ status: "active", repository: { title: "gadget", url: "https://github.com/nodespace-fixture/gadget" } }),
  );
  const urgent = created(
    env, "--type", "task", "--content", "Rotate the gadget signing key",
    "--properties", JSON.stringify({ status: "open", priority: "highest" }),
  );

  created(env, "--type", "checkbox", "--parent", urgent, "--content", "- [ ] The new key is in the gadget vault");
  ns(env, "relationship", "create", "--from", other, "--type", "tasks", "--to", urgent);

  const made = { project, standard, standardBody };

  mkdirSync(env.dir, { recursive: true });
  writeFileSync(file, JSON.stringify(made, null, 2));

  return made;
}

/** The two tasks of one run, ready in the fixture's project, and the standard as it starts. */
export function seedRun(env: ScenarioEnv, held: Fixture): { first: string; second: string } {
  ns(env, "node", "update", held.standardBody, "--content", standardText("heron"));
  ns(env, "node", "update", held.standard, "--properties", JSON.stringify({ use_for: STANDARD_USE_FOR }));

  const task = (title: string, priority: string, items: string[]): string => {
    const id = created(env, "--type", "task", "--content", title, "--properties", JSON.stringify({ status: "open", priority }));

    for (const item of items) {
      created(env, "--type", "checkbox", "--parent", id, "--content", `- [ ] ${item}`);
    }

    ns(env, "relationship", "create", "--from", held.project, "--type", "tasks", "--to", id);

    return id;
  };

  return {
    first: task("slugify collapses repeated dashes", "high", [
      '`slugify("a  --  b")` returns `a-b`',
      "`node test.js` passes and has a case for repeated dashes",
    ]),
    second: task("Add a soak check to the README", "medium", [
      "`sh soak.sh` exits 0 and prints `soak ok`",
      'README.md has a "Soak check" section naming `sh soak.sh`',
    ]),
  };
}

/** A run's tasks that are still open or in progress are cancelled, so the next run's are the next ready ones. */
export function clearRun(env: ScenarioEnv, tasks: { first: string; second: string }): void {
  for (const id of [tasks.first, tasks.second]) {
    const status = (ns(env, "node", "get", id).json.properties as Json | undefined)?.status;

    if (status === "open" || status === "in_progress") {
      ns(env, "node", "set-status", id, "cancelled");
    }
  }
}

/** A fresh checkout of the fixture repository for one run; answers its path. */
export function checkout(env: ScenarioEnv, label: string): string {
  const dir = join(env.dir, "checkouts", label, "widget");
  const identity = { GIT_AUTHOR_NAME: "fixture", GIT_AUTHOR_EMAIL: "fixture@example.com", GIT_COMMITTER_NAME: "fixture", GIT_COMMITTER_EMAIL: "fixture@example.com" };

  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "README.md"), "# widget\n\nA slug helper.\n");
  writeFileSync(
    join(dir, "slug.js"),
    "function slugify(text) {\n  return text.trim().toLowerCase().replace(/[^a-z0-9-]+/g, '-')\n}\n\nmodule.exports = { slugify }\n",
  );
  writeFileSync(
    join(dir, "test.js"),
    "const assert = require('node:assert')\nconst { slugify } = require('./slug')\n\nassert.strictEqual(slugify('Hello World'), 'hello-world')\nconsole.log('ok')\n",
  );
  // Slow on purpose: the item watch compares at most once in 60 seconds, and
  // the tool call after this one is the first that can notice the task moved.
  writeFileSync(join(dir, "soak.sh"), '#!/bin/sh\nsleep 80\necho "soak ok"\n');
  must(["git", "init", "-q", "-b", "main"], dir);
  must(["git", "remote", "add", "origin", REMOTE], dir);
  must(["git", "add", "."], dir);
  must(["git", "commit", "-q", "-m", "widget: initial"], dir, identity);

  return dir;
}

/** One run's log of what the driver did and what the session answered. */
export function logger(env: ScenarioEnv, label: string): { file: string; events: string; note(line: string): void; raw(text: string): void } {
  const logs = join(env.dir, "logs");
  const file = join(logs, `${label}.jsonl`);
  const events = join(logs, `${label}.events.log`);

  mkdirSync(logs, { recursive: true });
  writeFileSync(file, "");
  writeFileSync(events, "");

  return {
    file,
    events,
    note: line => appendFileSync(events, `${new Date().toISOString()} ${line}\n`),
    raw: text => appendFileSync(file, text),
  };
}

/** Between the first task and the second: the project's standard is edited by someone else. */
export function editStandard(env: ScenarioEnv, held: Fixture): boolean {
  const body = ns(env, "node", "update", held.standardBody, "--content", standardText("osprey"));
  const listed = ns(
    env, "node", "update", held.standard,
    "--properties", JSON.stringify({ use_for: `${STANDARD_USE_FOR.replace(". Use", ", and that osprey checks it. Use")}` }),
  );

  return body.ok && listed.ok;
}

/**
 * During the second task: once the session has started it, the task is
 * finished from elsewhere. Answers what was done, or `null` when the session
 * ended without starting the task.
 */
export async function finishElsewhere(env: ScenarioEnv, taskId: string, isOver: () => boolean): Promise<string | null> {
  while (!isOver()) {
    const status = (ns(env, "node", "get", taskId).json.properties as Json | undefined)?.status;

    if (status === "in_progress") {
      const node = ns(env, "node", "context", taskId).json.node as Json | undefined;
      const boxes = (Array.isArray(node?.checkboxes) ? node.checkboxes : []) as Json[];
      let isFinished = true;

      for (const box of boxes) {
        const text = String(box.content ?? "").replace(/^- \[.\] /, "");

        isFinished &&= ns(env, "node", "update", String(box.id), "--content", `- [x] ${text}`).ok;
      }

      isFinished &&= ns(env, "node", "set-status", taskId, "in_review").ok;
      isFinished &&= ns(env, "node", "set-status", taskId, "done").ok;

      if (isFinished) {
        return `ticked ${boxes.length}, in_review, done`;
      }

      return ns(env, "node", "set-status", taskId, "cancelled").ok ? "cancelled" : "could not be finished or cancelled";
    }

    await Bun.sleep(1000);
  }

  return null;
}

/** What a driver is given: one prompt at a time, answered with what the session said. */
export type Turn = (name: PromptName) => Promise<string>;

/** The four prompts and the two changes from outside, in order, through one session's `turn`. */
export async function play(
  env: ScenarioEnv,
  held: Fixture,
  tasks: { first: string; second: string },
  log: { note(line: string): void },
  turn: Turn,
): Promise<void> {
  const ask = async (name: PromptName) => {
    log.note(`PROMPT ${name}`);
    log.note(`ANSWER ${name}: ${(await turn(name)).slice(0, 3000)}`);
  };
  let isOver = false;

  await ask("probe");
  await ask("first");
  log.note(`OUTSIDE standard edited: ${editStandard(env, held)}`);
  await ask("changed");

  const finishing = finishElsewhere(env, tasks.second, () => isOver);

  try {
    await ask("second");
  } finally {
    isOver = true;
  }

  log.note(`OUTSIDE second task: ${(await finishing) ?? "never started by the session"}`);
}

/**
 * Runs the skill installer from this checkout with one harness's home moved
 * into the scenario folder, so the user's own configuration is not touched.
 */
export function installSkill(agent: string, variable: string, home: string): void {
  mkdirSync(home, { recursive: true });
  must(["bun", join(REPO_ROOT, "packages/skill/src/install.ts"), "install", agent], REPO_ROOT, { [variable]: home });
}
