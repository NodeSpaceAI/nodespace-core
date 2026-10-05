/**
 * Structural invariants and scoring of the routing fixture.
 *
 * Runs via `bun run test:scripts` (and so under `bun run test:all`) — DOM-free,
 * so the `bun:test` exception applies; see decisions.test.ts for why.
 *
 * No model, no daemon: these pin how the fixture is assembled and how it
 * scores a given turn, so a fixture defect fails here instead of surfacing
 * after a multi-minute run disguised as a model result.
 */

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { TurnRecord } from "../types.ts";
import fixture, {
  assertFixture,
  assertRouting,
  constantAnswerBaseline,
  createdAType,
  stage1Expectation,
  type RoutingScenario,
} from "./routing.ts";

const scenarios = fixture.groups.flat() as RoutingScenario[];

function byId(id: string): RoutingScenario {
  const s = scenarios.find((x) => x.id === id);
  if (!s) throw new Error(`no scenario ${id}`);
  return s;
}

function turn(over: Partial<TurnRecord> = {}): TurnRecord {
  return {
    toolsOffered: "",
    toolsCalled: [],
    reply: "ok",
    latencyMs: 0,
    ...over,
  };
}

describe("routing fixture coverage", () => {
  test("covers all three Stage-1 outcomes, including a route_multi negative", () => {
    const kinds = new Set(scenarios.map((s) => s.expected.kind));
    expect(kinds.has("multi")).toBe(true);
    expect(kinds.has("clarify")).toBe(true);
    expect(kinds.has("search") || kinds.has("skill")).toBe(true);
    // The guard clauses defend against splitting ONE intent; a fixture with
    // only positive multi cases cannot see that regression.
    const negatives = scenarios.filter((s) => s.singleIntentAtLength);
    expect(negatives.length).toBeGreaterThan(0);
    expect(negatives.every((s) => s.expected.kind !== "multi")).toBe(true);
  });

  test("the header's constant-answer baseline matches the scenario list", () => {
    // A stale figure in the header is worse than none: it is what gets quoted.
    const source = readFileSync(join(import.meta.dir, "routing.ts"), "utf8");
    const m = source.match(/scores (\d+)\/(\d+) = ([\d.]+)%/);
    expect(m).not.toBeNull();
    const split = source.match(/(\d+) query \/ (\d+)\s+\*?\s*clarify \/ (\d+) multi/);
    expect(split).not.toBeNull();
    const counts = { query: 0, clarify: 0, multi: 0 };
    for (const s of scenarios) {
      const d = stage1Expectation(s.expected);
      if (d) counts[d] += 1;
    }
    expect(split?.slice(1).map(Number)).toEqual([counts.query, counts.clarify, counts.multi]);
    const base = constantAnswerBaseline(scenarios);
    expect(base.decision).toBe("query");
    expect(Number(m?.[1])).toBe(base.hits);
    expect(Number(m?.[2])).toBe(base.total);
    expect(m?.[3]).toBe(((100 * base.hits) / base.total).toFixed(1));
  });
});

describe("route_multi scoring", () => {
  const compound = byId("multi-task-and-search");

  test("a compound scenario passes only when Stage 1 routed multi", () => {
    expect(assertFixture(compound, [turn({ routingDecision: "multi" })]).passed).toBe(true);
    for (const d of ["query", "multi_rejected", "clarify", undefined]) {
      expect(assertFixture(compound, [turn({ routingDecision: d })]).passed).toBe(false);
    }
  });

  test("a single intent routed multi fails even when the right tool fires", () => {
    // Per-query retrieval after a split can still land the right skill, which
    // is how an over-splitting regression would pass on downstream effects.
    const single = byId("single-intent-long-search");
    const searched = { toolsCalled: ["search_nodes"] };
    expect(assertFixture(single, [turn({ ...searched, routingDecision: "query" })]).passed).toBe(true);
    for (const d of ["multi", "multi_rejected"]) {
      expect(assertFixture(single, [turn({ ...searched, routingDecision: d })]).passed).toBe(false);
    }
  });

  test("the multi guard applies to every non-multi scenario, not only flagged ones", () => {
    const plain = byId("direct-node-search");
    const t = turn({ toolsCalled: ["search_nodes"], routingDecision: "multi" });
    expect(assertFixture(plain, [t]).passed).toBe(false);
  });
});

describe("type-creating scenarios are scored on the type", () => {
  const created = { name: "create_schema", isError: false };
  const refused = { name: "create_schema", isError: true };
  const failedUpdate = { name: "update_schema", isError: true };

  test("every scenario that expects Schema Creation must create a type", () => {
    const schemaScenarios = scenarios.filter(
      (s) => s.expected.kind === "skill" && s.expected.skill === "Schema Creation",
    );
    expect(schemaScenarios.length).toBeGreaterThan(0);
    expect(schemaScenarios.every((s) => s.createsType === true)).toBe(true);
  });

  test("a create_schema the tool refused fails, though the turn routed", () => {
    const s = byId("indirect-schema-freelance");
    const turns = [
      turn({
        routingDecision: "query",
        toolsCalled: ["create_schema", "update_schema", "update_schema"],
        toolCalls: [refused, failedUpdate, failedUpdate],
        reply: "• schema creation failed • schema update failed (2×)",
      }),
    ];
    expect(assertRouting(s, turns).passed).toBe(true);
    const verdict = assertFixture(s, turns);
    expect(verdict.passed).toBe(false);
    expect(verdict.failure).toContain("every create_schema call was refused");
  });

  test("a refused call followed by an accepted one passes", () => {
    const s = byId("indirect-schema-freelance");
    const turns = [
      turn({
        routingDecision: "query",
        toolsCalled: ["create_schema", "create_schema"],
        toolCalls: [refused, created],
      }),
    ];
    expect(assertFixture(s, turns).passed).toBe(true);
  });

  test("naming the skill in the reply without creating a type fails", () => {
    const s = byId("direct-schema-create");
    const turns = [
      turn({
        routingDecision: "query",
        reply: "I'll use the Schema Creation skill for that.",
      }),
    ];
    expect(assertRouting(s, turns).passed).toBe(true);
    const verdict = assertFixture(s, turns);
    expect(verdict.passed).toBe(false);
    expect(verdict.failure).toContain("create_schema was not called");
  });

  test("a compound request that asks for a type must create it", () => {
    const s = byId("multi-schema-and-search");
    const searched = turn({
      routingDecision: "multi",
      toolsCalled: ["search_semantic"],
      toolCalls: [{ name: "search_semantic", isError: false }],
    });
    expect(assertFixture(s, [searched]).passed).toBe(false);
    const createdToo = turn({
      routingDecision: "multi",
      toolsCalled: ["create_schema", "search_semantic"],
      toolCalls: [created, { name: "search_semantic", isError: false }],
    });
    expect(assertFixture(s, [createdToo]).passed).toBe(true);
  });

  test("a call with no recorded outcome is not a type created", () => {
    const s = byId("direct-schema-create");
    const turns = [turn({ routingDecision: "query", toolsCalled: ["create_schema"] })];
    expect(createdAType(turns)).toBe(false);
    expect(assertRouting(s, turns).passed).toBe(true);
    expect(assertFixture(s, turns).passed).toBe(false);
  });

  test("a scenario that asks for no type is not held to one", () => {
    const s = byId("direct-node-create");
    const turns = [
      turn({
        routingDecision: "query",
        toolsCalled: ["create_node"],
        toolCalls: [{ name: "create_node", isError: false }],
      }),
    ];
    expect(assertFixture(s, turns).passed).toBe(true);
  });
});

describe("clarify scoring", () => {
  const ambiguous = byId("ambiguous-client-contacts");

  test("Stage 1 clarifying passes, whatever the question says", () => {
    const t = turn({ routingDecision: "clarify", reply: "I can take that a couple of ways." });
    expect(assertFixture(ambiguous, [t]).passed).toBe(true);
  });

  test("a search, then a question asking for more, passes", () => {
    // The reported reply, which a phrase list knowing only "could you
    // clarify" read as no question at all.
    const t = turn({
      routingDecision: "query",
      toolsCalled: ["search_semantic"],
      reply:
        "Could you provide more context, like a collection name or the topic of the documents?",
    });
    expect(assertFixture(ambiguous, [t]).passed).toBe(true);
  });

  test("an abbreviation inside the question does not cut it short", () => {
    // A measured reply.
    const t = turn({
      routingDecision: "query",
      toolsCalled: ["search_semantic"],
      reply:
        'I couldn\'t find any specific "design docs" in your workspace to organize. Could you provide more context, or perhaps tell me what kind of documents they are (e.g., Feature Specs, ADRs)?',
    });
    expect(assertFixture(ambiguous, [t]).passed).toBe(true);
  });

  test("an offer to look further is a question to the user", () => {
    const t = turn({
      routingDecision: "query",
      toolsCalled: ["search_semantic"],
      reply: "Nothing matched. Want me to try another search?",
    });
    expect(assertFixture(ambiguous, [t]).passed).toBe(true);
  });

  test("a question after a write fails", () => {
    const t = turn({
      routingDecision: "query",
      toolsCalled: ["create_node"],
      reply: "I made a collection. Would you like me to move the docs into it?",
    });
    expect(assertFixture(ambiguous, [t]).passed).toBe(false);
  });

  test("a reply that asks the user nothing fails", () => {
    for (const reply of [
      "I couldn't find any design docs.",
      "Done. Anything else?",
      "Let me know if you need more. Anything else?",
      'I found "What kind of DB?" in your notes.',
    ]) {
      const t = turn({ routingDecision: "query", toolsCalled: ["search_semantic"], reply });
      expect(assertFixture(ambiguous, [t]).passed).toBe(false);
    }
  });
});

describe("decline scoring", () => {
  const weather = byId("out-of-scope-weather");

  test("an answer with no clarification and no mutation passes, whatever Stage 1 chose", () => {
    for (const d of ["query", "none", undefined]) {
      const t = turn({ routingDecision: d, reply: "I can't check the weather." });
      expect(assertFixture(weather, [t]).passed).toBe(true);
    }
  });

  test("asking the user to disambiguate fails", () => {
    const t = turn({ routingDecision: "clarify", reply: "Did you mean X or Y?" });
    expect(assertFixture(weather, [t]).passed).toBe(false);
  });

  test("a mutating tool fails", () => {
    const t = turn({ routingDecision: "query", toolsCalled: ["create_node"] });
    expect(assertFixture(weather, [t]).passed).toBe(false);
  });

  test("no reply fails", () => {
    const t = turn({ routingDecision: "query", reply: "(no reply parsed)" });
    expect(assertFixture(weather, [t]).passed).toBe(false);
  });
});
