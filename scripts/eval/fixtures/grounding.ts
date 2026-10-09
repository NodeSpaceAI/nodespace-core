/**
 * Grounding eval — scores whether a question about tasks assigned to a person
 * is answered correctly, and records what the turn was given to answer it.
 *
 * The workspace holds two people and a few tasks. A task's assignee is the
 * derived other side of `person.tasks`, so the question can only be answered
 * through that relationship; a property filter on `assignee` finds nothing and
 * reads as "no tasks". Each scenario is a different wording of a lookup, and a
 * correct answer is the count (or the statuses) the seed fixed in advance.
 *
 * What it records beside the verdict is the evidence the context work is
 * measured on: the skills routed to the turn, the tools it was offered and
 * called, and its latency.
 *
 * The wording of the prompts comes from manual chat transcripts and is kept as
 * typed, typos included.
 */

import type { EvalEnv } from "../env.ts";
import type { EvalFixture, Scenario, TurnRecord, Verdict } from "../types.ts";

const PERSON_TYPE = "person";
const TASK_TYPE = "task";

interface SeededTask {
  title: string;
  status: string;
  assignee: string | null;
}

interface SeededPerson {
  first: string;
  last: string;
}

const PEOPLE: SeededPerson[] = [
  { first: "Anoop", last: "Nair" },
  { first: "Norbert", last: "Weber" },
];

const TASKS: SeededTask[] = [
  { title: "Draft the vendor contract", status: "open", assignee: "Nair" },
  { title: "Review the pricing page", status: "in_progress", assignee: "Nair" },
  { title: "Plan the offsite", status: "open", assignee: null },
];

type Expected =
  | { kind: "count"; count: number }
  | { kind: "statuses"; statuses: RegExp[] };

interface GroundingScenario extends Scenario {
  expected: Expected;
}

const NUMBER_WORDS = ["zero", "one", "two", "three", "four", "five"];

const FIXTURES: GroundingScenario[] = [
  {
    id: "count-first-name",
    scenario: "Count of tasks assigned to a person, by first name",
    prompt: "How many tasks are assigned to Anoop?",
    expected: { kind: "count", count: 2 },
  },
  {
    id: "count-missing-preposition",
    scenario: "Count, wording without 'to'",
    prompt: "Tell me how many tasks are assigned Anoop",
    expected: { kind: "count", count: 2 },
  },
  {
    id: "count-typo-full-name",
    scenario: "Count by full name, with a typo in the verb",
    prompt: "How many tasks are assigne to Anoop Nair?",
    expected: { kind: "count", count: 2 },
  },
  {
    id: "count-none",
    scenario: "Count for a person with no tasks",
    prompt: "How many tasks are assigned to Norbert",
    expected: { kind: "count", count: 0 },
  },
  {
    id: "status-of-assigned",
    scenario: "Status of the tasks assigned to a person",
    prompt: "Could you check the status of tasks assigned to Anoop",
    expected: { kind: "statuses", statuses: [/open/i, /in[ _-]?progress/i] },
  },
];

function runNs(env: EvalEnv, args: string[]): unknown {
  const r = Bun.spawnSync([env.nsBin, "--socket", env.socket, "--json", ...args], {
    stdout: "pipe",
    stderr: "pipe",
  });
  if (r.exitCode !== 0) {
    throw new Error(
      `nodespace ${args.join(" ")} failed (exit ${r.exitCode}): ` +
        r.stderr.toString().trim(),
    );
  }
  const out = r.stdout.toString().trim();
  return out ? JSON.parse(out) : null;
}

interface ListedNode {
  id?: string;
  title?: string;
  content?: string;
}

function listType(env: EvalEnv, type: string): ListedNode[] {
  const out = runNs(env, ["node", "query", "--type", type, "--limit", "200"]) as
    | { nodes?: ListedNode[] }
    | ListedNode[]
    | null;
  return Array.isArray(out) ? out : (out?.nodes ?? []);
}

function bareId(id: string): string {
  return id.replace(/^nodespace:\/\//, "");
}

/**
 * Create the people and tasks and the `tasks` edges between them.
 *
 * Idempotent: a node already present by title is reused, and an edge that
 * already exists is not an error. It runs before every chat, so a database a
 * run left behind or one `--between-runs` wiped both end up the same.
 */
function seedWorkspace(env: EvalEnv): void {
  const people = new Map<string, string>();
  const existingPeople = listType(env, PERSON_TYPE);
  for (const p of PEOPLE) {
    const full = `${p.first} ${p.last}`;
    const present = existingPeople.find(
      (n) => (n.title ?? n.content ?? "").toLowerCase() === full.toLowerCase(),
    );
    const id =
      present?.id ??
      (
        runNs(env, [
          "node",
          "create",
          "--type",
          PERSON_TYPE,
          "--properties",
          JSON.stringify({ first_name: p.first, last_name: p.last }),
        ]) as { id?: string } | null
      )?.id;
    if (!id) throw new Error(`seeding person '${full}' returned no id`);
    people.set(p.last, bareId(id));
  }

  const existingTasks = listType(env, TASK_TYPE);
  for (const t of TASKS) {
    const present = existingTasks.find(
      (n) => (n.title ?? n.content ?? "").toLowerCase() === t.title.toLowerCase(),
    );
    const id =
      present?.id ??
      (
        runNs(env, [
          "node",
          "create",
          "--type",
          TASK_TYPE,
          "--content",
          t.title,
          "--properties",
          JSON.stringify({ status: t.status }),
        ]) as { id?: string } | null
      )?.id;
    if (!id) throw new Error(`seeding task '${t.title}' returned no id`);
    if (t.assignee) {
      const person = people.get(t.assignee);
      if (!person) throw new Error(`no seeded person '${t.assignee}'`);
      // Creating an edge that exists already must not fail the seed.
      try {
        runNs(env, [
          "relationship",
          "create",
          "--from",
          person,
          "--type",
          "tasks",
          "--to",
          bareId(id),
        ]);
      } catch (e) {
        if (!/exist|already|duplicate/i.test(String(e))) throw e;
      }
    }
  }
}

/** True when the reply states `count` as a digit or a number word. */
export function statesCount(reply: string, count: number): boolean {
  const word = NUMBER_WORDS[count];
  const other = [...Array(NUMBER_WORDS.length).keys()].filter((n) => n !== count);
  const says = (n: number): boolean =>
    new RegExp(`\\b(${n}|${NUMBER_WORDS[n]})\\b`, "i").test(reply);
  if (count === 0) {
    return (
      /\b(0|zero|no|none|not\s+assigned)\b/i.test(reply) &&
      !other.some(says)
    );
  }
  return (
    new RegExp(`\\b(${count}|${word})\\b`, "i").test(reply) &&
    !other.filter((n) => n !== 0).some(says)
  );
}

export function assertScenario(
  scenario: GroundingScenario,
  turns: TurnRecord[],
): Verdict {
  const turn = turns[turns.length - 1];
  if (!turn) return { passed: false, failure: "No turn recorded." };
  const wrote = turn.toolsCalled.filter((t) =>
    ["create_node", "update_node", "update_task_status", "create_relationship"].includes(t),
  );
  if (wrote.length > 0) {
    return {
      passed: false,
      failure: `A lookup wrote to the graph: ${wrote.join(", ")}`,
    };
  }
  const e = scenario.expected;
  if (e.kind === "count") {
    return statesCount(turn.reply, e.count)
      ? { passed: true }
      : {
          passed: false,
          failure: `Expected the reply to state ${e.count}; got: ${turn.reply.slice(0, 200)}`,
        };
  }
  const missing = e.statuses.filter((re) => !re.test(turn.reply));
  return missing.length === 0
    ? { passed: true }
    : {
        passed: false,
        failure: `Reply is missing ${missing.map(String).join(", ")}: ${turn.reply.slice(0, 200)}`,
      };
}

const fixture: EvalFixture = {
  name: "grounding",
  description: "Grounding Eval Results (lookups through a derived relationship)",
  // One chat per scenario, so a long conversation is not scored as a bad lookup.
  groups: FIXTURES.map((s) => [s]),
  seedGroup(env: EvalEnv) {
    seedWorkspace(env);
  },
  score(scenario, turns) {
    return assertScenario(scenario as GroundingScenario, turns);
  },
  extra(scenario, turns) {
    return {
      expected: (scenario as GroundingScenario).expected,
      routedSkills: turns.map((t) => t.routedSkills ?? ""),
      toolsOffered: turns.map((t) => t.toolsOffered),
      toolsCalled: turns.flatMap((t) => t.toolsCalled),
      latencyMs: turns.reduce((sum, t) => sum + t.latencyMs, 0),
      routingMs: turns.reduce((sum, t) => sum + (t.routingMs ?? 0), 0),
    };
  },
};

export default fixture;
