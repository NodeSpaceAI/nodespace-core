/**
 * Unit tests for assertExpectation() — the function that decides whether an
 * agent-matrix scenario passed.
 *
 * Runs via `bun run test:scripts` (and so under `bun run test:all`). This is a
 * deliberate exception to the project-wide "never use `bun test`" rule: that
 * rule exists so DOM tests cannot bypass the Happy-DOM Vitest config, and this
 * file touches no DOM. It cannot run under Vitest anyway — it imports
 * `bun:test`, and scripts/ is outside every Vitest project glob.
 *
 * It needs no model and no daemon, which is what makes the scoring logic
 * cheap to protect even though the eval itself is manual.
 */

import { describe, expect, test } from "bun:test";
import fixture, {
  actionTools,
  assertExpectation,
  type Expectation,
  type MatrixScenario,
} from "./agent-matrix.ts";
import type { ToolCallRecord, TurnRecord } from "../types.ts";

describe("assertExpectation", () => {
  describe("noTools", () => {
    const expect_: Expectation = { kind: "noTools" };

    test("passes with no action tools", () => {
      expect(assertExpectation(expect_, []).passed).toBe(true);
    });

    test("tolerates search_skills", () => {
      expect(assertExpectation(expect_, ["search_skills"]).passed).toBe(true);
    });

    test("fails with any action tool present", () => {
      const result = assertExpectation(expect_, ["create_node"]);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("create_node");
    });

    test("fails with an action tool even alongside search_skills", () => {
      const result = assertExpectation(expect_, [
        "search_skills",
        "create_node",
      ]);
      expect(result.passed).toBe(false);
    });
  });

  describe("toolOnce", () => {
    const expect_: Expectation = { kind: "toolOnce", tool: "create_node" };

    test("passes with exactly one call", () => {
      expect(assertExpectation(expect_, ["create_node"]).passed).toBe(true);
    });

    test("passes with exactly one call interleaved with search_skills", () => {
      expect(
        assertExpectation(expect_, ["search_skills", "create_node"]).passed,
      ).toBe(true);
    });

    test("fails with zero calls", () => {
      const result = assertExpectation(expect_, []);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("got 0");
    });

    test("fails with multiple calls", () => {
      const result = assertExpectation(expect_, ["create_node", "create_node"]);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("got 2");
    });
  });

  describe("toolOnce minProperties", () => {
    const expect_: Expectation = {
      kind: "toolOnce",
      tool: "create_node",
      minProperties: 1,
    };

    test("fails when the call persisted no property values", () => {
      // The observed production case: create_node called with only `content`
      // and `node_type`, persisting a bare shell that later scenarios then key
      // on and cannot resolve.
      const result = assertExpectation(expect_, ["create_node"], [
        { name: "create_node", isError: false, fieldCount: 0 },
      ]);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("persisted 0 property value(s)");
    });

    test("passes when the call persisted at least the minimum", () => {
      expect(
        assertExpectation(expect_, ["create_node"], [
          { name: "create_node", isError: false, fieldCount: 2 },
        ]).passed,
      ).toBe(true);
    });

    test("treats an absent field count as unknown, not as zero", () => {
      // A results file recorded before create_node reported its property count
      // must not read as a fresh failure.
      expect(
        assertExpectation(expect_, ["create_node"], [
          { name: "create_node", isError: false },
        ]).passed,
      ).toBe(true);
    });

    test("does not apply the minimum to an unrelated tool's calls", () => {
      expect(
        assertExpectation(expect_, ["create_node"], [
          { name: "create_node", isError: false, fieldCount: 3 },
          { name: "search_nodes", isError: false, fieldCount: 0 },
        ]).passed,
      ).toBe(true);
    });

    test("still enforces the count when no minimum is set", () => {
      // Without minProperties the assertion is unchanged: a zero-property call
      // passes, which is the behaviour every other scenario relies on.
      expect(
        assertExpectation({ kind: "toolOnce", tool: "create_node" }, [
          "create_node",
        ], [{ name: "create_node", isError: false, fieldCount: 0 }]).passed,
      ).toBe(true);
    });
  });

  describe("toolSequence", () => {
    const expect_: Expectation = {
      kind: "toolSequence",
      tools: ["search_nodes", "update_node"],
    };

    test("passes when the sequence appears in order", () => {
      expect(
        assertExpectation(expect_, ["search_nodes", "update_node"]).passed,
      ).toBe(true);
    });

    test("passes with other tools interleaved", () => {
      const result = assertExpectation(expect_, [
        "search_skills",
        "search_nodes",
        "search_skills",
        "update_node",
      ]);
      expect(result.passed).toBe(true);
    });

    test("fails when out of order", () => {
      const result = assertExpectation(expect_, [
        "update_node",
        "search_nodes",
      ]);
      expect(result.passed).toBe(false);
    });

    test("fails when a required tool is missing", () => {
      const result = assertExpectation(expect_, ["search_nodes"]);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("search_nodes,update_node");
    });

    test("fails on an empty tool list", () => {
      expect(assertExpectation(expect_, []).passed).toBe(false);
    });

    // Scenario 6's shape: the right tools in the right order still fail the
    // turn if the final call carried nothing to persist.
    describe("minProperties", () => {
      const withMin: Expectation = {
        kind: "toolSequence",
        tools: ["resolve_query", "update_node"],
        minProperties: 1,
      };
      const seq = ["resolve_query", "update_node"];

      test("passes when the last call persisted a property", () => {
        expect(
          assertExpectation(withMin, seq, [
            { name: "update_node", isError: false, fieldCount: 1 },
          ]).passed,
        ).toBe(true);
      });

      test("fails when the sequence is right but nothing was persisted", () => {
        const result = assertExpectation(withMin, seq, [
          { name: "update_node", isError: false, fieldCount: 0 },
        ]);
        expect(result.passed).toBe(false);
        expect(result.failure).toContain("update_node");
      });

      test("checks the last tool by default, not the first", () => {
        // resolve_query reports no fields; the assertion must target
        // update_node rather than passing on the resolver's record.
        const result = assertExpectation(withMin, seq, [
          { name: "resolve_query", isError: false, fieldCount: 5 },
          { name: "update_node", isError: false, fieldCount: 0 },
        ]);
        expect(result.passed).toBe(false);
      });

      test("honours an explicit propertiesOn override", () => {
        const result = assertExpectation(
          { ...withMin, propertiesOn: "resolve_query" },
          seq,
          [
            { name: "resolve_query", isError: false, fieldCount: 0 },
            { name: "update_node", isError: false, fieldCount: 9 },
          ],
        );
        expect(result.passed).toBe(false);
      });

      test("still passes the sequence check when minProperties is unset", () => {
        expect(
          assertExpectation({ kind: "toolSequence", tools: seq }, seq, [
            { name: "update_node", isError: false, fieldCount: 0 },
          ]).passed,
        ).toBe(true);
      });
    });
  });

  describe("noRetry", () => {
    const expect_: Expectation = { kind: "noRetry", tool: "search_nodes" };

    test("passes with no back-to-back repeats", () => {
      expect(assertExpectation(expect_, ["search_nodes"]).passed).toBe(true);
    });

    test("fails on a repeated call", () => {
      const result = assertExpectation(expect_, [
        "search_nodes",
        "search_nodes",
      ]);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("retry loop");
    });

    test("tolerates non-adjacent repeats", () => {
      const result = assertExpectation(expect_, [
        "search_nodes",
        "create_node",
        "search_nodes",
      ]);
      expect(result.passed).toBe(true);
    });

    // Documents the default's blind spot, so the reason `minCalls` exists is
    // pinned rather than implied: with zero calls the run-length loop never
    // executes, so bare noRetry passes a turn in which the tool never fired.
    // That is the read-path failure mode (the model interrogates the user
    // instead of searching), which is why a scenario testing for it must opt in.
    test("bare noRetry passes on zero calls — the reason minCalls exists", () => {
      expect(assertExpectation(expect_, []).passed).toBe(true);
    });

    describe("minCalls", () => {
      const withMin: Expectation = {
        kind: "noRetry",
        tool: "search_nodes",
        minCalls: 1,
      };

      test("fails when the tool never fired", () => {
        const result = assertExpectation(withMin, []);
        expect(result.passed).toBe(false);
        expect(result.failure).toContain("got 0");
      });

      test("fails when only routing tools fired", () => {
        const result = assertExpectation(withMin, ["search_skills"]);
        expect(result.passed).toBe(false);
        expect(result.failure).toContain("got 0");
      });

      test("passes when the tool fired once", () => {
        expect(assertExpectation(withMin, ["search_nodes"]).passed).toBe(true);
      });

      test("still reports a retry loop rather than the count", () => {
        const result = assertExpectation(withMin, [
          "search_nodes",
          "search_nodes",
        ]);
        expect(result.passed).toBe(false);
        expect(result.failure).toContain("retry loop");
      });

      test("counts non-adjacent calls toward the minimum", () => {
        const result = assertExpectation(withMin, [
          "search_nodes",
          "create_node",
          "search_nodes",
        ]);
        expect(result.passed).toBe(true);
      });
    });

    test("search_skills is filtered out before the run-length check, so it does not break up a repeat", () => {
      // search_skills is a routing tool (ROUTING_TOOLS), stripped by actionTools()
      // before noRetry evaluates adjacency — so this is still two search_nodes in a row.
      const result = assertExpectation(expect_, [
        "search_nodes",
        "search_skills",
        "search_nodes",
      ]);
      expect(result.passed).toBe(false);
    });
  });

  describe("noExtraTypes", () => {
    const expect_: Expectation = { kind: "noExtraTypes" };

    test("passes with exactly one create_schema", () => {
      expect(assertExpectation(expect_, ["create_schema"]).passed).toBe(true);
    });

    test("fails with zero create_schema calls", () => {
      const result = assertExpectation(expect_, []);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("got 0");
    });

    test("fails with multiple create_schema calls", () => {
      const result = assertExpectation(expect_, [
        "create_schema",
        "create_schema",
      ]);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("got 2");
    });

    test("tolerates search_skills alongside a single create_schema", () => {
      const result = assertExpectation(expect_, [
        "search_skills",
        "create_schema",
      ]);
      expect(result.passed).toBe(true);
    });
  });

  /**
   * A create_schema call can be counted once and still leave the user with
   * nothing usable. Both cases below scored as PASSES before the outcome of the
   * call was carried alongside its name.
   */
  describe("create_schema outcome", () => {
    const noExtra: Expectation = { kind: "noExtraTypes" };
    const once: Expectation = { kind: "toolOnce", tool: "create_schema" };

    // The observed scenario-3 false pass: a title_template referencing fields
    // that were never defined, rejected by title-template validation.
    test("fails when the single create_schema call was rejected", () => {
      const result = assertExpectation(
        noExtra,
        ["create_schema"],
        [{ name: "create_schema", isError: true }],
      );
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("REJECTED");
    });

    // Distinct from rejection: this call SUCCEEDS. A create_schema carrying
    // neither fields nor description persists a type with no properties.
    test("fails when create_schema succeeded with zero fields", () => {
      const result = assertExpectation(
        noExtra,
        ["create_schema"],
        [{ name: "create_schema", isError: false, fieldCount: 0 }],
      );
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("NO fields");
    });

    test("passes when create_schema persisted fields", () => {
      const result = assertExpectation(
        noExtra,
        ["create_schema"],
        [{ name: "create_schema", isError: false, fieldCount: 4 }],
      );
      expect(result.passed).toBe(true);
    });

    // Scenarios 8a/8b reach create_schema through toolOnce, so the same hole
    // has to be closed on that branch and not just on noExtraTypes.
    test("toolOnce fails on a rejected create_schema", () => {
      const result = assertExpectation(
        once,
        ["create_schema"],
        [{ name: "create_schema", isError: true }],
      );
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("REJECTED");
    });

    test("toolOnce fails on a fieldless create_schema", () => {
      const result = assertExpectation(
        once,
        ["create_schema"],
        [{ name: "create_schema", isError: false, fieldCount: 0 }],
      );
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("NO fields");
    });

    test("toolOnce still reports a wrong call count before outcome", () => {
      const result = assertExpectation(once, [], []);
      expect(result.passed).toBe(false);
      expect(result.failure).toContain("got 0");
    });

    // A baseline recorded before fieldCount existed must not read as a fresh
    // failure: absence is unknown, not zero.
    test("passes when the outcome was never captured", () => {
      expect(assertExpectation(noExtra, ["create_schema"], []).passed).toBe(
        true,
      );
      expect(
        assertExpectation(
          noExtra,
          ["create_schema"],
          [{ name: "create_schema", isError: false }],
        ).passed,
      ).toBe(true);
    });

    // Only create_schema outcomes are judged here; an unrelated failing tool is
    // some other assertion's business.
    test("ignores non-create_schema calls", () => {
      const result = assertExpectation(
        once,
        ["create_schema"],
        [
          { name: "search_nodes", isError: true },
          { name: "create_schema", isError: false, fieldCount: 2 },
        ],
      );
      expect(result.passed).toBe(true);
    });
  });
});

/**
 * A rejected TARGET tool must score red, on every assertion kind.
 *
 * Before this, `isError` was only ever inspected on two narrow paths —
 * `schemaCallsAreSound` (create_schema only) and `callPersistedProperties`
 * (only when a scenario opted into `minProperties`). Every other scenario
 * scored a rejected call green on tool name alone.
 *
 * That is the most likely failure shape for the relationship scenarios
 * specifically: `create_relationship` takes two node ids the model has to
 * recover, and two invented ids are rejected outright — so the one scenario
 * added to measure linking would have reported success on precisely the
 * failure it exists to catch.
 */
describe("a rejected target tool scores red", () => {
  test("toolOnce: create_relationship rejected (11c's shape)", () => {
    const v = assertExpectation(
      { kind: "toolOnce", tool: "create_relationship" },
      ["create_relationship"],
      [{ name: "create_relationship", isError: true }],
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("REJECTED");
  });

  test("toolOnce: a successful call still passes", () => {
    expect(
      assertExpectation(
        { kind: "toolOnce", tool: "create_relationship" },
        ["create_relationship"],
        [{ name: "create_relationship", isError: false }],
      ).passed,
    ).toBe(true);
  });

  test("noRetry: get_related_nodes rejected (11d's shape)", () => {
    const v = assertExpectation(
      { kind: "noRetry", tool: "get_related_nodes", minCalls: 1 },
      ["get_related_nodes"],
      [{ name: "get_related_nodes", isError: true }],
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("REJECTED");
  });

  test("toolSequence: a rejected final tool scores red", () => {
    const v = assertExpectation(
      { kind: "toolSequence", tools: ["resolve_query", "update_node"] },
      ["resolve_query", "update_node"],
      [
        { name: "resolve_query", isError: false },
        { name: "update_node", isError: true },
      ],
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("REJECTED");
  });

  // An empty result is a SUCCESSFUL call — `isError` reflects the executor's
  // own is_error flag, not result emptiness. Scenario 7 is built entirely on a
  // query that legitimately finds nothing, so conflating the two would red out
  // the one scenario whose correct answer is "no matches".
  test("an empty but successful search stays green (scenario 7)", () => {
    expect(
      assertExpectation({ kind: "noRetry", tool: "search_nodes" }, ["search_nodes"], [
        { name: "search_nodes", isError: false },
      ]).passed,
    ).toBe(true);
  });

  // `minProperties` inspects the same `isError` and returns a strictly more
  // specific diagnosis, so it must keep precedence where a scenario opted in.
  test("minProperties keeps its more specific message", () => {
    const v = assertExpectation(
      { kind: "toolOnce", tool: "update_node", minProperties: 1 },
      ["update_node"],
      [{ name: "update_node", isError: true }],
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("never reached storage");
  });

  // Self-correction on the TARGET tool is success, not failure. `noRetry`
  // deliberately tolerates several non-adjacent calls, so a first call rejected
  // for a malformed argument, corrected and retried successfully, is a turn
  // that accomplished what the prompt asked for. This shape was observed live
  // (create_node rejected for a missing node_type, then supplied and persisted).
  test("a rejected call followed by a successful one passes", () => {
    expect(
      assertExpectation(
        { kind: "noRetry", tool: "get_related_nodes", minCalls: 1 },
        ["get_related_nodes"],
        [
          { name: "get_related_nodes", isError: true },
          { name: "get_related_nodes", isError: false },
        ],
      ).passed,
    ).toBe(true);
  });

  // The other half: if EVERY call to the target tool was rejected, the turn
  // ends with nothing written while the tool name still appears in the trace.
  test("every call rejected still fails, and says how many", () => {
    const v = assertExpectation(
      { kind: "noRetry", tool: "create_relationship", minCalls: 1 },
      ["create_relationship"],
      [
        { name: "create_relationship", isError: true },
        { name: "create_relationship", isError: true },
      ],
    );
    expect(v.passed).toBe(false);
    expect(v.failure).toContain("2 times, all REJECTED");
  });

  // Only the TARGET tool is judged: a model that tries a bad search, gets an
  // error, recovers and then does the right thing has still done it.
  test("a rejected NON-target call does not fail the scenario", () => {
    expect(
      assertExpectation(
        { kind: "toolOnce", tool: "create_relationship" },
        ["search_nodes", "create_relationship"],
        [
          { name: "search_nodes", isError: true },
          { name: "create_relationship", isError: false },
        ],
      ).passed,
    ).toBe(true);
  });
});

/**
 * Fixture-level invariants, as distinct from the scoring logic above.
 *
 * These pin properties the eval's validity rests on but which no assertion in
 * `assertExpectation` can see, because they are facts about the SCENARIO SET
 * rather than about one turn's verdict. Each has a failure mode that is silent
 * at run time: the eval still produces a plausible number, and it is wrong.
 */
describe("fixture invariants", () => {
  const all = fixture.groups.flat() as MatrixScenario[];

  /** A turn record carrying just the tool names a scenario would have called. */
  function turn(
    toolsCalled: string[],
    toolCalls?: ToolCallRecord[],
  ): TurnRecord {
    return {
      toolsOffered: toolsCalled.join(","),
      toolsCalled,
      toolCalls,
      reply: "",
      latencyMs: 0,
    };
  }

  // `id` is the key baseline diffing joins on. A duplicate does not throw — the
  // second row silently overwrites the first in any id-keyed comparison, so a
  // scenario's result is attributed to a different scenario.
  test("scenario ids are unique", () => {
    const ids = all.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  // The dev-workflow re-theme changed every prompt's DOMAIN while keeping the
  // mechanics. Preserving the ids is what lets a pre-re-theme baseline still
  // join against a post-re-theme run for the scenarios that carried over;
  // renaming one would read as "scenario removed, scenario added" and silently
  // drop it from the diff instead of showing a domain-driven score change.
  test("the pre-existing scenario ids all survived the re-theme", () => {
    const ids = new Set(all.map((s) => s.id));
    for (const id of [
      "1",
      "2",
      "3",
      "4",
      "5",
      "6",
      "7",
      "8a",
      "8b",
      "8c",
      "8d",
      "8e",
      "9",
      "10a",
      "10b",
      "10c",
    ]) {
      expect(ids.has(id)).toBe(true);
    }
  });

  // A scenario whose target tool is filtered out as a routing tool can never be
  // observed firing, so it is unwinnable by construction and scores a correct
  // model as a failure. `create_relationship`/`get_related_nodes` arrived with
  // the relationship group and are the first target tools added since
  // ROUTING_TOOLS was written — exactly when this can go wrong unnoticed.
  test("no scenario targets a tool that actionTools() strips", () => {
    for (const s of all) {
      const e = s.expect;
      // `noExtraTypes` names no tool in its payload but targets create_schema
      // implicitly — scenario 3 is scored entirely on create_schema's count.
      // Omitting it left the one scenario using that kind uncovered by exactly
      // the regression this test exists for.
      const targets =
        e.kind === "toolOnce" || e.kind === "noRetry"
          ? [e.tool]
          : e.kind === "toolSequence"
            ? e.tools
            : e.kind === "noExtraTypes"
              ? ["create_schema"]
              : [];
      for (const t of targets) {
        expect(actionTools([t])).toEqual([t]);
      }
    }
  });

  // `minProperties` asserts against `fieldCount`, which the tool layer reports
  // only for calls carrying schema FIELD VALUES. create_relationship's payload
  // is two ids and a relation name — none of them field values — so asserting
  // minProperties on it would fail every correct call.
  test("minProperties is never asserted on create_relationship", () => {
    for (const s of all) {
      const e = s.expect;
      if (e.kind === "toolOnce" && e.tool === "create_relationship") {
        expect(e.minProperties).toBeUndefined();
      }
      if (e.kind === "toolSequence") {
        const target = e.propertiesOn ?? e.tools[e.tools.length - 1];
        if (target === "create_relationship") {
          expect(e.minProperties).toBeUndefined();
        }
      }
    }
  });

  // Every scenario is scored through the fixture's own `score()`, not through
  // assertExpectation directly. This checks that wiring end to end for one
  // passing and one failing shape, so a fixture that stopped reading `expect`
  // (or started reading a differently-named field) fails here rather than
  // scoring every scenario identically.
  // Loops over EVERY scenario rather than spot-checking two. An earlier
  // version checked only ids 1 and 11c, which a `score()` that ignored
  // `scenario.expect` and looked expectations up from a hardcoded id table
  // would still pass — the name promised more than the body delivered.
  //
  // For each scenario this synthesizes the turn its own expectation demands
  // and asserts it passes, then asserts a deliberately wrong turn fails. Any
  // `score()` not actually reading each scenario's `expect` fails somewhere in
  // the 20.
  test("score() reads each scenario's own expectation", () => {
    /** The minimal tool sequence that satisfies `e`. */
    function satisfyingTools(e: MatrixScenario["expect"]): string[] {
      switch (e.kind) {
        case "noTools":
          return [];
        case "toolOnce":
        case "noRetry":
          return [e.tool];
        case "toolSequence":
          return [...e.tools];
        case "noExtraTypes":
          return ["create_schema"];
      }
    }

    let checked = 0;
    for (const s of all) {
      const good = satisfyingTools(s.expect);
      // `minProperties` scenarios need a call record carrying field values;
      // `noExtraTypes`/create_schema needs a non-zero fieldCount to be sound.
      const calls = good.map((name) => ({
        name,
        isError: false,
        fieldCount: 2,
      }));
      expect(fixture.score(s, [turn(good, calls)]).passed).toBe(true);

      // A wrong turn for this scenario. Chosen per kind rather than "no tools
      // at all", because a bare `noRetry` without `minCalls` legitimately
      // PASSES on zero calls — its repeat-detecting loop never executes. Using
      // an empty turn as the universal negative would assert the opposite of
      // that documented behaviour and fail here for the wrong reason.
      const bad =
        s.expect.kind === "noTools"
          ? ["create_node"]
          : s.expect.kind === "noRetry"
            ? // A blind retry loop: red whether or not `minCalls` is set.
              [s.expect.tool, s.expect.tool]
            : [];
      expect(fixture.score(s, [turn(bad)]).passed).toBe(false);
      checked++;
    }
    expect(checked).toBe(all.length);
  });
});

// ---------------------------------------------------------------------------
// End-state invariants.
//
// These guard the SCORED half of the fixture. The `expect` invariants above
// now guard a diagnostic, so a scenario could silently stop asserting anything
// real without any of them noticing.
// ---------------------------------------------------------------------------

describe("end-state fixture invariants", () => {
  const all = fixture.groups.flat() as MatrixScenario[];

  // An `end` with no clauses passes every diff, so a scenario carrying one
  // would score green forever while looking fully specified.
  test("every scenario states at least one end-state clause", () => {
    for (const s of all) {
      expect(s.end).toBeDefined();
      const clauses = Object.entries(s.end).filter(([, v]) => v !== undefined);
      expect(clauses.length).toBeGreaterThan(0);
    }
  });

  // `noUnexpectedNodes` is meaningless without something to be unexpected
  // RELATIVE to: on its own it just asserts the turn created nothing at all,
  // which `expectNoWrites` says directly and more legibly.
  test("noUnexpectedNodes is only used alongside a node the scenario expects", () => {
    for (const s of all) {
      if (!s.end.noUnexpectedNodes) continue;
      expect(
        s.end.createdNode !== undefined || s.end.updatedNode !== undefined,
      ).toBe(true);
    }
  });

  // A write scenario asserting nothing about what it wrote would pass on any
  // write at all. Every scenario that is not a pure read must pin something.
  test("every scenario either expects no writes or says what was written", () => {
    for (const s of all) {
      const writes =
        s.end.createdNode !== undefined ||
        s.end.updatedNode !== undefined ||
        s.end.createdSchemas !== undefined ||
        s.end.createdEdge !== undefined;
      expect(s.end.expectNoWrites === true || writes).toBe(true);
    }
  });

  // `expectNoWrites` and a write clause are contradictory: the scenario would
  // be unwinnable, and would look like a model failure rather than a fixture
  // one — the trap this fixture already documents for prompt wording.
  test("no scenario both expects no writes and expects a write", () => {
    for (const s of all) {
      if (!s.end.expectNoWrites) continue;
      expect(s.end.createdNode).toBeUndefined();
      expect(s.end.updatedNode).toBeUndefined();
      expect(s.end.createdSchemas).toBeUndefined();
      expect(s.end.createdEdge).toBeUndefined();
    }
  });

  // An edge expectation that names no relation passes on any turn that merely
  // created a node: a new node had no edges walked in the "before" snapshot,
  // so every edge on it — including one the daemon materialized at creation
  // time — is reported as added. Naming the relation is what keeps the
  // assertion about the link the scenario actually asked for.
  test("every edge expectation names its relation", () => {
    for (const s of all) {
      if (!s.end.createdEdge) continue;
      expect(s.end.createdEdge.relation).toBeDefined();
      expect(s.end.createdEdge.relation).not.toBe("");
    }
  });

  // Setup scenarios are excluded from the denominator, so marking a scored
  // scenario as setup silently removes a measurement. Pin the exact set.
  test("only the state-establishing scenarios are marked as setup", () => {
    const setupIds = all.filter((s) => s.setup).map((s) => s.id).sort();
    expect(setupIds).toEqual(["11a", "11b", "12a", "12b", "12b2", "12c"]);
  });

  // Scenario 13's whole design is that its referent lives ONLY in seeded state.
  // A prompt that named the target incident, its type, or the on-call property
  // would hand the model the answer and silently turn 13 back into the direct
  // string match that cost scenarios 6 and 12 their indirection — the failure
  // would look like a pass.
  //
  // Checks every prompt in the fixture, not just 13's: the leak that matters
  // most is an unrelated scenario mentioning the seeded title, because that is
  // the one nobody would think to look for.
  test("no prompt names scenario 13's seeded incident titles", () => {
    // INSTANCE data: the seeded incident titles. These genuinely reach the
    // model through no channel at all — they are absent from chat history AND
    // from workspace context, which retrieves only `"schema"`-type nodes. A
    // prompt naming one would be the single thing that hands the model the
    // answer, so this list is the real guarantee.
    const neverReachesTheModel = [
      "search index corruption",
      "checkout latency spike",
      "auth token expiry storm",
    ];
    for (const s of all) {
      for (const p of [s.prompt, ...(s.priorTurns ?? [])]) {
        for (const f of neverReachesTheModel) {
          expect(
            p.toLowerCase().includes(f),
            `scenario ${s.id}'s prompt names "${f}", which must exist only in ` +
              `seeded state — naming it in a prompt makes scenario 13's ` +
              `target recoverable without a lookup`,
          ).toBe(false);
        }
      }
    }

    // The on-call VALUE is different in kind: scenario 13's own prompt must
    // name it — that is the reference. What matters is that it identifies the
    // target only via seeded state, so no OTHER scenario may mention it (which
    // would put it in a shared chat's history) and 13 must actually use it.
    const onCall = "rowan";
    for (const s of all) {
      if (s.id === "13") continue;
      for (const p of [s.prompt, ...(s.priorTurns ?? [])]) {
        expect(
          p.toLowerCase().includes(onCall),
          `scenario ${s.id} names the on-call value "${onCall}", which only ` +
            `scenario 13 may reference`,
        ).toBe(false);
      }
    }
    const s13 = all.find((s) => s.id === "13");
    expect(s13).toBeDefined();
    expect(s13!.prompt.toLowerCase()).toContain(onCall);
  });

  // SEPARATE TEST, and separate deliberately: this one is HOUSEKEEPING, not a
  // correctness guarantee, and folding it into the test above would let that
  // test's name promise something the system does not deliver.
  //
  // The seeded schema's type and field names are NOT hidden from the model —
  // workspace context renders the schema into the system prompt, so it sees
  // both. Keeping them out of fixture prompts only keeps it clear which
  // scenario owns that state. Stated explicitly because a future author
  // designing against "the model has never heard of on_call" would be designing
  // against something this fixture cannot provide. See
  // `scenario_13_seeded_schema_reaches_the_prompt_but_its_instances_do_not`
  // in packages/core/src/ops/context_ops.rs.
  test("seeded schema vocabulary is not restated in fixture prompts", () => {
    for (const s of all) {
      for (const p of [s.prompt, ...(s.priorTurns ?? [])]) {
        for (const f of ["incident-report", "on_call"]) {
          expect(
            p.toLowerCase().includes(f),
            `scenario ${s.id}'s prompt names "${f}". The model already sees ` +
              `this via workspace context, so this is not a correctness bug — ` +
              `but naming seeded vocabulary in a prompt muddies which scenario ` +
              `owns that state`,
          ).toBe(false);
        }
      }
    }
  });

  // 13 is the only scenario where A READ is forced: its referent is absent from
  // history, so no direct write can reach the right id. That is the condition
  // under which asserting a read-then-write subsequence is defensible, and
  // pinning it stops a future scenario from reviving `toolSequence` on a turn
  // where a direct write is also correct — the mistake previously removed
  // from both scenario 6 and 12d.
  //
  // Note the limit of the claim, since overstating it is how the previous two
  // attempts went wrong: WHICH read is not forced. `resolve_query` reaches the
  // same node, and a model using it will show as a trajectory mismatch against
  // a passing outcome. That is tolerable only because `expect` is a diagnostic
  // and not the score — under trajectory scoring this assertion would
  // be wrong for the same reason the earlier two were.
  test("toolSequence is asserted only where the route is forced", () => {
    const seqIds = all
      .filter((s) => s.expect.kind === "toolSequence")
      .map((s) => s.id)
      .sort();
    expect(seqIds).toEqual(["13"]);
  });


  // The scenarios whose whole purpose is that a VALUE reached storage. If one
  // of these stops asserting a property, it reverts to passing on a bare shell
  // — the `property_count: 0` shape that reached production.
  test("the value-carrying scenarios assert a persisted property", () => {
    for (const id of ["4", "6", "8c", "10b"]) {
      const s = all.find((x) => x.id === id);
      expect(s).toBeDefined();
      const want = s!.end.createdNode ?? s!.end.updatedNode;
      expect(want).toBeDefined();
      const pins =
        want!.minProperties !== undefined ||
        want!.properties !== undefined ||
        want!.hasPropertyValue !== undefined;
      expect(pins).toBe(true);
    }
  });

  // The graph capability is what makes any of this score. Without it the
  // runner silently falls back to trajectory grading for the whole eval.
  test("the fixture opts into graph grading and scores each scenario's own end state", () => {
    expect(fixture.graph).toBeDefined();
    let checked = 0;
    for (const s of all) {
      // A diff that satisfies this scenario's clauses, built from them.
      const satisfying = {
        addedNodes: s.end.createdNode
          ? [
              {
                id: `new-${s.id}`,
                node_type: s.end.createdNode.type ?? "text",
                content: s.end.createdNode.contentMatches ?? "",
                properties: buildProps(s.end.createdNode),
              },
            ]
          : [],
        addedSchemas:
          s.end.createdSchemas !== undefined
            ? Array.from({ length: s.end.createdSchemas }, (_, i) => `type-${i}`)
            : [],
        addedEdges: s.end.createdEdge
          ? [
              {
                from: "a",
                relation: s.end.createdEdge.relation ?? "mentions",
                to: "b",
              },
            ]
          : [],
        changedNodes: s.end.updatedNode
          ? [
              {
                before: {
                  id: `existing-${s.id}`,
                  node_type: s.end.updatedNode.type ?? "text",
                  content: s.end.updatedNode.contentMatches ?? "",
                  properties: {},
                },
                after: {
                  id: `existing-${s.id}`,
                  node_type: s.end.updatedNode.type ?? "text",
                  content: s.end.updatedNode.contentMatches ?? "",
                  properties: buildProps(s.end.updatedNode),
                },
              },
            ]
          : [],
      };
      expect(fixture.graph!.scoreOutcome(s, satisfying).passed).toBe(true);
      checked++;
    }
    expect(checked).toBe(all.length);
  });

  /** Property values that satisfy a node expectation's clauses. */
  function buildProps(
    want: NonNullable<MatrixScenario["end"]["createdNode"]>,
  ): Record<string, unknown> {
    const props: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(want.properties ?? {})) {
      props[k] = v === true ? "some-value" : v;
    }
    if (want.hasPropertyValue !== undefined) {
      props.some_key = want.hasPropertyValue;
    }
    // Top up to `minProperties` with filler the assertion will count.
    let i = 0;
    while (Object.keys(props).length < (want.minProperties ?? 0)) {
      props[`filler_${i++}`] = "x";
    }
    return props;
  }
});
