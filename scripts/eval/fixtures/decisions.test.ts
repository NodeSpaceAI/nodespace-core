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
import type { ToolCallRecord, TurnRecord } from "../types.ts";
import fixture, { listedSchemas, setupTypePresent } from "./decisions.ts";

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
  });

  // An env whose CLI does not exist: any seeding attempt throws.
  const noDaemon = {
    nsBin: "/nonexistent/nodespace",
    socket: "/nonexistent/daemon.sock",
    log: "",
    model: "",
    timeoutMs: 0,
    aichat: "",
  };
  const isLinked = (g: (typeof fixture.groups)[number]) =>
    g.some((s) => (s as { linkedSkills?: boolean }).linkedSkills === true);

  test("group seeding touches only the linked-skill groups", () => {
    // Three more skills and one more custom type change what retrieval returns
    // for every request. Seeded for any other group, they would change what
    // that group's scenario is scored against.
    for (const group of fixture.groups.filter((g) => !isLinked(g))) {
      expect(() => fixture.seedGroup?.(noDaemon, group)).not.toThrow();
    }
    const linked = fixture.groups.filter(isLinked);
    expect(linked.length).toBeGreaterThan(0);
    for (const group of linked) {
      expect(() => fixture.seedGroup?.(noDaemon, group)).toThrow();
    }
  });

  test("the linked-skill groups run last", () => {
    // Groups share the rep's database, so whatever a group seeds is there for
    // every group after it.
    const firstLinked = fixture.groups.findIndex(isLinked);
    expect(firstLinked).toBeGreaterThan(0);
    expect(fixture.groups.slice(firstLinked).every(isLinked)).toBe(true);
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

describe("schema list decoding", () => {
  // The shape `nodespace --json schema list` emits: a `schemas` array of flat
  // schema nodes. A reader of any other shape finds nothing, so the fixture
  // sees an empty workspace, tries to create types that exist, and reports a
  // setup turn's state as missing.
  const output = {
    count: 2,
    schemas: [
      { id: "task", content: "Task", is_core: true, fields: [{ name: "status", type: "enum" }] },
      {
        id: "company_sold_to",
        content: "Company Sold To",
        is_core: false,
        fields: [{ name: "signed_date", type: "date" }],
      },
    ],
  };

  test("reads each schema's id, core flag and fields", () => {
    expect(listedSchemas(output)).toEqual([
      { id: "task", isCore: true, fields: [{ name: "status", type: "enum" }] },
      {
        id: "company_sold_to",
        isCore: false,
        fields: [{ name: "signed_date", type: "date" }],
      },
    ]);
  });

  test("an unrecognised shape decodes to no schemas", () => {
    expect(listedSchemas(null)).toEqual([]);
    expect(listedSchemas({ nodes: [{ id: "task" }] })).toEqual([]);
    expect(listedSchemas({ schemas: [{ content: "no id" }] })).toEqual([]);
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

describe("outcome and no-tool scoring", () => {
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

  test("a turn the system searched for passes the search outcome", () => {
    // The first round replied in prose and the loop ran the search itself; it
    // still ran before the user saw a reply, which is what this scores.
    const searches = fixture.groups
      .flat()
      .find((s) => s.id === "outcome-question-searches-first");
    if (!searches) throw new Error("outcome-question-searches-first is missing");
    const score = (t: TurnRecord) => fixture.score(searches, [t]).passed;

    expect(score(turn("Here is what I found.", [["search_semantic", false]]))).toBe(true);
    expect(score(turn("Here is what I found.", [["search_nodes", false]]))).toBe(true);
    expect(score(turn("Could you provide more context?", []))).toBe(false);
    // Reading a node is not searching for one.
    expect(score(turn("It says so here.", [["get_node", false]]))).toBe(false);
  });

  test("a no-tool expectation covers the whole turn, not its first round", () => {
    // A first round with no call is recorded as "selected nothing", but the
    // loop can go on to run a search for it. That turn called a tool.
    const noTool = fixture.groups
      .flat()
      .find((s) => s.id === "op-own-skills-no-search");
    if (!noTool) throw new Error("op-own-skills-no-search is missing");
    const firstRoundCalledNothing = {
      kind: "operation",
      selected: null,
      candidates: ["search_semantic", "search_nodes", "get_node"],
      offMenu: false,
    };
    const withDecision = (t: TurnRecord): TurnRecord =>
      ({ ...t, decisions: [firstRoundCalledNothing] }) as TurnRecord;

    expect(
      fixture.score(noTool, [withDecision(turn("I have eleven skills.", []))]).passed,
    ).toBe(true);
    expect(
      fixture.score(noTool, [
        withDecision(turn("No such skill exists.", [["search_semantic", false]])),
      ]).passed,
    ).toBe(false);
  });

  test("a reply outcome is scored on the reply alone", () => {
    const named = fixture.groups.flat().find((s) => s.id === "outcome-find-own-skill");
    if (!named) throw new Error("outcome-find-own-skill is missing");
    const score = (t: TurnRecord) => fixture.score(named, [t]).passed;

    // Whether the turn searched first is not what is scored.
    expect(score(turn("That would be Research & Search.", []))).toBe(true);
    expect(
      score(turn("That would be Research & Search.", [["search_semantic", false]])),
    ).toBe(true);
    expect(score(turn("I couldn't find a specific skill.", [["search_semantic", false]]))).toBe(
      false,
    );
    // The reported failure with one word changed must not pass on that word.
    expect(
      score(turn("I researched your notes and couldn't find a specific skill.", [])),
    ).toBe(false);
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

describe("held-turn outcome scoring", () => {
  const scenario = fixture.groups.flat().find((s) => s.id === "held-off-menu-type");
  if (!scenario) throw new Error("held-off-menu-type is missing");

  type Call = ToolCallRecord;

  const schema = (enforced: boolean, selected: string) => ({
    kind: "schema" as const,
    candidates: ["warranty_claim"],
    selected,
    offMenu: selected !== "warranty_claim",
    enforced,
  });
  const turn = (decisions: TurnRecord["decisions"], calls: Call[]): TurnRecord =>
    ({
      toolsOffered: "",
      toolsCalled: calls.map((c) => c.name),
      toolCalls: calls,
      decisions,
      reply: "",
      latencyMs: 0,
    }) as TurnRecord;

  // The calls a turn can make, as the scrape records them.
  const refused: Call = { name: "create_node", isError: true, typeRefused: true };
  const created: Call = { name: "create_node", isError: false, fieldCount: 2 };
  const searched: Call = { name: "search_nodes", isError: false };

  const verdict = (t: TurnRecord) => fixture.score(scenario, [t]);

  test("an off-menu call that dispatch refused, and nothing written, passes", () => {
    expect(verdict(turn([schema(true, "task")], [refused])).passed).toBe(true);
  });

  test("a read after the refusal still passes", () => {
    expect(verdict(turn([schema(true, "task")], [refused, searched])).passed).toBe(true);
  });

  test("an identical re-send that no guard dispatched is not a call that ran", () => {
    // The model repeats the refused call. The round records a second off-menu
    // decision, and the duplicate-call guard stops the round before dispatch:
    // one refusal, two decisions, nothing ran.
    const t = turn([schema(true, "task"), schema(true, "task")], [refused]);
    expect(verdict(t).passed).toBe(true);
  });

  test("an off-menu call the daemon reports as run fails", () => {
    const t = turn(
      [schema(true, "task")],
      [{ name: "create_node", isError: false, fieldCount: 1, offMenuRan: true }],
    );
    const v = verdict(t);
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("ran on a held turn");
  });

  test("a record created after the refusal fails: it is the wrong type", () => {
    // The user asked for a task. Refused, the model re-sent the same record as
    // a warranty claim, which is on the menu and is not what was asked for.
    const t = turn([schema(true, "task"), schema(true, "warranty_claim")], [refused, created]);
    const v = verdict(t);
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("after a refusal");
  });

  test("a record created with no refusal fails the same way", () => {
    // The model followed the `enum` on its first call and wrote the reminder
    // as a warranty claim. Nothing was refused, and the record is still wrong.
    const v = verdict(turn([schema(true, "warranty_claim")], [created]));
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("not after a refusal");
  });

  test("a create that wrote nothing is not a record created", () => {
    // Skipped for a route_clarify in the same round, or answered by the
    // duplicate-write guard: not an error, and nothing persisted.
    const skipped: Call = { name: "create_node", isError: false };
    expect(verdict(turn([schema(true, "warranty_claim")], [skipped])).passed).toBe(true);
  });

  test("a turn that was not held fails as unmeasured, not as a model failure", () => {
    const v = verdict(turn([schema(false, "task")], [created]));
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("nothing here was measured");
  });

  test("the control fails as unmeasured on a turn that was not held", () => {
    // Naming the linked type on an open turn stays on the menu, and says
    // nothing about a held one.
    const control = fixture.groups.flat().find((s) => s.id === "held-on-menu-type");
    if (!control) throw new Error("held-on-menu-type is missing");

    const open = fixture.score(control, [turn([schema(false, "warranty_claim")], [created])]);
    expect(open.passed).toBe(false);
    expect(open.failure).toContain("nothing here was measured");

    // Held, the control creates the record: that is what it is for.
    const held = fixture.score(control, [turn([schema(true, "warranty_claim")], [created])]);
    expect(held.passed).toBe(true);
  });

  test("what followed a refusal is recorded", () => {
    const after = (rest: Call[], reply = "") => {
      const t = turn([schema(true, "task")], [refused, ...rest]);
      t.reply = reply;
      return fixture.extra?.(scenario, [t]).afterRefusal;
    };

    expect(after([created])).toBe("created");
    expect(after([{ name: "route_clarify", isError: false }])).toBe("clarified");
    expect(after([], "I can take that a couple of ways. Which did you mean?")).toBe("clarified");
    expect(after([searched], "I could not add a task.")).toBe("replied");
    // A create that persisted nothing is not a write.
    expect(after([{ name: "create_node", isError: false }], "Done.")).toBe("replied");
    // Nothing refused: nothing to report.
    const clean = turn([schema(true, "warranty_claim")], [created]);
    expect(fixture.extra?.(scenario, [clean]).afterRefusal).toBeNull();
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
