/**
 * Unit tests for assertEndState() and diffSnapshots() — the functions that
 * decide whether an agent-matrix scenario passed once grading moved from
 * trajectory to outcome.
 *
 * Runs via `bun run test:scripts` (and so under `bun run test:all`). Deliberate
 * exception to the project-wide "never use `bun test`" rule, for the same
 * reason the sibling eval tests are: this file touches no DOM, and cannot run
 * under Vitest anyway (it imports `bun:test`, and scripts/ is outside every
 * Vitest project glob).
 *
 * The three cases the whole change exists for get named tests here, because
 * "the new scorer agrees with the old one" is not the property worth
 * protecting — the property worth protecting is that it DISAGREES on exactly
 * these, and in the right direction:
 *   - a correct result reached by a shorter path passes;
 *   - a self-corrected write passes;
 *   - two create_schema calls fail while two search_nodes calls pass.
 */

import { describe, expect, test } from "bun:test";
import {
  assertEndState,
  turnAskedForClarification,
  populatedCount,
  nodeSatisfies,
  valueMatches,
  type EndState,
} from "./end-state.ts";
import {
  diffSnapshots,
  readNodeList,
  readRelatedIds,
  toSnapshotNode,
  type GraphDiff,
  type GraphSnapshot,
  type SnapshotNode,
} from "./graph.ts";

function node(
  id: string,
  type: string,
  content: string,
  properties: Record<string, unknown> = {},
): SnapshotNode {
  return { id, node_type: type, content, properties };
}

function snapshot(
  nodes: SnapshotNode[],
  schemas: string[] = [],
  edges: GraphSnapshot["edges"] = [],
): GraphSnapshot {
  return { nodes, schemas, edges };
}

/** An empty diff — nothing at all happened this turn. */
const NOTHING: GraphDiff = {
  addedNodes: [],
  addedSchemas: [],
  addedEdges: [],
  changedNodes: [],
};

describe("diffSnapshots", () => {
  test("reports nodes added between the two snapshots", () => {
    const before = snapshot([node("a", "task", "one")]);
    const after = snapshot([node("a", "task", "one"), node("b", "task", "two")]);
    const d = diffSnapshots(before, after);
    expect(d.addedNodes.map((n) => n.id)).toEqual(["b"]);
    expect(d.changedNodes).toEqual([]);
  });

  test("reports a node whose properties changed", () => {
    const before = snapshot([node("a", "task", "one", { status: "open" })]);
    const after = snapshot([node("a", "task", "one", { status: "done" })]);
    const d = diffSnapshots(before, after);
    expect(d.addedNodes).toEqual([]);
    expect(d.changedNodes.map((c) => c.after.properties.status)).toEqual(["done"]);
  });

  test("reports a node whose content changed", () => {
    const before = snapshot([node("a", "text", "old")]);
    const after = snapshot([node("a", "text", "new")]);
    expect(diffSnapshots(before, after).changedNodes).toHaveLength(1);
  });

  test("an unchanged node is neither added nor changed", () => {
    const before = snapshot([node("a", "task", "one", { status: "open" })]);
    const after = snapshot([node("a", "task", "one", { status: "open" })]);
    const d = diffSnapshots(before, after);
    expect(d.addedNodes).toEqual([]);
    expect(d.changedNodes).toEqual([]);
  });

  // Plain JSON.stringify is key-order sensitive, so a daemon reserializing a
  // node's properties differently would report a change that did not happen —
  // and `expectNoWrites` fails on ANY changed node, so a read scenario would
  // red out for something the model had no part in.
  test("a property key reordering is not a change", () => {
    const before = snapshot([
      node("a", "spec", "x", { days: 5, signed_off: false }),
    ]);
    const after = snapshot([
      node("a", "spec", "x", { signed_off: false, days: 5 }),
    ]);
    expect(diffSnapshots(before, after).changedNodes).toEqual([]);
  });

  test("a nested property reordering is not a change either", () => {
    const before = snapshot([node("a", "spec", "x", { meta: { b: 1, a: 2 } })]);
    const after = snapshot([node("a", "spec", "x", { meta: { a: 2, b: 1 } })]);
    expect(diffSnapshots(before, after).changedNodes).toEqual([]);
  });

  // The normalization must not hide a real change.
  test("a real property change is still detected", () => {
    const before = snapshot([node("a", "spec", "x", { days: 5 })]);
    const after = snapshot([node("a", "spec", "x", { days: 8 })]);
    expect(diffSnapshots(before, after).changedNodes).toHaveLength(1);
  });

  test("array order is still significant", () => {
    const before = snapshot([node("a", "spec", "x", { tags: ["p", "q"] })]);
    const after = snapshot([node("a", "spec", "x", { tags: ["q", "p"] })]);
    expect(diffSnapshots(before, after).changedNodes).toHaveLength(1);
  });

  test("reports added schemas and edges", () => {
    const before = snapshot([], ["task"]);
    const after = snapshot([], ["task", "spec"], [
      { from: "a", relation: "mentions", to: "b" },
    ]);
    const d = diffSnapshots(before, after);
    expect(d.addedSchemas).toEqual(["spec"]);
    expect(d.addedEdges).toHaveLength(1);
  });

  test("an edge present before the turn is not reported as added", () => {
    const edge = { from: "a", relation: "mentions", to: "b" };
    const d = diffSnapshots(snapshot([], [], [edge]), snapshot([], [], [edge]));
    expect(d.addedEdges).toEqual([]);
  });

  // A capture fault must not become a model verdict — the same confusion
  // `sendFailed` and `emptyGeneration` exist to prevent, one layer down.
  test("a capture error propagates and suppresses every other finding", () => {
    const before: GraphSnapshot = {
      nodes: [],
      schemas: [],
      edges: [],
      captureError: "daemon went away",
    };
    const after = snapshot([node("a", "task", "one")]);
    const d = diffSnapshots(before, after);
    expect(d.captureError).toBe("daemon went away");
    expect(d.addedNodes).toEqual([]);
  });
});

describe("snapshot parsing", () => {
  test("reads a CLI node list", () => {
    const nodes = readNodeList({
      count: 1,
      nodes: [
        { id: "a", node_type: "task", content: "x", properties: { status: "open" } },
      ],
    });
    expect(nodes).toEqual([node("a", "task", "x", { status: "open" })]);
  });

  test("tolerates a payload with no nodes array", () => {
    expect(readNodeList(null)).toEqual([]);
    expect(readNodeList({})).toEqual([]);
  });

  // output.rs::node_to_json degrades `properties` to a raw string if the
  // daemon's encoding ever breaks. That must not crash a snapshot.
  test("treats non-object properties as no properties", () => {
    const n = toSnapshotNode({
      id: "a",
      node_type: "task",
      content: "x",
      properties: "{malformed",
    });
    expect(n?.properties).toEqual({});
  });

  test("rejects a node with no id or type", () => {
    expect(toSnapshotNode({ node_type: "task" })).toBeNull();
    expect(toSnapshotNode({ id: "a" })).toBeNull();
  });

  test("reads related ids from objects and bare strings alike", () => {
    expect(
      readRelatedIds({ nodes: [{ id: "a" }, "b", { nope: 1 }] }),
    ).toEqual(["a", "b"]);
  });

  test("tolerates a payload with no nodes array", () => {
    expect(readRelatedIds(null)).toEqual([]);
    expect(readRelatedIds({ count: 0 })).toEqual([]);
  });
});

describe("valueMatches", () => {
  // The model chooses the serialization; the product does not care which.
  // Pinning one spelling would red-line a correct write for a formatting
  // choice, which is the class of false failure this change exists to remove.
  test("compares numbers across the string/number boundary", () => {
    expect(valueMatches(8, 8)).toBe(true);
    expect(valueMatches("8", 8)).toBe(true);
    expect(valueMatches(5, 8)).toBe(false);
  });

  test("compares strings ignoring case, spacing and separators", () => {
    expect(valueMatches("Signed off", "signed_off")).toBe(true);
    expect(valueMatches("in_progress", "In Progress")).toBe(true);
    expect(valueMatches("open", "done")).toBe(false);
  });

  // `Number([8]) === 8`, so a bare Number() coercion would accept a
  // single-element array as the scalar the prompt asked for. The string/number
  // tolerance is deliberate; array coercion is not.
  test("does not accept an array as a scalar number", () => {
    expect(valueMatches([8], 8)).toBe(false);
    expect(valueMatches(["8"], 8)).toBe(false);
    expect(valueMatches({ v: 8 }, 8)).toBe(false);
    expect(valueMatches(null, 8)).toBe(false);
  });

  test("`true` asserts presence rather than a value", () => {
    expect(valueMatches("2026-08-06", true)).toBe(true);
    expect(valueMatches(0, true)).toBe(true);
    expect(valueMatches("", true)).toBe(false);
    expect(valueMatches(null, true)).toBe(false);
    expect(valueMatches(undefined, true)).toBe(false);
  });
});

describe("populatedCount", () => {
  test("counts only properties holding a real value", () => {
    expect(
      populatedCount({ a: "x", b: "", c: null, d: undefined, e: 0, f: [] }),
    ).toBe(2);
  });
});

describe("nodeSatisfies", () => {
  test("matches on type, content substring and property value", () => {
    const n = node("a", "task", "Rebuild the reports page", { status: "open" });
    expect(nodeSatisfies(n, { type: "task" })).toBe(true);
    expect(nodeSatisfies(n, { contentMatches: "reports" })).toBe(true);
    expect(nodeSatisfies(n, { contentMatches: "REPORTS" })).toBe(true);
    expect(nodeSatisfies(n, { properties: { status: "open" } })).toBe(true);
    expect(nodeSatisfies(n, { type: "text" })).toBe(false);
    expect(nodeSatisfies(n, { contentMatches: "invoices" })).toBe(false);
  });

  test("minProperties counts populated values only", () => {
    const n = node("a", "spec", "offline sync", { days: 5, note: "" });
    expect(nodeSatisfies(n, { minProperties: 1 })).toBe(true);
    expect(nodeSatisfies(n, { minProperties: 2 })).toBe(false);
  });

  // The winnability-preserving pin: the model chooses the key, so the value is
  // the part that is actually about the model's behavior.
  test("hasPropertyValue matches whichever key the value landed under", () => {
    expect(
      nodeSatisfies(node("a", "spec", "x", { estimate_days: 8 }), {
        hasPropertyValue: 8,
      }),
    ).toBe(true);
    expect(
      nodeSatisfies(node("a", "spec", "x", { days: "8" }), {
        hasPropertyValue: 8,
      }),
    ).toBe(true);
    expect(
      nodeSatisfies(node("a", "spec", "x", { days: 5 }), {
        hasPropertyValue: 8,
      }),
    ).toBe(false);
  });

  // Strictly stronger than minProperties, which any unrelated write satisfies.
  test("hasPropertyValue is not satisfied by an unrelated property", () => {
    const n = node("a", "spec", "offline sync", { signed_off: true });
    expect(nodeSatisfies(n, { minProperties: 1 })).toBe(true);
    expect(nodeSatisfies(n, { hasPropertyValue: 8 })).toBe(false);
  });
});

describe("assertEndState", () => {
  test("createdNode passes when a matching node was created", () => {
    const diff: GraphDiff = {
      ...NOTHING,
      addedNodes: [node("a", "spec", "offline sync", { days: 5 })],
    };
    const want: EndState = {
      createdNode: { contentMatches: "offline sync", minProperties: 1 },
    };
    expect(assertEndState(want, diff).passed).toBe(true);
  });

  test("createdNode fails when the node persisted no property values", () => {
    const diff: GraphDiff = {
      ...NOTHING,
      addedNodes: [node("a", "spec", "offline sync")],
    };
    const v = assertEndState(
      { createdNode: { contentMatches: "offline sync", minProperties: 1 } },
      diff,
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("offline sync");
  });

  test("updatedNode passes when an existing node was modified", () => {
    const before = node("a", "spec", "offline sync", { days: 5 });
    const after = node("a", "spec", "offline sync", { days: 8 });
    const diff: GraphDiff = { ...NOTHING, changedNodes: [{ before, after }] };
    expect(
      assertEndState(
        { updatedNode: { contentMatches: "offline sync", properties: { days: 8 } } },
        diff,
      ).passed,
    ).toBe(true);
  });

  // The failure worth naming: it looks like success to the user until they
  // find two nodes where they expected one.
  test("updatedNode fails, and says so specifically, when the turn created a duplicate instead", () => {
    const diff: GraphDiff = {
      ...NOTHING,
      addedNodes: [node("b", "spec", "offline sync", { days: 8 })],
    };
    const v = assertEndState(
      { updatedNode: { contentMatches: "offline sync" } },
      diff,
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("duplicate");
  });

  test("createdSchemas passes on the expected count", () => {
    expect(
      assertEndState({ createdSchemas: 1 }, { ...NOTHING, addedSchemas: ["spec"] })
        .passed,
    ).toBe(true);
  });

  test("createdSchemas fails when no type was created", () => {
    const v = assertEndState({ createdSchemas: 1 }, NOTHING);
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("nowhere to record");
  });

  test("createdEdge passes only for the named relation", () => {
    const diff: GraphDiff = {
      ...NOTHING,
      addedEdges: [{ from: "a", relation: "mentions", to: "b" }],
    };
    expect(assertEndState({ createdEdge: { relation: "mentions" } }, diff).passed).toBe(
      true,
    );
    expect(assertEndState({ createdEdge: { relation: "member_of" } }, diff).passed).toBe(
      false,
    );
  });

  test("expectNoWrites passes on a turn that changed nothing", () => {
    expect(assertEndState({ expectNoWrites: true }, NOTHING).passed).toBe(true);
  });

  test("expectNoWrites fails when the turn created a node", () => {
    const v = assertEndState(
      { expectNoWrites: true },
      { ...NOTHING, addedNodes: [node("a", "text", "the answer")] },
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("write nothing");
  });

  test("noUnexpectedNodes ignores the node the scenario asked for", () => {
    const diff: GraphDiff = {
      ...NOTHING,
      addedNodes: [node("a", "spec", "offline sync", { days: 5 })],
    };
    expect(
      assertEndState(
        {
          createdNode: { contentMatches: "offline sync" },
          noUnexpectedNodes: true,
        },
        diff,
      ).passed,
    ).toBe(true);
  });

  test("noUnexpectedNodes fails on a node the scenario did not ask for", () => {
    const diff: GraphDiff = {
      ...NOTHING,
      addedNodes: [
        node("a", "spec", "offline sync", { days: 5 }),
        node("b", "spec", "something else entirely"),
      ],
    };
    const v = assertEndState(
      { createdNode: { contentMatches: "offline sync" }, noUnexpectedNodes: true },
      diff,
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("unexpected node");
  });

  // A capture fault is an environment failure. Scoring it either way files a
  // dead daemon as a model verdict.
  test("declines to score when the snapshot could not be captured", () => {
    const v = assertEndState(
      { expectNoWrites: true },
      { ...NOTHING, captureError: "socket closed" },
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("could not be captured");
  });
});

// ---------------------------------------------------------------------------
// The three cases this whole change exists for.
//
// Each is a live observation from the runs documented on the issue, and each
// one scored the WRONG way under trajectory grading.
// ---------------------------------------------------------------------------

describe("the disagreements that motivated outcome grading", () => {
  test("a correct result reached by a shorter path passes (scenario 6)", () => {
    // DeepSeek V4 Pro called update_node and produced the right end state
    // without calling resolve_query first. Trajectory failed it on the missing
    // step; the user got exactly what they asked for.
    const before = node("a", "spec", "offline sync", { days: 5 });
    const after = node("a", "spec", "offline sync", {
      days: 5,
      signed_off: true,
    });
    const diff: GraphDiff = { ...NOTHING, changedNodes: [{ before, after }] };
    const v = assertEndState(
      {
        updatedNode: { contentMatches: "offline sync", minProperties: 1 },
        noUnexpectedNodes: true,
      },
      diff,
    );
    expect(v.passed).toBe(true);
  });

  test("a self-corrected write passes (scenario 11a's shape)", () => {
    // create_node rejected for a missing node_type, the model supplied it, and
    // the second call persisted. Trajectory red-lined it on an exactly-once
    // rule — punishing the recovery behaviour we want. The end state is a
    // single correct node either way.
    const diff: GraphDiff = {
      ...NOTHING,
      addedNodes: [node("a", "text", "the reports page uses server-side rendering")],
    };
    expect(
      assertEndState({ createdNode: { contentMatches: "reports page" } }, diff).passed,
    ).toBe(true);
  });

  describe("severity is no longer flat", () => {
    // Two search_nodes calls: wasted latency, nothing persisted, no harm done.
    test("a repeated read passes, because it changed nothing", () => {
      expect(assertEndState({ expectNoWrites: true }, NOTHING).passed).toBe(true);
    });

    // Two create_schema calls: a spurious type is now in the user's graph and
    // needs real cleanup. Same score as the above under trajectory grading.
    test("a repeated schema creation fails, because a spurious type persists", () => {
      const v = assertEndState(
        { createdSchemas: 1 },
        { ...NOTHING, addedSchemas: ["spec", "spec_draft"] },
      );
      expect(v.passed).toBe(false);
      expect(v.failure).toContain("cleanup");
    });

    // Zero search_nodes on a query: the user gets a wrong answer. Caught by
    // the trajectory diagnostic rather than by end state, which is the honest
    // division — a read that answers from nothing leaves no trace in the graph
    // by definition, so end state cannot see it and does not pretend to.
    test("a read that never searched leaves no end-state trace, by construction", () => {
      expect(assertEndState({ expectNoWrites: true }, NOTHING).passed).toBe(true);
    });
  });
});

describe("clarifyOk", () => {
  const empty: GraphDiff = {
    addedNodes: [],
    changedNodes: [],
    addedEdges: [],
    addedSchemas: [],
  };
  const wantWrite: EndState = {
    clarifyOk: true,
    createdNode: { contentMatches: "harbour" },
  };
  const notAction = (t: string) => t === "route_clarify";

  test("credits a turn that called route_clarify", () => {
    const asked = turnAskedForClarification(["route_clarify"], "Which one?", notAction);
    expect(assertEndState(wantWrite, empty, asked).passed).toBe(true);
  });

  test("credits a no-action turn whose reply asks a question", () => {
    const asked = turnAskedForClarification([], "What is the start date?", notAction);
    expect(assertEndState(wantWrite, empty, asked).passed).toBe(true);
  });

  test("does NOT credit a clarification when the scenario did not opt in", () => {
    const asked = turnAskedForClarification([], "What is the start date?", notAction);
    const want: EndState = { createdNode: { contentMatches: "harbour" } };
    expect(assertEndState(want, empty, asked).passed).toBe(false);
  });

  test("does NOT credit a silent no-op — a question mark is required", () => {
    const asked = turnAskedForClarification([], "Done.", notAction);
    expect(asked).toBe(false);
    expect(assertEndState(wantWrite, empty, asked).passed).toBe(false);
  });

  test("does NOT credit a turn that acted and merely ended on a question", () => {
    // The write happened; a trailing question must not convert a wrong
    // outcome into a pass.
    const asked = turnAskedForClarification(
      ["create_node"],
      "Created it. Anything else?",
      notAction,
    );
    expect(asked).toBe(false);
  });

  test("a correct write still passes when clarifyOk is set", () => {
    const diff: GraphDiff = {
      ...empty,
      addedNodes: [
        { id: "n1", node_type: "planning_cycle", content: "Harbour", properties: {} },
      ],
    };
    const asked = turnAskedForClarification(["create_node"], "Created.", notAction);
    expect(assertEndState(wantWrite, diff, asked).passed).toBe(true);
  });

  test("an uncapturable snapshot still reports the environment fault", () => {
    // clarifyOk must not mask a dead daemon: the capture error is the more
    // important signal and is checked first.
    const broken: GraphDiff = { ...empty, captureError: "daemon gone" };
    // askedForClarification TRUE is the case that matters: a dead daemon plus
    // a reply containing a question mark must not score as a pass. Passing
    // false here would make the test vacuous — it would pass even if the
    // clarifyOk branch ran first.
    const v = assertEndState(wantWrite, broken, true);
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("could not be captured");
  });
});

// -- Nested (type-keyed) properties -------------------------------------
//
// These assert the SCORING OUTCOME, not that a helper exists: each one is
// built from a real daemon-shaped payload that scored red before the
// flattening fix, so a regression re-reds the same scenario rather than
// merely changing a helper's return value.

describe("type-keyed property flattening", () => {
  // Exactly the payload scenario 13 produced. It failed 3/3 reps: the write
  // was correct and `>=2 property value(s)` could not be satisfied because
  // `populatedCount` saw only the wrapper.
  const scenario13 = {
    id: "n1",
    node_type: "incident_report",
    content: "search index corruption",
    properties: {
      incident_report: {
        _schema_version: 1,
        on_call: "rowan",
        resolved: true,
      },
    },
  };

  test("counts inner fields, so minProperties: 2 is satisfiable", () => {
    const node = toSnapshotNode(scenario13);
    expect(node).not.toBeNull();
    expect(nodeSatisfies(node!, { contentMatches: "search index", minProperties: 2 })).toBe(true);
  });

  test("excludes _schema_version from the count", () => {
    // One real field beside the bookkeeping key must NOT pass minProperties: 2,
    // or the fix would trade a false negative for a false positive.
    const oneRealField = toSnapshotNode({
      id: "n2",
      node_type: "task",
      content: "Swap the image resizer over to the new pipeline",
      properties: { task: { _schema_version: 1, due_date: "2026-08-06" } },
    });
    expect(nodeSatisfies(oneRealField!, { minProperties: 2 })).toBe(false);
    expect(nodeSatisfies(oneRealField!, { minProperties: 1 })).toBe(true);
  });

  test("matches a keyed property lookup (scenario 10b)", () => {
    const node = toSnapshotNode({
      id: "n3",
      node_type: "task",
      content: "Swap the image resizer over to the new pipeline",
      properties: { task: { _schema_version: 1, due_date: "2026-08-06", status: "open" } },
    });
    expect(nodeSatisfies(node!, { type: "task", properties: { due_date: true } })).toBe(true);
  });

  test("finds a value by hasPropertyValue (scenario 9)", () => {
    const node = toSnapshotNode({
      id: "n4",
      node_type: "feature_write_up",
      content: "Offline sync",
      properties: {
        feature_write_up: { _schema_version: 1, estimated_days: 8, signed_off: true },
      },
    });
    expect(nodeSatisfies(node!, { contentMatches: "offline sync", hasPropertyValue: 8 })).toBe(true);
  });

  test("leaves flat properties untouched", () => {
    const node = toSnapshotNode({
      id: "n5",
      node_type: "task",
      content: "flat",
      properties: { due_date: "2026-08-06", status: "open" },
    });
    expect(nodeSatisfies(node!, { properties: { status: "open" }, minProperties: 2 })).toBe(true);
  });

  test("keeps siblings that sit beside the wrapper", () => {
    const node = toSnapshotNode({
      id: "n6",
      node_type: "task",
      content: "mixed",
      properties: { task: { status: "open" }, extra: "kept" },
    });
    expect(nodeSatisfies(node!, { properties: { status: "open", extra: "kept" } })).toBe(true);
  });

  test("does not unwrap a key that is not the node's own type", () => {
    // `build_job` here is a property name, not this node's wrapper. Lifting it
    // would invent fields the daemon never wrote.
    const node = toSnapshotNode({
      id: "n7",
      node_type: "task",
      content: "not a wrapper",
      properties: { build_job: { estimated_days: 9 } },
    });
    expect(nodeSatisfies(node!, { properties: { estimated_days: 9 } })).toBe(false);
    expect(nodeSatisfies(node!, { hasPropertyValue: 9 })).toBe(false);
  });
});

// -- changedNodes under flattening --------------------------------------
//
// Review finding: flattening silently changed what `diffSnapshots` compares,
// on the one comparator behind `expectNoWrites` — a false-PASS path. These pin
// the intent in both directions so the loosening cannot widen unnoticed.
//
// Built through `toSnapshotNode` rather than the `node()` helper: the helper
// constructs a SnapshotNode directly, so it would never exercise the flattening
// these tests exist to check.

describe("diffSnapshots: persistence keys vs real fields", () => {
  const taskNode = (schemaVersion: number, status: string) =>
    toSnapshotNode({
      id: "a",
      node_type: "task",
      content: "one",
      properties: { task: { _schema_version: schemaVersion, status } },
    })!;

  test("a _schema_version-only bump is NOT a change", () => {
    // Persistence bumping a schema version the model never touched must not
    // red-line a read scenario through expectNoWrites.
    const d = diffSnapshots(
      snapshot([taskNode(1, "open")]),
      snapshot([taskNode(2, "open")]),
    );
    expect(d.changedNodes).toEqual([]);
  });

  test("a real field change inside the wrapper IS still a change", () => {
    // The loosening must be bounded to PERSISTENCE_KEYS: anything a model can
    // actually write still compares exactly.
    const d = diffSnapshots(
      snapshot([taskNode(1, "open")]),
      snapshot([taskNode(1, "done")]),
    );
    expect(d.changedNodes.map((c) => c.after.properties.status)).toEqual(["done"]);
  });

  test("a real field change alongside a version bump IS a change", () => {
    const d = diffSnapshots(
      snapshot([taskNode(1, "open")]),
      snapshot([taskNode(2, "done")]),
    );
    expect(d.changedNodes).toHaveLength(1);
  });
});

describe("flattenTypeKeyedProperties: collision precedence", () => {
  test("the wrapper wins over a same-named sibling", () => {
    // Stated rule, not defended behaviour: no daemon serialization produces
    // this today. Pinned so the precedence is not silently reversed.
    const n = toSnapshotNode({
      id: "n1",
      node_type: "task",
      content: "collision",
      properties: { task: { status: "inner" }, status: "outer" },
    });
    expect(n!.properties.status).toBe("inner");
  });
});
