import { beforeAll, describe, expect, test } from "bun:test";
import type { GraphDiff, SnapshotNode } from "../graph.ts";
import type { TurnRecord } from "../types.ts";
import fixture, { scoreOutcome, scoreTrajectory, useSeededIds } from "./tasks.ts";

const IDS = {
  project: "p1",
  anoop: "a1",
  stopped: "t-stopped",
  done: "t-done",
  review: "t-review",
  decision: "d1",
  question: "q-seeded",
};

const scenario = (id: string) => {
  const s = fixture.groups.flat().find((x) => x.id === id);
  if (!s) throw new Error(`no scenario ${id}`);
  return s;
};

const node = (
  id: string,
  node_type: string,
  content: string,
  properties: Record<string, unknown> = {},
): SnapshotNode => ({ id, node_type, content, properties });

const empty = (): GraphDiff => ({
  addedNodes: [],
  addedSchemas: [],
  addedEdges: [],
  changedNodes: [],
});

const turn = (reply: string, tools: string[] = []): TurnRecord =>
  ({ toolsOffered: "", toolsCalled: tools, reply, latencyMs: 0 }) as TurnRecord;

/** A well-formed bug task: brief, one unticked box, its project. */
function readyDiff(): GraphDiff {
  return {
    ...empty(),
    addedNodes: [
      node("n1", "task", "Cut the release branch shows done with an open box", { status: "open" }),
      node("n2", "text", "The task reads done while its checklist is not."),
      node("n3", "checkbox", "- [ ] A done task has no unticked box"),
    ],
    addedEdges: [
      { from: "n1", relation: "has_child", to: "n2" },
      { from: "n1", relation: "has_child", to: "n3" },
      { from: "n1", relation: "project", to: IDS.project },
    ],
  };
}

beforeAll(() => useSeededIds(IDS));

describe("fixture invariants", () => {
  test("scenario ids are unique, and each states what it expects", () => {
    const all = fixture.groups.flat();
    expect(new Set(all.map((s) => s.id)).size).toBe(all.length);
    for (const s of all) expect(s.expected).toBeDefined();
  });

  test("each scenario is its own chat", () => {
    expect(fixture.groups.every((g) => g.length === 1)).toBe(true);
  });

  test("the relations walked cover every edge a scenario asserts", () => {
    expect(fixture.graph?.relations).toEqual(
      expect.arrayContaining(["assignee", "project", "blocks", "blocked_by"]),
    );
  });
});

describe("bug-created-ready", () => {
  const s = scenario("bug-created-ready");
  const score = (d: GraphDiff) => scoreOutcome(s, d);

  test("a task with a brief, a checklist and its project passes", () => {
    expect(score(readyDiff()).passed).toBe(true);
  });

  test("nothing created fails", () => {
    expect(score(empty()).passed).toBe(false);
  });

  test("a task without a checklist fails", () => {
    const d = readyDiff();
    d.addedNodes = d.addedNodes.filter((n) => n.node_type !== "checkbox");
    expect(score(d).failure).toContain("checklist");
  });

  test("a task without its project fails", () => {
    const d = readyDiff();
    d.addedEdges = d.addedEdges.filter((e) => e.relation !== "project");
    expect(score(d).failure).toContain("project");
  });

  test("a ticked box fails", () => {
    const d = readyDiff();
    d.addedNodes[2] = node("n3", "checkbox", "- [x] done already");
    expect(score(d).passed).toBe(false);
  });

  test("a blocked or non-open task is not ready", () => {
    const blocked = readyDiff();
    blocked.addedEdges.push({ from: "n1", relation: "blocked_by", to: "x" });
    expect(score(blocked).passed).toBe(false);
    const cancelled = readyDiff();
    cancelled.addedNodes[0] = node("n1", "task", "t", { status: "cancelled" });
    expect(score(cancelled).passed).toBe(false);
  });

  test("editing the reported task instead of tracking it fails", () => {
    const d = readyDiff();
    d.changedNodes.push({
      before: node(IDS.done, "task", "Cut the release branch", { status: "done" }),
      after: node(IDS.done, "task", "Cut the release branch", { status: "open" }),
    });
    expect(score(d).passed).toBe(false);
  });

  test("a prefixed id in an edge still matches the project", () => {
    const d = readyDiff();
    d.addedEdges[2] = { from: "nodespace://n1", relation: "project", to: `nodespace://${IDS.project}` };
    expect(score(d).passed).toBe(true);
  });
});

describe("bug-diagnosed", () => {
  const s = scenario("bug-diagnosed");
  const diagnosed = (): GraphDiff => {
    const d = readyDiff();
    d.addedNodes[1] = node(
      "n2",
      "text",
      "Reproduced: ran the done transition with one box unticked and the change was refused. " +
        "The cause is a rule that only reads a change of the task's own status.",
    );
    d.addedNodes[2] = node("n3", "checkbox", "- [ ] A regression test fails before the fix");
    return d;
  };

  test("a brief with the observation and cause, and a regression item, passes", () => {
    expect(scoreOutcome(s, diagnosed()).passed).toBe(true);
  });

  test("a bare report with no cause fails", () => {
    const d = diagnosed();
    d.addedNodes[1] = node("n2", "text", "Ran it and it was wrong.");
    expect(scoreOutcome(s, d).failure).toContain("cause");
  });

  test("a brief that never says how it was seen fails", () => {
    const d = diagnosed();
    d.addedNodes[1] = node("n2", "text", "The cause is a missing rule.");
    expect(scoreOutcome(s, d).failure).toContain("seen");
  });

  test("a checklist with no regression test fails", () => {
    const d = diagnosed();
    d.addedNodes[2] = node("n3", "checkbox", "- [ ] Fix the rule");
    expect(scoreOutcome(s, d).failure).toContain("regression");
  });

  test("nothing written fails", () => {
    expect(scoreOutcome(s, empty()).passed).toBe(false);
  });

  test("a created task is preceded by a search", () => {
    expect(scoreTrajectory(s, [turn("", ["search_nodes", "create_node"])]).passed).toBe(true);
  });
});

describe("no-write scenarios", () => {
  for (const id of ["bug-not-reproducible", "declined-request"]) {
    test(`${id}: nothing written passes, a created task fails`, () => {
      expect(scoreOutcome(scenario(id), empty()).passed).toBe(true);
      const d = { ...empty(), addedNodes: [node("n1", "task", "x")] };
      expect(scoreOutcome(scenario(id), d).passed).toBe(false);
    });
  }

  test("a changed decision fails", () => {
    const d = empty();
    d.changedNodes.push({
      before: node(IDS.decision, "decision", "x", { decision_status: "accepted" }),
      after: node(IDS.decision, "decision", "x", { decision_status: "proposed" }),
    });
    expect(scoreOutcome(scenario("declined-request"), d).passed).toBe(false);
  });
});

describe("question-blocks-task", () => {
  const s = scenario("question-blocks-task");
  const good = (): GraphDiff => ({
    ...empty(),
    addedNodes: [node("q1", "task", "Does the guide cover the v1 export format?")],
    addedEdges: [
      { from: "q1", relation: "assignee", to: IDS.anoop },
      { from: "q1", relation: "blocks", to: IDS.stopped },
    ],
  });

  test("a question task for its owner that blocks the stopped task passes", () => {
    expect(scoreOutcome(s, good()).passed).toBe(true);
  });

  test("assigned to nobody fails", () => {
    const d = good();
    d.addedEdges = d.addedEdges.filter((e) => e.relation !== "assignee");
    expect(scoreOutcome(s, d).failure).toContain("assigned");
  });

  test("not blocking the stopped task fails", () => {
    const d = good();
    d.addedEdges = d.addedEdges.filter((e) => e.relation !== "blocks");
    expect(scoreOutcome(s, d).failure).toContain("block");
  });

  test("changing the stopped task fails", () => {
    const d = good();
    d.changedNodes.push({
      before: node(IDS.stopped, "task", "Write the migration guide", { status: "open" }),
      after: node(IDS.stopped, "task", "Write the migration guide", { status: "blocked" }),
    });
    expect(scoreOutcome(s, d).failure).toContain("stopped task");
  });
});

describe("answer-asks-user", () => {
  const s = scenario("answer-asks-user");

  test("a question to the user and no writes passes", () => {
    const named = "Which index format should the design lead with?";
    expect(scoreOutcome(s, empty(), [turn(named, ["route_clarify"])]).passed).toBe(true);
    expect(scoreOutcome(s, empty(), [turn(named)]).passed).toBe(true);
  });

  test("asking for more context without finding the question fails", () => {
    const reply = "I couldn't find a question. Could you tell me more about it?";
    expect(scoreOutcome(s, empty(), [turn(reply, ["search_semantic"])]).failure).toContain(
      "never found",
    );
  });

  test("an answer recorded for the user fails, however the reply reads", () => {
    const d = { ...empty(), addedNodes: [node("n1", "text", "Lead with Team")] };
    expect(scoreOutcome(s, d, [turn("Done. Anything else?", ["create_node"])]).passed).toBe(false);
  });

  test("silence with no question fails", () => {
    expect(scoreOutcome(s, empty(), [turn("Understood.")]).passed).toBe(false);
  });
});

describe("trajectory", () => {
  test("a created task is preceded by a search", () => {
    const s = scenario("bug-created-ready");
    expect(scoreTrajectory(s, [turn("", ["search_nodes", "create_node"])]).passed).toBe(true);
    expect(scoreTrajectory(s, [turn("", ["create_node", "search_nodes"])]).passed).toBe(false);
  });

  test("a no-write scenario that wrote is flagged", () => {
    const s = scenario("declined-request");
    expect(scoreTrajectory(s, [turn("", ["search_nodes"])]).passed).toBe(true);
    expect(scoreTrajectory(s, [turn("", ["create_node"])]).passed).toBe(false);
  });
});
