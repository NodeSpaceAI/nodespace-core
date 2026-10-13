/**
 * Tasks eval — scores whether an agent that has the task procedures follows
 * them: Creating a Task and Answering a Question.
 *
 * The retrieval evals measure which skill is found. This one measures what an
 * agent that has it does with it, graded on what the graph holds afterwards
 * rather than on how the reply is worded:
 *
 *   - a bug the agent can see is created as a ready task: a brief, a checklist
 *     and its project, with no open question or blocker in front of it;
 *   - a bug the agent cannot see is left unwritten;
 *   - a request an accepted decision declines creates nothing;
 *   - a question that stops work becomes a task assigned to the person who owes
 *     the answer and blocking the stopped task, which keeps its status;
 *   - a question waiting on the user is put to the user, never answered by the
 *     agent, so nothing is recorded as their answer.
 *
 * The workspace is the entity tier: a project, three people (one of them the
 * database's owner, who "me" is), tasks assigned through `person.tasks`, and a
 * decision. The daemon seeds the queues and the skills itself, so the fixture
 * adds instance data only.
 *
 * Several repetitions share one database unless the run purges between them
 * (`--between-runs`); the seed is idempotent and puts every seeded status back,
 * but a task one repetition created stays, so run `--runs N` with a purge.
 */

import type { EvalEnv } from "../env.ts";
import type { GraphDiff, SnapshotNode } from "../graph.ts";
import { assertEndState } from "../end-state.ts";
import type { EvalFixture, Scenario, TurnRecord, Verdict } from "../types.ts";
import {
  bareId,
  ensureEdge,
  findByTitle,
  listType,
  ownerId,
  runNs,
} from "./seed-helpers.ts";

const PROJECT_TITLE = "Harbor relaunch";
const OWNER = { first: "Mara", last: "Quint", email: "mara.quint@example.com" };
const ANOOP = { first: "Anoop", last: "Nair" };
const INGRID = { first: "Ingrid", last: "Solberg" };

const STOPPED_TASK = "Draft the vendor contract";
const REVIEW_TASK = "Review the pricing page";
const DONE_TASK = "Send the launch invite";
const DONE_TASK_BOX = "- [ ] The invite list is signed off";
const DECLINED_TITLE = "Offline mode for the mobile app will not be built";
const WAITING_QUESTION = "Which pricing tier should the page lead with?";
/** What a reply that found the waiting question names, however it words it. */
const WAITING_QUESTION_SUBJECT = /pricing\s+tier|lead\s+with/i;

/** Relations the scenarios assert beyond the universal four. */
const RELATIONS = ["assignee", "project", "blocks", "blocked_by"];

type Expected =
  | { kind: "ready-task" }
  | { kind: "no-writes" }
  | { kind: "question-task" }
  | { kind: "asks-user" };

interface TaskScenario extends Scenario {
  expected: Expected;
}

const FIXTURES: TaskScenario[] = [
  {
    id: "bug-created-ready",
    scenario: "A bug the agent can see becomes a ready task, with its project",
    prompt:
      "Something is off in Harbor relaunch: Send the launch invite is marked done " +
      "but one box on its checklist is still unticked. Get that tracked as work.",
    expected: { kind: "ready-task" },
  },
  {
    id: "bug-not-reproducible",
    scenario: "A bug the agent cannot see is reported, and nothing is written",
    prompt:
      "Bug in Harbor relaunch: Review the pricing page has no assignee, so nobody " +
      "ever sees it. Log it.",
    expected: { kind: "no-writes" },
  },
  {
    id: "declined-request",
    scenario: "A request an accepted decision declines creates no task",
    prompt: "Add a task to build offline mode for the mobile app.",
    expected: { kind: "no-writes" },
  },
  {
    id: "question-blocks-task",
    scenario: "A question that stops work is a task for its owner, blocking the stopped task",
    prompt:
      "I can't carry on with Draft the vendor contract until Anoop says whether " +
      "the contract runs one year or three. Put his question down so it waits for " +
      "him, not for me.",
    expected: { kind: "question-task" },
  },
  {
    id: "answer-asks-user",
    scenario: "A question waiting on the user is put to them, never answered by the agent",
    prompt: "Take the question that's waiting on me and get it answered.",
    expected: { kind: "asks-user" },
  },
];

/** What the seed made, which the pure scoring below reads ids from. */
export interface SeededIds {
  project: string;
  anoop: string;
  stopped: string;
  done: string;
  review: string;
  decision: string;
  question: string;
}

let seeded: SeededIds | null = null;

/** Set the ids scoring compares against. The seed does it; tests do it directly. */
export function useSeededIds(ids: SeededIds): void {
  seeded = ids;
}

function personId(env: EvalEnv, p: { first: string; last: string }): string {
  const full = `${p.first} ${p.last}`;
  const present = findByTitle(env, "person", full);
  if (present) return present;
  const created = runNs(env, [
    "node",
    "create",
    "--type",
    "person",
    "--properties",
    JSON.stringify({ first_name: p.first, last_name: p.last }),
  ]) as { id?: string } | null;
  if (!created?.id) throw new Error(`seeding person '${full}' returned no id`);
  return bareId(created.id);
}

/** A node of `type` with this title, created when absent, its properties reset. */
function ensureNode(
  env: EvalEnv,
  type: string,
  title: string,
  properties: Record<string, unknown>,
): string {
  const present = findByTitle(env, type, title);
  if (present) {
    if (Object.keys(properties).length > 0) {
      runNs(env, ["node", "update", present, "--properties", JSON.stringify(properties)]);
    }
    return present;
  }
  const args = ["node", "create", "--type", type, "--content", title];
  if (Object.keys(properties).length > 0) {
    args.push("--properties", JSON.stringify(properties));
  }
  const created = runNs(env, args) as { id?: string } | null;
  if (!created?.id) throw new Error(`seeding ${type} '${title}' returned no id`);
  return bareId(created.id);
}

/**
 * Create the workspace: people, a project, tasks and the links between them, a
 * done task with an unticked box, an accepted decision, and a question task
 * waiting on the owner.
 *
 * Idempotent: a node present by title is reused and its seeded status put
 * back, and an edge that exists already is not an error. It runs before every
 * chat, so a database a run left behind and a freshly purged one end up alike.
 */
export function seedWorkspace(env: EvalEnv): SeededIds {
  const owner = ownerId(env);
  runNs(env, [
    "node",
    "update",
    owner,
    "--properties",
    JSON.stringify({ first_name: OWNER.first, last_name: OWNER.last, email: OWNER.email }),
  ]);
  const anoop = personId(env, ANOOP);
  const ingrid = personId(env, INGRID);

  const project = ensureNode(env, "project", PROJECT_TITLE, {});

  const stopped = ensureNode(env, "task", STOPPED_TASK, { status: "open" });
  const review = ensureNode(env, "task", REVIEW_TASK, { status: "in_progress" });
  const done = ensureNode(env, "task", DONE_TASK, { status: "done" });
  const question = ensureNode(env, "task", WAITING_QUESTION, { status: "open" });

  const box = listType(env, "checkbox").find(
    (n) => (n.content ?? n.title ?? "") === DONE_TASK_BOX,
  );
  if (!box) {
    runNs(env, ["node", "create", "--type", "checkbox", "--content", DONE_TASK_BOX, "--parent", done]);
  }

  const decision = ensureNode(env, "decision", DECLINED_TITLE, {
    decision_status: "accepted",
  });

  for (const t of [stopped, review, done, question]) {
    ensureEdge(env, project, "tasks", t);
  }
  ensureEdge(env, owner, "tasks", stopped);
  ensureEdge(env, ingrid, "tasks", review);
  ensureEdge(env, owner, "tasks", done);
  ensureEdge(env, owner, "tasks", question);
  // The question stops the review task; it is the owner's to answer.
  ensureEdge(env, question, "blocks", review);

  const ids = { project, anoop, stopped, done, review, decision, question };
  useSeededIds(ids);
  return ids;
}

function same(a: string, b: string): boolean {
  return bareId(a) === bareId(b);
}

const WORK_TYPES = ["task", "issue"];

function addedWork(diff: GraphDiff): SnapshotNode[] {
  return diff.addedNodes.filter((n) => WORK_TYPES.includes(n.node_type));
}

function children(diff: GraphDiff, parent: SnapshotNode, type: string): SnapshotNode[] {
  const ids = diff.addedEdges
    .filter((e) => e.relation === "has_child" && same(e.from, parent.id))
    .map((e) => e.to);
  return diff.addedNodes.filter(
    (n) => n.node_type === type && ids.some((id) => same(id, n.id)),
  );
}

function linksTo(diff: GraphDiff, from: SnapshotNode, relation: string, to: string): boolean {
  return diff.addedEdges.some(
    (e) => e.relation === relation && same(e.from, from.id) && same(e.to, to),
  );
}

function linksAny(diff: GraphDiff, from: SnapshotNode, relation: string): boolean {
  return diff.addedEdges.some((e) => e.relation === relation && same(e.from, from.id));
}

function fail(failure: string): Verdict {
  return { passed: false, failure };
}

function touchedSeeded(diff: GraphDiff, ids: string[]): string | null {
  const hit = diff.changedNodes.find((c) => ids.some((id) => same(id, c.after.id)));
  return hit ? `${hit.after.node_type}:"${hit.after.content.slice(0, 60)}"` : null;
}

/** The bug is created as one ready task: brief, unticked checklist, its project. */
function scoreReadyTask(diff: GraphDiff, ids: SeededIds): Verdict {
  const work = addedWork(diff);
  if (work.length !== 1) {
    return fail(
      `expected exactly one task to be created, got ${work.length}: ` +
        (work.map((n) => `${n.node_type}:"${n.content.slice(0, 60)}"`).join("; ") || "(none)"),
    );
  }
  const task = work[0];
  if (diff.addedNodes.some((n) => n.node_type === "decision")) {
    return fail("a decision was recorded for a request nobody declined");
  }
  const touched = touchedSeeded(diff, [ids.done, ids.project]);
  if (touched) return fail(`the report was meant to be tracked, but ${touched} was changed`);
  if (children(diff, task, "text").length === 0) {
    return fail("the task has no brief: no text node under it");
  }
  const boxes = children(diff, task, "checkbox");
  if (boxes.length === 0) return fail("the task has no checklist: no checkbox under it");
  if (boxes.some((b) => /\[x\]/i.test(b.content))) {
    return fail("a checklist box was created already ticked");
  }
  if (!linksTo(diff, task, "project", ids.project)) {
    return fail("the task is not linked to its project");
  }
  const status = task.properties.status;
  if (status !== undefined && String(status).toLowerCase() !== "open") {
    return fail(`the task is not open, so it is not in Ready tasks: status=${String(status)}`);
  }
  if (linksAny(diff, task, "blocked_by")) {
    return fail("the task is blocked, so it is not in Ready tasks");
  }
  return { passed: true };
}

/** A question task for its owner, blocking the stopped task, which is left as it was. */
function scoreQuestionTask(diff: GraphDiff, ids: SeededIds): Verdict {
  const work = addedWork(diff);
  if (work.length !== 1) {
    return fail(
      `expected exactly one question task to be created, got ${work.length}: ` +
        (work.map((n) => `${n.node_type}:"${n.content.slice(0, 60)}"`).join("; ") || "(none)"),
    );
  }
  const q = work[0];
  if (q.content.trim() === "") return fail("the question task holds no question");
  if (!linksTo(diff, q, "assignee", ids.anoop)) {
    return fail("the question task is not assigned to the person who owes the answer");
  }
  if (!linksTo(diff, q, "blocks", ids.stopped)) {
    return fail("the question task does not block the task that stopped");
  }
  const touched = touchedSeeded(diff, [ids.stopped]);
  if (touched) {
    return fail(`the stopped task must keep its status and assignee, but ${touched} was changed`);
  }
  return { passed: true };
}

/** Nothing written. The agent may report, or ask; what it must not do is act. */
function scoreNoWrites(diff: GraphDiff): Verdict {
  return assertEndState({ expectNoWrites: true }, diff);
}

/** Nothing written, and the question reached the user. */
function scoreAsksUser(diff: GraphDiff, turns: TurnRecord[] | undefined): Verdict {
  const wrote = scoreNoWrites(diff);
  if (!wrote.passed) {
    return fail(`an answer was recorded for the user: ${wrote.failure ?? ""}`);
  }
  const seen = turns ?? [];
  const called = seen.flatMap((t) => t.toolsCalled);
  const reply = seen.length ? seen[seen.length - 1].reply : "";
  // Asking for more context is not putting the question: the agent has to have
  // found the waiting question, which its reply shows by naming its subject.
  if (!WAITING_QUESTION_SUBJECT.test(reply)) {
    return fail("the waiting question was never found, so it was not put to the user");
  }
  if (called.includes("route_clarify") || reply.includes("?")) return { passed: true };
  return fail("nothing was written, but the question was never put to the user");
}

export function scoreOutcome(
  scenario: Scenario,
  diff: GraphDiff,
  turns?: TurnRecord[],
): Verdict {
  if (diff.captureError) {
    return fail(`graph end-state could not be captured: ${diff.captureError}`);
  }
  const e = (scenario as TaskScenario).expected;
  if (e.kind === "no-writes") return scoreNoWrites(diff);
  if (e.kind === "asks-user") return scoreAsksUser(diff, turns);
  if (!seeded) return fail("the workspace was never seeded, so ids are unknown");
  return e.kind === "ready-task" ? scoreReadyTask(diff, seeded) : scoreQuestionTask(diff, seeded);
}

/**
 * The trajectory, a diagnostic beside the outcome: a created task is preceded
 * by a search, and a scenario that must write nothing calls no write tool.
 */
export function scoreTrajectory(scenario: Scenario, turns: TurnRecord[]): Verdict {
  const turn = turns[turns.length - 1];
  if (!turn) return fail("No turn recorded.");
  const e = (scenario as TaskScenario).expected;
  const calls = turn.toolsCalled;
  const firstCreate = calls.indexOf("create_node");
  if (e.kind === "ready-task") {
    const searched = calls.indexOf("search_nodes");
    return searched !== -1 && (firstCreate === -1 || searched < firstCreate)
      ? { passed: true }
      : fail(`expected a search before the first create_node; tools called: ${calls.join(", ") || "none"}`);
  }
  if (e.kind === "no-writes" || e.kind === "asks-user") {
    const wrote = calls.filter((t) => /^(create|update)_/.test(t));
    return wrote.length === 0 ? { passed: true } : fail(`wrote: ${wrote.join(", ")}`);
  }
  return firstCreate !== -1
    ? { passed: true }
    : fail(`no create_node call; tools called: ${calls.join(", ") || "none"}`);
}

const fixture: EvalFixture = {
  name: "tasks",
  description: "Tasks Eval Results (creating a task, answering a question)",
  // One chat per scenario: a write one scenario made must not be a precondition
  // for, or a distraction in, the next.
  groups: FIXTURES.map((s) => [s]),
  seedGroup(env: EvalEnv) {
    seedWorkspace(env);
  },
  score: scoreTrajectory,
  graph: {
    types: [],
    relations: RELATIONS,
    scoreOutcome,
  },
  extra(scenario, turns) {
    return {
      expected: (scenario as TaskScenario).expected,
      routedSkills: turns.map((t) => t.routedSkills ?? ""),
      toolsOffered: turns.map((t) => t.toolsOffered),
      toolsCalled: turns.flatMap((t) => t.toolsCalled),
      toolCalls: turns.flatMap((t) => t.toolCalls ?? []),
      latencyMs: turns.reduce((sum, t) => sum + t.latencyMs, 0),
    };
  },
};

export default fixture;
