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
  constantAnswerBaseline,
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
