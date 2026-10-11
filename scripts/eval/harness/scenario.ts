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
 * asks the daemon which database it serves and refuses the user's own, since
 * every run writes and then hands an agent a queue to work.
 */

import { appendFileSync, existsSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { REPO_ROOT } from "../env.ts";
import { EnvironmentError, readServedDatabasePath } from "../preflight.ts";

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

const ISOLATE =
  "Start a daemon of its own for this, with both variables set for the daemon and for this script:\n" +
  "    NODESPACE_HOME=/tmp/ns-harness NODESPACED_SOCKET=/tmp/ns-harness/.nodespace/daemon.sock target/debug/nodespaced";

export interface ScenarioEnv {
  /** Path to the `nodespace` CLI the session and this script both run. */
  nsBin: string;
  /** The isolated daemon's socket. */
  socket: string;
  /** Where checkouts, installs and logs go. */
  dir: string;
  model: string | undefined;
}

/**
 * A path with its links resolved. The part of it that does not exist yet (a
 * socket not made, a database not created) is kept under the resolved part
 * that does.
 */
function resolved(path: string): string {
  const absolute = resolve(path);

  try {
    return realpathSync(absolute);
  } catch {
    const parent = dirname(absolute);

    return parent === absolute ? absolute : join(resolved(parent), basename(absolute));
  }
}

/**
 * Whether `path` is the user's own NodeSpace folder or inside it, whichever
 * way either is spelled.
 */
export function isUsersOwn(path: string, userHome: string): boolean {
  const own = resolved(join(userHome, ".nodespace"));
  const given = resolved(path);

  return userHome !== "" && (given === own || given.startsWith(`${own}/`));
}

export function readEnv(): ScenarioEnv {
  const home = process.env.NODESPACE_HOME ?? "";
  const socket = process.env.NODESPACED_SOCKET ?? "";
  const userHome = process.env.HOME ?? "";

  if (home === "" || socket === "") {
    throw new EnvironmentError("NODESPACE_HOME and NODESPACED_SOCKET must name an isolated daemon.", ISOLATE);
  }

  // A fast first refusal. The session's plugin and CLI keep the write journal
  // under NODESPACE_HOME, so that has to be the isolated one too.
  if (isUsersOwn(join(home, ".nodespace"), userHome) || isUsersOwn(socket, userHome)) {
    throw new EnvironmentError("NODESPACE_HOME or NODESPACED_SOCKET names the user's own NodeSpace.", ISOLATE);
  }

  const nsBin = process.env.NS_BIN ?? join(REPO_ROOT, "target/debug/nodespace");
  // The variables say where this script looks, not what the daemon there
  // serves: only the daemon can say that. Throws when it cannot be read.
  const served = readServedDatabasePath({ nsBin, socket });

  if (isUsersOwn(served, userHome)) {
    throw new EnvironmentError(
      `The daemon at ${socket} serves the user's own database (${served}). Every run writes projects, tasks and a skill.`,
      ISOLATE,
    );
  }

  return {
    nsBin,
    socket,
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

  return {
    ...(process.env as Record<string, string>),
    PATH: `${bin}:${process.env.PATH ?? ""}`,
    NODESPACED_SOCKET: env.socket,
    ...extra,
  };
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
  const ran = run([env.nsBin, "--socket", env.socket, "--json", ...args]);
  let json: Json = {};

  try {
    json = JSON.parse(ran.out) as Json;
  } catch {
    // Not JSON: `raw` holds what it printed.
  }

  return { ok: ran.ok, json, raw: ran.out + ran.err };
}

/** A write the fixture depends on: a run built on one that failed would measure nothing. */
function mustNs(env: ScenarioEnv, ...args: string[]): Json {
  const ran = ns(env, ...args);

  if (!ran.ok) {
    throw new Error(`nodespace ${args.join(" ")}\n${ran.raw}`);
  }

  return ran.json;
}

function created(env: ScenarioEnv, ...args: string[]): string {
  const id = mustNs(env, "node", "create", ...args).id;

  if (typeof id !== "string") {
    throw new Error(`node create ${args.join(" ")} answered no id`);
  }

  return id;
}

function statusOf(env: ScenarioEnv, id: string): string {
  return String((ns(env, "node", "get", id).json.properties as Json | undefined)?.status ?? "unknown");
}

/** What the fixture holds, kept between runs against one database. */
export interface Fixture {
  project: string;
  standard: string;
  standardBody: string;
  /** The other project, whose task a queue run that is not narrowed returns first. */
  otherProject: string;
}

/** The tasks of one run. */
export interface RunTasks {
  first: string;
  second: string;
  /** The other project's task, more urgent than either. */
  other: string;
}

const STANDARD_USE_FOR =
  "Commit a change to the Widget repository: how the commit message is written. Use before any git commit in the widget checkout.";

function standardText(checker: string): string {
  return `Every commit subject in the Widget repository starts with \`widget:\` and is written in lower case. The commit body ends with the line \`Checked-by: ${checker}\`. One commit per task.`;
}

/**
 * The project, the standard attached to it and the other project: made once
 * per database, and found again on later runs.
 */
export function fixture(env: ScenarioEnv): Fixture {
  const file = join(env.dir, "fixture.json");

  if (existsSync(file)) {
    const held = JSON.parse(readFileSync(file, "utf8")) as Fixture;

    if (ns(env, "node", "get", held.project).ok && ns(env, "node", "get", held.otherProject).ok) {
      return held;
    }
  }

  const project = created(
    env, "--type", "software-project", "--content", "Widget",
    "--properties", JSON.stringify({ status: "active", repository: { title: "widget", url: REPOSITORY_URL } }),
  );
  const standard = created(
    env, "--type", "skill", "--content", "Widget Commit Standard",
    "--properties", JSON.stringify({ use_for: STANDARD_USE_FOR }),
  );
  const standardBody = created(env, "--type", "text", "--parent", standard, "--content", standardText("heron"));

  mustNs(env, "relationship", "create", "--from", standard, "--type", "attached_to", "--to", project);

  const otherProject = created(
    env, "--type", "software-project", "--content", "Gadget",
    "--properties", JSON.stringify({ status: "active", repository: { title: "gadget", url: "https://github.com/nodespace-fixture/gadget" } }),
  );
  const made = { project, standard, standardBody, otherProject };

  mkdirSync(env.dir, { recursive: true });
  writeFileSync(file, JSON.stringify(made, null, 2));

  return made;
}

/**
 * One run's tasks, all ready, and the standard as it starts. The other
 * project's task is made for each run: a session that took an earlier one
 * would leave the next run with nothing to be handed by mistake.
 */
export function seedRun(env: ScenarioEnv, held: Fixture): RunTasks {
  mustNs(env, "node", "update", held.standardBody, "--content", standardText("heron"));
  mustNs(env, "node", "update", held.standard, "--properties", JSON.stringify({ use_for: STANDARD_USE_FOR }));

  const task = (project: string, title: string, priority: string, items: string[]): string => {
    const id = created(env, "--type", "task", "--content", title, "--properties", JSON.stringify({ status: "open", priority }));

    for (const item of items) {
      created(env, "--type", "checkbox", "--parent", id, "--content", `- [ ] ${item}`);
    }

    mustNs(env, "relationship", "create", "--from", project, "--type", "tasks", "--to", id);

    return id;
  };

  return {
    other: task(held.otherProject, "Rotate the gadget signing key", "highest", ["The new key is in the gadget vault"]),
    first: task(held.project, "slugify collapses repeated dashes", "high", [
      '`slugify("a  --  b")` returns `a-b`',
      "`node test.js` passes and has a case for repeated dashes",
    ]),
    second: task(held.project, "Add a soak check to the README", "medium", [
      "`sh soak.sh` exits 0 and prints `soak ok`",
      'README.md has a "Soak check" section naming `sh soak.sh`',
    ]),
  };
}

/** A run's tasks that are still open or in progress are cancelled, so the next run's are the next ready ones. */
export function clearRun(env: ScenarioEnv, tasks: RunTasks): void {
  for (const id of [tasks.first, tasks.second, tasks.other]) {
    const status = statusOf(env, id);

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
  // The remote is there for project resolution to read. Nothing is pushed to
  // it: the prompt says so, and this makes it so.
  must(["git", "remote", "set-url", "--push", "origin", "DISABLED"], dir);
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
    "--properties", JSON.stringify({ use_for: STANDARD_USE_FOR.replace(". Use", ", and that osprey checks it. Use") }),
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
    if (statusOf(env, taskId) === "in_progress") {
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
  tasks: RunTasks,
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
  // Anything but `open` means the session took another project's work.
  log.note(`OTHER PROJECT'S TASK is ${statusOf(env, tasks.other)}`);
}

/**
 * Runs the skill installer from this checkout with one harness's home moved
 * into the scenario folder, so the user's own configuration is not touched.
 */
export function installSkill(agent: string, variable: string, home: string): void {
  mkdirSync(home, { recursive: true });
  must(["bun", join(REPO_ROOT, "packages/skill/src/install.ts"), "install", agent], REPO_ROOT, { [variable]: home });
}
