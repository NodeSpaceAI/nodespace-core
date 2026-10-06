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
import fixture, { groupSeeds, listedSchemaIds } from "./decisions.ts";

describe("decision fixture assembly", () => {
  test("every scenario gets its own chat, with no turn ahead of it but its own", () => {
    // Two confounds this pins. With all scenarios in one chat, a measured
    // 3-rep run recorded `toolsCalled: []` for every scenario after the
    // fourth: conversation length scored as decision quality. With two setup
    // turns ahead of each scenario, the chats after the first met types that
    // already existed, and each scenario was asked after two failed turns.
    expect(fixture.groups.every((g) => g.length === 1)).toBe(true);
    expect(fixture.groups.flat().some((s) => s.setup === true)).toBe(false);
  });

  const isLinked = (g: (typeof fixture.groups)[number]) =>
    g.some((s) => (s as { linkedSkills?: boolean }).linkedSkills === true);

  test("every chat is seeded with the workspace, and only the linked-skill ones with linked skills", () => {
    // The workspace is set back before each chat: an earlier scenario moved
    // the seeded spec's date, and a later one was scored against the move. Three
    // more skills and a custom type change what retrieval returns for every
    // request, so they go only where they are measured.
    for (const group of fixture.groups) {
      expect(groupSeeds(group)).toEqual(
        isLinked(group) ? ["workspace", "linked-skills"] : ["workspace"],
      );
    }
    expect(fixture.groups.filter(isLinked).length).toBeGreaterThan(0);
  });

  test("seeding runs through the daemon before every chat", () => {
    // `seedGroup` is the hook the runner calls before each chat; a seed that
    // never reached the CLI would leave the chat on whatever came before it.
    const noDaemon = {
      nsBin: "/nonexistent/nodespace",
      socket: "/nonexistent/daemon.sock",
      log: "",
      model: "",
      timeoutMs: 0,
      aichat: "",
    };
    for (const group of fixture.groups) {
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
    const ids = fixture.groups.flat().map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("schema list decoding", () => {
  // The shape `nodespace --json schema list` emits: a `schemas` array of flat
  // schema nodes. A reader of any other shape finds nothing, so the fixture
  // sees an empty workspace and tries to create types that exist.
  const output = {
    count: 2,
    schemas: [
      { id: "task", content: "Task", is_core: true, fields: [{ name: "status", type: "enum" }] },
      {
        id: "feature-spec",
        content: "Feature Spec",
        is_core: false,
        fields: [{ name: "signed_off_date", type: "date" }],
      },
    ],
  };

  test("reads each schema's id", () => {
    expect(listedSchemaIds(output)).toEqual(["task", "feature-spec"]);
  });

  test("an unrecognised shape decodes to no schemas", () => {
    expect(listedSchemaIds(null)).toEqual([]);
    expect(listedSchemaIds({ nodes: [{ id: "task" }] })).toEqual([]);
    expect(listedSchemaIds({ schemas: [{ content: "no id" }] })).toEqual([]);
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
      'I can take that a couple of ways. "Kestrel Gateway" already exists as a feature-spec (nodespace://ks-1).';
    expect(passes(turn(reply, [["create_node", true]]))).toBe(true);
  });

  test("a create that succeeded fails, whatever the reply says", () => {
    // A clarifying reply over a duplicate that landed anyway is still the
    // silent duplicate — the reply cannot undo the write.
    const reply = "I can take that a couple of ways. Kestrel Gateway?";
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
    const t = { ...turn("I can take that a couple of ways. Kestrel Gateway", []) };
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

  test("a must-not-lead skill expectation fails on the named skill, on no leader, and on no skill that can act", () => {
    const single = fixture.groups.flat().find((s) => s.id === "skill-instance-request");
    if (!single) throw new Error("skill-instance-request is missing");
    const led = (selected: string | null, candidates: string[]): TurnRecord =>
      ({
        ...turn("Done.", [["create_node", false]]),
        decisions: [{ kind: "skill", selected, offMenu: false, enforced: false, candidates }],
      }) as TurnRecord;
    const tied = ["Bulk Import", "Organization", "Node Creation"];
    const passes = (selected: string | null, candidates: string[]) =>
      fixture.score(single, [led(selected, candidates)]).passed;

    // Any of the near-tied skills may lead.
    expect(passes("Bulk Import", tied)).toBe(true);
    expect(passes("Node Creation", ["Node Creation", "Bulk Import", "Organization"])).toBe(true);
    // The type skill leading is the failure the scenario is for.
    expect(passes("Schema Creation", ["Schema Creation", "Bulk Import", "Node Creation"])).toBe(
      false,
    );
    // So is the type skill taking the place of the only skill that can create
    // the record: the leader is unchanged and `create_node` is not offered.
    expect(passes("Bulk Import", ["Bulk Import", "Organization", "Schema Creation"])).toBe(false);
    // Graph Editing holds `create_node` too.
    expect(passes("Bulk Import", ["Bulk Import", "Organization", "Graph Editing"])).toBe(true);
    // Nothing clearing its bar is not a pass by default.
    expect(passes(null, tied)).toBe(false);
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

describe("held-turn node outcome scoring", () => {
  const scenario = fixture.groups.flat().find((s) => s.id === "held-off-menu-node");
  if (!scenario) throw new Error("held-off-menu-node is missing");

  const schema = (enforced: boolean, selected: string | null) => ({
    kind: "schema" as const,
    candidates: ["bug-report"],
    selected,
    offMenu: selected !== null && selected !== "bug-report",
    enforced,
  });
  const turn = (decisions: TurnRecord["decisions"], calls: ToolCallRecord[]): TurnRecord =>
    ({
      toolsOffered: "",
      toolsCalled: calls.map((c) => c.name),
      toolCalls: calls,
      decisions,
      reply: "",
      latencyMs: 0,
    }) as TurnRecord;

  const refused: ToolCallRecord = { name: "update_node", isError: true, typeRefused: true };
  const updated: ToolCallRecord = { name: "update_node", isError: false, fieldCount: 1 };
  const searched: ToolCallRecord = { name: "search_nodes", isError: false };

  const verdict = (t: TurnRecord) => fixture.score(scenario, [t]);

  test("the scenario does not name the type its record is, which would put it on the menu", () => {
    expect(scenario.prompt).toContain("Kestrel Gateway");
    expect(scenario.prompt.toLowerCase()).not.toContain("spec");
  });

  test("an update dispatch refused for its node's type, and nothing written, passes", () => {
    expect(verdict(turn([schema(true, "feature-spec")], [refused])).passed).toBe(true);
    expect(verdict(turn([schema(true, "feature-spec")], [refused, searched])).passed).toBe(
      true,
    );
  });

  test("a held turn that made no write passes", () => {
    expect(verdict(turn([schema(true, null)], [searched])).passed).toBe(true);
  });

  test("an update on an off-menu node the daemon reports as run fails", () => {
    const v = verdict(
      turn([schema(true, "feature-spec")], [{ ...updated, offMenuRan: true }]),
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("ran on a held turn");
  });

  test("a write that landed on an offered type's record fails: it is not the record named", () => {
    const v = verdict(turn([schema(true, "feature-spec"), schema(true, "bug-report")], [
      refused,
      updated,
    ]));
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("update_node");
  });

  test("a turn that was not held fails as not held, whatever it did", () => {
    const v = verdict(turn([schema(false, null)], [updated]));
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("was enforced");
  });
});

describe("held-turn outcome scoring", () => {
  const scenario = fixture.groups.flat().find((s) => s.id === "held-off-menu-type");
  if (!scenario) throw new Error("held-off-menu-type is missing");

  type Call = ToolCallRecord;

  const schema = (enforced: boolean, selected: string) => ({
    kind: "schema" as const,
    candidates: ["bug-report"],
    selected,
    offMenu: selected !== "bug-report",
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
    // a bug report, which is on the menu and is not what was asked for.
    const t = turn([schema(true, "task"), schema(true, "bug-report")], [refused, created]);
    const v = verdict(t);
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("after a refusal");
  });

  test("a record created with no refusal fails the same way", () => {
    // The model followed the `enum` on its first call and wrote the reminder
    // as a bug report. Nothing was refused, and the record is still wrong.
    const v = verdict(turn([schema(true, "bug-report")], [created]));
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("not after a refusal");
  });

  test("a create that wrote nothing is not a record created", () => {
    // Skipped for a route_clarify in the same round, or answered by the
    // duplicate-write guard: not an error, and nothing persisted.
    const skipped: Call = { name: "create_node", isError: false };
    expect(verdict(turn([schema(true, "bug-report")], [skipped])).passed).toBe(true);
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

    const open = fixture.score(control, [turn([schema(false, "bug-report")], [created])]);
    expect(open.passed).toBe(false);
    expect(open.failure).toContain("nothing here was measured");

    // Held, the control creates the record: that is what it is for.
    const held = fixture.score(control, [turn([schema(true, "bug-report")], [created])]);
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
    const clean = turn([schema(true, "bug-report")], [created]);
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
    "The schemas available are: feature\\_spec, planning\\_cycle, person, project, date, text, and task.";

  test("a reply naming built-in and custom types off a successful search passes", () => {
    expect(passes(turn(everyType, [["search_nodes", false]]))).toBe(true);
  });

  test("a reply naming only the custom types, with no tool called, fails", () => {
    // Custom types only, and no tool called.
    expect(passes(turn("The schemas we currently have are 'feature-spec' and 'planning-cycle'.", []))).toBe(false);
  });

  test("a complete reply with no successful search fails", () => {
    // Naming the right types without having looked is a pass for the wrong
    // reason: the next workspace has different ones.
    expect(passes(turn(everyType, []))).toBe(false);
    expect(passes(turn(everyType, [["search_nodes", true]]))).toBe(false);
  });

  test("a search whose reply leaves a built-in type out fails", () => {
    const reply = "Found 21 node types, including built-in ones like 'text' and custom schemas such as 'feature-spec'.";
    expect(passes(turn(reply, [["search_nodes", false]]))).toBe(false);
  });

  test("a type is matched as a word, not inside another", () => {
    // `task` must not be satisfied by a type named `subtask-list`.
    const reply = "The schemas are: feature-spec, person, project, subtask-list.";
    expect(passes(turn(reply, [["search_nodes", false]]))).toBe(false);
  });

  test("tools called with no recorded outcomes fails loudly", () => {
    const t = { ...turn(everyType, []) };
    t.toolsCalled = ["search_nodes"];
    delete t.toolCalls;
    expect(passes(t)).toBe(false);
  });
});
