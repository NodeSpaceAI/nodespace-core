/**
 * Structural invariants of the decision fixture.
 *
 * Runs via `bun run test:scripts` (and so under `bun run test:all`). This is a
 * deliberate exception to the project-wide "never use `bun test`" rule: that
 * rule exists so DOM tests cannot bypass the Happy-DOM Vitest config, and this
 * file touches no DOM. It cannot run under Vitest anyway — it imports
 * `bun:test`, and scripts/ is outside every Vitest project glob.
 *
 * Needs no model and no daemon: these assert how the fixture is ASSEMBLED, not
 * how the model behaves. That is what makes them worth having — the eval
 * itself is manual and slow, so a fixture defect otherwise surfaces only after
 * a multi-minute run, disguised as a model result.
 */

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { TurnRecord } from "../types.ts";
import fixture, { setupTypePresent } from "./decisions.ts";

describe("decision fixture assembly", () => {
  test("every scored scenario gets its own group", () => {
    // The confound this split exists to remove: with all scenarios in one
    // group they shared a chat, and a measured 3-rep run recorded
    // `toolsCalled: []` for every scenario after the fourth — including ones
    // unrelated to the change under test, which pass in isolation. That is
    // conversation length being scored as decision quality.
    const scoredPerGroup = fixture.groups.map(
      (g) => g.filter((s) => s.setup !== true).length,
    );
    expect(scoredPerGroup.every((n) => n === 1)).toBe(true);
  });

  test("every group carries the full setup prefix", () => {
    // Each group is a fresh chat, so a group missing its setup turns would run
    // its scenario against a workspace with no types to select among — which
    // scores as a model failure rather than the fixture fault it is.
    for (const group of fixture.groups) {
      const setupIds = group.filter((s) => s.setup === true).map((s) => s.id);
      expect(setupIds).toEqual(["setup-company", "setup-venue"]);
    }
  });

  test("setup turns precede the scored scenario in every group", () => {
    for (const group of fixture.groups) {
      const firstScored = group.findIndex((s) => s.setup !== true);
      const lastSetup = group.map((s) => s.setup === true).lastIndexOf(true);
      expect(lastSetup).toBeLessThan(firstScored);
    }
  });

  test("instance seeding is run-scoped, not group-scoped", () => {
    // The defect this pins cost three measured 3-rep runs, each of which
    // looked like a model result.
    //
    // `--between-runs` wipes the database between reps, and `seedGroup` fires
    // inside the group loop — so the first group of every rep sees a workspace
    // whose types its own setup turns have not created yet. A seed hung on
    // `seedGroup` finds nothing, silently no-ops, and every scenario scores
    // against a workspace with no instance in it.
    //
    // `seedRun` fires once per rep, after the wipe and before any group, which
    // is the only point where "every scenario in this rep has the instance" is
    // expressible. Moving this back to `seedGroup` reintroduces a failure that
    // is invisible in the results.
    expect(typeof fixture.seedRun).toBe("function");
    expect(fixture.seedGroup).toBeUndefined();
  });

  test("scenario ids are unique across groups", () => {
    // Results join on id across reps; a duplicate silently merges two
    // scenarios' verdicts.
    const ids = fixture.groups
      .flatMap((g) => g.filter((s) => s.setup !== true))
      .map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("setup state", () => {
  test("every setup scenario names the type it establishes", () => {
    // A setup scenario without it cannot be checked, so a turn that rightly
    // creates nothing would exclude its whole group again.
    const setups = fixture.groups[0].filter((s) => s.setup === true);
    expect(setups.map((s) => s.establishes)).toEqual(["company", "venue"]);
  });

  test("a type is present under whatever id the model gave it", () => {
    expect(setupTypePresent("company", ["company_sold_to"])).toBe(true);
    expect(setupTypePresent("company", ["client_company"])).toBe(true);
    expect(setupTypePresent("venue", ["event_location"])).toBe(true);
    expect(setupTypePresent("venue", ["event_place"])).toBe(true);
  });

  test("the other setup type does not stand in for a missing one", () => {
    expect(setupTypePresent("venue", ["company_sold_to"])).toBe(false);
    expect(setupTypePresent("company", ["event_location"])).toBe(false);
    // A venue named after its clients is still the venue type.
    expect(setupTypePresent("company", ["client_venue"])).toBe(false);
    expect(setupTypePresent("venue", ["client_venue"])).toBe(true);
  });

  test("the match is by word, so an unrelated type sharing one reads as present", () => {
    // The limit of matching on a hint rather than an id: a later scenario's
    // `event_log` would stand in for the venue type. The ids the setup turns
    // produce are not predictable, so the match cannot be tighter.
    expect(setupTypePresent("venue", ["event_log"])).toBe(true);
    expect(setupTypePresent("company", ["customer_feedback"])).toBe(true);
  });

  test("an empty workspace has neither", () => {
    expect(setupTypePresent("company", [])).toBe(false);
    expect(setupTypePresent("venue", [])).toBe(false);
  });
});

describe("duplicate-entity outcome scoring", () => {
  const scenario = fixture.groups
    .flat()
    .find((s) => s.id === "schema-on-menu-company");
  if (!scenario) throw new Error("schema-on-menu-company is missing");

  const turn = (reply: string, calls: [string, boolean][]): TurnRecord =>
    ({
      toolsOffered: "",
      toolsCalled: calls.map(([name]) => name),
      toolCalls: calls.map(([name, isError]) => ({ name, isError })),
      reply,
      latencyMs: 0,
    }) as TurnRecord;

  const passes = (t: TurnRecord) => fixture.score(scenario, [t]).passed;

  test("a refused create followed by a clarification naming the record passes", () => {
    const reply =
      'I can take that a couple of ways. "Northwind Trading" already exists as a company_sold_to (nodespace://nw-1).';
    expect(passes(turn(reply, [["create_node", true]]))).toBe(true);
  });

  test("a create that succeeded fails, whatever the reply says", () => {
    // A clarifying reply over a duplicate that landed anyway is still the
    // silent duplicate — the reply cannot undo the write.
    const reply = "I can take that a couple of ways. Northwind Trading?";
    expect(passes(turn(reply, [["create_node", false]]))).toBe(false);
  });

  test("acting on the existing record passes", () => {
    expect(
      passes(turn("Updated it.", [["create_node", true], ["update_node", false]])),
    ).toBe(true);
  });

  test("tools called with no recorded outcomes fails loudly", () => {
    // Without per-call outcomes a landed create is indistinguishable from a
    // refused one; reading absence as "none succeeded" would pass a duplicate.
    const t = { ...turn("I can take that a couple of ways. Northwind Trading", []) };
    t.toolsCalled = ["create_node"];
    delete t.toolCalls;
    expect(passes(t)).toBe(false);
  });

  test("neither asking nor acting fails", () => {
    expect(passes(turn("It already exists.", [["create_node", true]]))).toBe(false);
  });

  test("the clarification opener matches the agent's", () => {
    // The scorer recognises a clarification by its opening words. Drift from
    // the agent's constant would fail every clarified turn as "not asked" —
    // a harness defect that reads as the guard regressing.
    const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");
    const pattern = /const CLARIFICATION_OPENER(?::\s*&str)?\s*=\s*"([^"]+)"/;
    const agent = read("../../../packages/agent/src/local_agent/agent_loop.rs").match(pattern);
    const scorer = read("./decisions.ts").match(pattern);
    expect(agent?.[1]).toBeDefined();
    expect(scorer?.[1]).toBe(agent?.[1]);
  });
});

describe("type-listing outcome scoring", () => {
  const scenario = fixture.groups.flat().find((s) => s.id === "outcome-list-types");
  if (!scenario) throw new Error("outcome-list-types is missing");

  const turn = (reply: string, calls: [string, boolean][]): TurnRecord =>
    ({
      toolsOffered: "",
      toolsCalled: calls.map(([name]) => name),
      toolCalls: calls.map(([name, isError]) => ({ name, isError })),
      reply,
      latencyMs: 0,
    }) as TurnRecord;

  const passes = (t: TurnRecord) => fixture.score(scenario, [t]).passed;

  const everyType =
    "The schemas available are: company\\_sold\\_to, venue, person, project, date, text, and task.";

  test("a reply naming built-in and custom types off a successful search passes", () => {
    expect(passes(turn(everyType, [["search_nodes", false]]))).toBe(true);
  });

  test("the reported reply, read from the context block, fails", () => {
    // Custom types only, and no tool called.
    expect(passes(turn("The schemas we currently have are 'company_sold_to' and 'venue'.", []))).toBe(false);
  });

  test("a complete reply with no successful search fails", () => {
    // Naming the right types without having looked is a pass for the wrong
    // reason: the next workspace has different ones.
    expect(passes(turn(everyType, []))).toBe(false);
    expect(passes(turn(everyType, [["search_nodes", true]]))).toBe(false);
  });

  test("a search whose reply leaves a built-in type out fails", () => {
    const reply = "Found 21 node types, including built-in ones like 'text' and custom schemas such as 'company_sold_to'.";
    expect(passes(turn(reply, [["search_nodes", false]]))).toBe(false);
  });

  test("a type is matched as a word, not inside another", () => {
    // `task` must not be satisfied by a type named `subtask_list`.
    const reply = "The schemas are: company_sold_to, person, project, subtask_list.";
    expect(passes(turn(reply, [["search_nodes", false]]))).toBe(false);
  });

  test("tools called with no recorded outcomes fails loudly", () => {
    const t = { ...turn(everyType, []) };
    t.toolsCalled = ["search_nodes"];
    delete t.toolCalls;
    expect(passes(t)).toBe(false);
  });
});
