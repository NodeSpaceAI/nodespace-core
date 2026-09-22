/**
 * Unit tests for runner.ts's pure scoring/reporting helpers — the uniformity
 * guard, empty-generation exclusion, raw-output trace assembly, and the
 * multi-rep aggregation (pass^k, flip rate, guidance-drift detection) that
 * `--runs` reports from.
 *
 * Runs via `bun run test:scripts` (and so under `bun run test:all`). Deliberate
 * exception to the project-wide "never use `bun test`" rule: this file touches
 * no DOM, and cannot run under Vitest anyway (imports `bun:test`, and scripts/
 * is outside every Vitest project glob).
 *
 * These specifically exist because the diagnostic behavior they protect —
 * "fail loudly on impossible uniformity" and "exclude degenerate empty
 * generations from the denominator" — is itself the thing that stops a false
 * result from looking like a real one. A bug in the guard is exactly the kind
 * of harness defect this eval exists to prevent, so it needs a test that runs
 * with no model and no daemon.
 */

import { describe, expect, test } from "bun:test";
import {
  aggregateReps,
  buildTraceLines,
  checkGuidanceDrift,
  checkUniformity,
  formatReliabilityTable,
  markerFor,
  parseTurnOutput,
  partitionExcluded,
  readBaselineReliability,
} from "./runner.ts";
import { assertExpectation } from "./fixtures/agent-matrix.ts";
import { EnvironmentError } from "./preflight.ts";
import type { ScenarioResult } from "./types.ts";

function result(overrides: Partial<ScenarioResult> = {}): ScenarioResult {
  return {
    id: "1",
    scenario: "test scenario",
    prompt: "hi",
    passed: true,
    turns: [
      {
        toolsOffered: "",
        toolsCalled: [],
        reply: "hello",
        latencyMs: 100,
      },
    ],
    ...overrides,
  };
}

describe("partitionExcluded", () => {
  test("scored results with no exclusions pass through unchanged", () => {
    const results = [result({ id: "1" }), result({ id: "2", passed: false })];
    const { scored, excludedCount } = partitionExcluded(results);
    expect(scored).toHaveLength(2);
    expect(excludedCount).toBe(0);
  });

  test("excludes scenarios flagged as empty generations", () => {
    const results = [
      result({ id: "1" }),
      result({ id: "2", excludedAsEmptyGeneration: true, passed: false }),
      result({ id: "3" }),
    ];
    const { scored, excludedCount } = partitionExcluded(results);
    expect(scored.map((r) => r.id)).toEqual(["1", "3"]);
    expect(excludedCount).toBe(1);
  });

  test("all-excluded run scores zero total, not zero-of-N", () => {
    const results = [
      result({ id: "1", excludedAsEmptyGeneration: true, passed: false }),
      result({ id: "2", excludedAsEmptyGeneration: true, passed: false }),
    ];
    const { scored, excludedCount } = partitionExcluded(results);
    expect(scored).toHaveLength(0);
    expect(excludedCount).toBe(2);
  });

  // Setup scenarios establish state for later ones and are not observations.
  // Scoring them let one ambiguous verb in a setup turn cost three points:
  // itself, plus the two successors it left with nothing to act on.
  test("excludes scenarios flagged as fixture setup", () => {
    const results = [
      result({ id: "11a", excludedAsSetup: true }),
      result({ id: "11b", excludedAsSetup: true }),
      result({ id: "11c" }),
    ];
    const { scored, setupCount } = partitionExcluded(results);
    expect(scored.map((r) => r.id)).toEqual(["11c"]);
    expect(setupCount).toBe(2);
  });

  // The two exclusions mean opposite things — an empty generation is a fault
  // whose rate is itself a result, a setup turn is a fixed part of the fixture
  // — so collapsing them would hide a rising empty-generation rate.
  test("counts setup and empty-generation exclusions separately", () => {
    const results = [
      result({ id: "1", excludedAsEmptyGeneration: true, passed: false }),
      result({ id: "11a", excludedAsSetup: true }),
      result({ id: "2" }),
    ];
    const { scored, excludedCount, setupCount } = partitionExcluded(results);
    expect(scored.map((r) => r.id)).toEqual(["2"]);
    expect(excludedCount).toBe(1);
    expect(setupCount).toBe(1);
  });

  // A setup turn that also came back an empty generation must not be counted
  // twice — the totals would stop adding up to the number of scenarios run.
  test("a setup scenario that was also an empty generation counts once", () => {
    const results = [
      result({
        id: "11a",
        excludedAsSetup: true,
        excludedAsEmptyGeneration: true,
        passed: false,
      }),
      result({ id: "11c" }),
    ];
    const { scored, excludedCount, setupCount } = partitionExcluded(results);
    expect(scored.map((r) => r.id)).toEqual(["11c"]);
    expect(excludedCount).toBe(1);
    expect(setupCount).toBe(0);
  });
});

describe("markerFor", () => {
  test("renders pass, fail, setup and exclusion distinctly", () => {
    expect(markerFor({ passed: true })).toBe("✓");
    expect(markerFor({ passed: false })).toBe("✗");
    expect(markerFor({ passed: true, excludedAsSetup: true })).toBe("⊙");
    expect(markerFor({ passed: false, excludedAsEmptyGeneration: true })).toBe("⊘");
  });

  // The two call sites used to duplicate this precedence and disagree on it.
  // Empty generation wins, matching partitionExcluded, which counts such a
  // scenario in the exclusion bucket whose rate is itself a result.
  test("empty generation outranks setup", () => {
    expect(
      markerFor({
        passed: false,
        excludedAsSetup: true,
        excludedAsEmptyGeneration: true,
      }),
    ).toBe("⊘");
  });
});

describe("checkUniformity", () => {
  test("below the minimum scenario count, never flags — even 0/1 or 1/1", () => {
    expect(checkUniformity(0, 1)).toBeNull();
    expect(checkUniformity(1, 1)).toBeNull();
    expect(checkUniformity(0, 3)).toBeNull();
    expect(checkUniformity(3, 3)).toBeNull();
  });

  test("a genuine partial result at or above the minimum is not flagged", () => {
    expect(checkUniformity(2, 4)).toBeNull();
    expect(checkUniformity(9, 12)).toBeNull();
  });

  test("all-failed at or above the minimum is flagged as an environment error", () => {
    const err = checkUniformity(0, 4);
    expect(err).toBeInstanceOf(EnvironmentError);
    expect(err?.message).toContain("FAILED");
    expect(err?.message).toContain("0/4");
  });

  test("all-passed at or above the minimum is flagged as an environment error", () => {
    const err = checkUniformity(12, 12);
    expect(err).toBeInstanceOf(EnvironmentError);
    expect(err?.message).toContain("PASSED");
    expect(err?.message).toContain("12/12");
  });

  test("a custom minimum is honored", () => {
    expect(checkUniformity(0, 2, 2)).toBeInstanceOf(EnvironmentError);
    expect(checkUniformity(0, 2, 3)).toBeNull();
  });

  test("total of zero never divides or flags", () => {
    expect(checkUniformity(0, 0)).toBeNull();
  });
});

describe("buildTraceLines", () => {
  test("produces no lines when no turn captured raw output", () => {
    const results = [result({ id: "1" }), result({ id: "2" })];
    expect(buildTraceLines(results)).toEqual([]);
  });

  test("includes only turns that captured raw output, in order", () => {
    const results = [
      result({
        id: "1",
        turns: [
          {
            toolsOffered: "",
            toolsCalled: ["create_node"],
            reply: "done",
            latencyMs: 50,
            rawOutput: "[iteration 0] create_node(...)",
            routingDecision: "query",
            stage2CandidatesInjected: true,
          },
        ],
      }),
      result({ id: "2" }), // no rawOutput — contributes nothing
    ];
    const lines = buildTraceLines(results);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatchObject({
      rep: 1,
      scenarioId: "1",
      turnIndex: 0,
      isPriorContext: false,
      rawOutput: "[iteration 0] create_node(...)",
      routingDecision: "query",
      stage2CandidatesInjected: true,
      toolsCalled: ["create_node"],
    });
  });

  test("marks all but the last turn as prior context", () => {
    const results = [
      result({
        id: "1",
        turns: [
          {
            toolsOffered: "",
            toolsCalled: [],
            reply: "context turn",
            latencyMs: 10,
            rawOutput: "raw 0",
          },
          {
            toolsOffered: "",
            toolsCalled: [],
            reply: "scored turn",
            latencyMs: 20,
            rawOutput: "raw 1",
          },
        ],
      }),
    ];
    const lines = buildTraceLines(results);
    expect(lines).toHaveLength(2);
    expect(lines[0].isPriorContext).toBe(true);
    expect(lines[1].isPriorContext).toBe(false);
  });

  // A multi-rep trace is only useful if a line says which rep it came from —
  // "what did the model say the time it failed" is the question reps exist to
  // let you ask.
  test("tags each line with the rep it came from", () => {
    const results = [
      result({
        id: "1",
        turns: [
          {
            toolsOffered: "",
            toolsCalled: [],
            reply: "r",
            latencyMs: 10,
            rawOutput: "raw",
          },
        ],
      }),
    ];
    expect(buildTraceLines(results, 3)[0].rep).toBe(3);
  });
});

describe("aggregateReps × setup exclusion", () => {
  // The seam between outcome grading (which introduced setup scenarios) and
  // --runs (which introduced pass^k). A setup scenario counted here would put
  // the cascade back one level up: a flaky setup turn would be reported as an
  // unreliable SCENARIO, when what it actually means is that its successors'
  // reps are not comparable.
  test("setup scenarios are excluded from the pass^k denominator", () => {
    const agg = aggregateReps([
      [result({ id: "11a", excludedAsSetup: true }), result({ id: "11c" })],
      [result({ id: "11a", excludedAsSetup: true }), result({ id: "11c" })],
    ]);
    expect(agg.scoredScenarios).toBe(1);
    expect(agg.passAtK).toBe(1);
  });

  test("a setup scenario is marked, not reported as never scored", () => {
    const agg = aggregateReps([
      [result({ id: "11a", excludedAsSetup: true, passed: false })],
      [result({ id: "11a", excludedAsSetup: true, passed: true })],
    ]);
    const s = agg.scenarios.find((x) => x.id === "11a")!;
    expect(s.setup).toBe(true);
    expect(s.scoredReps).toBe(0);
    // A setup turn that passed in one rep and failed in another is NOT a flip
    // worth reporting — it was never an observation in the first place.
    expect(s.flipped).toBe(false);
    expect(agg.flipped).toBe(0);
  });

  // It must render as setup rather than as a degenerate empty generation,
  // which would name an inference bug that did not happen.
  test("the reliability table renders a setup scenario as setup", () => {
    const agg = aggregateReps([
      [result({ id: "11a", excludedAsSetup: true }), result({ id: "11c" })],
    ]);
    const lines = formatReliabilityTable(agg);
    const setupLine = lines.find((l) => l.includes("11a"))!;
    expect(setupLine).toContain("⊙");
    expect(setupLine).toContain("fixture setup");
    expect(setupLine).not.toContain("empty generation");
  });

  // A setup scenario and a genuinely-excluded one both have scoredReps === 0
  // and must stay distinguishable.
  test("setup and all-excluded scenarios render differently", () => {
    const agg = aggregateReps([
      [
        result({ id: "11a", excludedAsSetup: true }),
        result({ id: "5", excludedAsEmptyGeneration: true, passed: false }),
      ],
    ]);
    const lines = formatReliabilityTable(agg);
    expect(lines.find((l) => l.includes("11a"))).toContain("fixture setup");
    expect(lines.find((l) => l.includes(" 5 "))).toContain("empty generation");
  });
});

describe("aggregateReps", () => {
  test("a single rep reports pass^1 == pass^k and no flips", () => {
    const agg = aggregateReps([
      [result({ id: "1" }), result({ id: "2", passed: false })],
    ]);
    expect(agg.reps).toBe(1);
    expect(agg.scoredScenarios).toBe(2);
    expect(agg.passAtK).toBe(1);
    expect(agg.passAt1).toBe(1);
    expect(agg.flipped).toBe(0);
  });

  test("a scenario passing in every rep counts toward pass^k", () => {
    const agg = aggregateReps([
      [result({ id: "1" })],
      [result({ id: "1" })],
      [result({ id: "1" })],
    ]);
    expect(agg.passAtK).toBe(1);
    expect(agg.scenarios[0]).toMatchObject({
      scoredReps: 3,
      passedReps: 3,
      passedAll: true,
      flipped: false,
    });
  });

  // The motivating case from the issue: 2/3 is not a pass, and it is a
  // different finding from 0/3. A single run renders both identically.
  test("a scenario that flips is excluded from pass^k and flagged", () => {
    const agg = aggregateReps([
      [result({ id: "1" })],
      [result({ id: "1", passed: false })],
      [result({ id: "1" })],
    ]);
    expect(agg.passAtK).toBe(0);
    expect(agg.flipped).toBe(1);
    expect(agg.scenarios[0]).toMatchObject({
      scoredReps: 3,
      passedReps: 2,
      passedAll: false,
      flipped: true,
    });
  });

  test("a scenario failing in every rep is a hard fail, not a flip", () => {
    const agg = aggregateReps([
      [result({ id: "1", passed: false })],
      [result({ id: "1", passed: false })],
    ]);
    expect(agg.passAtK).toBe(0);
    expect(agg.flipped).toBe(0);
    expect(agg.scenarios[0]).toMatchObject({ passedAll: false, flipped: false });
  });

  test("pass^1 is the mean of the per-rep scores, not a per-scenario rate", () => {
    // rep 1 scores 2/2, rep 2 scores 0/2 → mean 1.0 out of 2 scenarios.
    const agg = aggregateReps([
      [result({ id: "1" }), result({ id: "2" })],
      [result({ id: "1", passed: false }), result({ id: "2", passed: false })],
    ]);
    expect(agg.passAt1).toBe(1);
    expect(agg.passAtK).toBe(0);
    expect(agg.flipped).toBe(2);
  });

  // An inference bug is not evidence either way about the scenario, so an
  // excluded rep must not make a reliable scenario read as a flipping one.
  test("an excluded rep leaves the denominator, not the numerator", () => {
    const agg = aggregateReps([
      [result({ id: "1" })],
      [result({ id: "1", excludedAsEmptyGeneration: true, passed: false })],
      [result({ id: "1" })],
    ]);
    expect(agg.passAtK).toBe(1);
    expect(agg.flipped).toBe(0);
    expect(agg.scenarios[0]).toMatchObject({
      scoredReps: 2,
      passedReps: 2,
      excludedReps: 1,
      passedAll: true,
    });
  });

  test("a scenario excluded in every rep is counted as neither pass nor fail", () => {
    const agg = aggregateReps([
      [
        result({ id: "1" }),
        result({ id: "2", excludedAsEmptyGeneration: true, passed: false }),
      ],
      [
        result({ id: "1" }),
        result({ id: "2", excludedAsEmptyGeneration: true, passed: false }),
      ],
    ]);
    // Scenario 2 was never scored: it leaves the denominator entirely rather
    // than counting as a failure that no measurement supports.
    expect(agg.scoredScenarios).toBe(1);
    expect(agg.passAtK).toBe(1);
    expect(agg.scenarios).toHaveLength(2);
    expect(agg.scenarios[1]).toMatchObject({
      scoredReps: 0,
      excludedReps: 2,
      passedAll: false,
      flipped: false,
    });
  });

  // A rep that aborted partway contributes fewer scenarios than the others.
  // Each scenario's reliability must then be judged against the reps that
  // actually reached it, not against the run's rep count — otherwise every
  // scenario after the abort point reads as unreliable because of where the
  // run stopped rather than because of how the model behaved.
  test("scenarios present in only some reps are judged on the reps that reached them", () => {
    const agg = aggregateReps([
      [result({ id: "1" }), result({ id: "2" })],
      [result({ id: "1" })], // rep 2 stopped after scenario 1
      [result({ id: "1" }), result({ id: "2", passed: false })],
    ]);
    expect(agg.reps).toBe(3);
    expect(agg.scenarios[0]).toMatchObject({ scoredReps: 3, passedReps: 3, passedAll: true });
    expect(agg.scenarios[1]).toMatchObject({ scoredReps: 2, passedReps: 1, flipped: true });
    expect(agg.passAtK).toBe(1);
    // pass^1 averages the per-rep scored counts (2, 1, 1) — it is a mean of
    // what each rep actually scored, not a rate over the run's scenario list.
    expect(agg.passAt1).toBeCloseTo(4 / 3, 5);
  });

  test("keeps fixture order and preserves scenario descriptions", () => {
    const agg = aggregateReps([
      [
        result({ id: "b", scenario: "second" }),
        result({ id: "a", scenario: "first" }),
      ],
      [result({ id: "a", scenario: "first" })],
    ]);
    expect(agg.scenarios.map((s) => s.id)).toEqual(["b", "a"]);
    expect(agg.scenarios[0].scenario).toBe("second");
  });

  test("no reps at all aggregates to zeroes rather than throwing", () => {
    const agg = aggregateReps([]);
    expect(agg).toMatchObject({
      reps: 0,
      scoredScenarios: 0,
      passAtK: 0,
      passAt1: 0,
      flipped: 0,
    });
  });
});

describe("checkGuidanceDrift", () => {
  const guidance = {
    skill: [
      { key: "Research & Search", version: "d64c01a0" },
      { key: "Node Creation", version: "7d5db44b" },
    ],
  };

  test("identical guidance does not drift", () => {
    expect(checkGuidanceDrift(guidance, structuredClone(guidance), 2)).toBeNull();
  });

  // Node query order is not a guarantee, so a reordered readback must not read
  // as a rebuilt daemon — that would abort correct runs.
  test("the same entries in a different order do not drift", () => {
    const reordered = {
      skill: [
        { key: "Node Creation", version: "7d5db44b" },
        { key: "Research & Search", version: "d64c01a0" },
      ],
    };
    expect(checkGuidanceDrift(guidance, reordered, 2)).toBeNull();
  });

  test("a changed content version is drift", () => {
    const changed = {
      skill: [
        { key: "Research & Search", version: "CHANGED" },
        { key: "Node Creation", version: "7d5db44b" },
      ],
    };
    const err = checkGuidanceDrift(guidance, changed, 3);
    expect(err).toBeInstanceOf(EnvironmentError);
    expect(err?.message).toContain("rep 3");
  });

  test("an added or removed seeded node is drift", () => {
    const fewer = { skill: [{ key: "Node Creation", version: "7d5db44b" }] };
    expect(checkGuidanceDrift(guidance, fewer, 2)).toBeInstanceOf(
      EnvironmentError,
    );
  });

  test("both sides absent is not drift", () => {
    expect(checkGuidanceDrift(undefined, undefined, 2)).toBeNull();
  });

  test("guidance appearing where there was none is drift", () => {
    expect(checkGuidanceDrift(undefined, guidance, 2)).toBeInstanceOf(
      EnvironmentError,
    );
  });
});

describe("formatReliabilityTable", () => {
  test("a single rep renders plain pass/fail, not an N/N tally", () => {
    const agg = aggregateReps([
      [result({ id: "1" }), result({ id: "2", passed: false })],
    ]);
    const lines = formatReliabilityTable(agg);
    expect(lines[0]).toContain("✓ 1  pass");
    expect(lines[1]).toContain("✗ 2  fail");
    expect(lines.join("\n")).not.toContain("reps");
  });

  // The finding the issue is about: "fails 3/3" and "fails 1/3" must not
  // render identically.
  test("distinguishes a standing failure from a flipping one", () => {
    const agg = aggregateReps([
      [result({ id: "always" }), result({ id: "never", passed: false }), result({ id: "flaky" })],
      [result({ id: "always" }), result({ id: "never", passed: false }), result({ id: "flaky", passed: false })],
      [result({ id: "always" }), result({ id: "never", passed: false }), result({ id: "flaky" })],
    ]);
    const lines = formatReliabilityTable(agg);
    expect(lines[0]).toBe("  ✓ always  3/3 reps");
    expect(lines[1]).toBe("  ✗ never  0/3 reps");
    expect(lines[2]).toBe("  ~ flaky  2/3 reps  ← FLIPPED");
  });

  test("reports excluded reps alongside the tally", () => {
    const agg = aggregateReps([
      [result({ id: "1" })],
      [result({ id: "1", excludedAsEmptyGeneration: true, passed: false })],
    ]);
    expect(formatReliabilityTable(agg)[0]).toBe("  ✓ 1  1/1 reps · 1 excluded");
  });

  test("a scenario excluded in every rep says so rather than reading as a fail", () => {
    const agg = aggregateReps([
      [result({ id: "1", excludedAsEmptyGeneration: true, passed: false })],
      [result({ id: "1", excludedAsEmptyGeneration: true, passed: false })],
    ]);
    expect(formatReliabilityTable(agg)[0]).toBe(
      "  ⊘ 1  excluded in all 2 rep(s) — never scored",
    );
  });

  test("a single-rep exclusion does not render as 'all 1 rep(s)'", () => {
    const agg = aggregateReps([
      [result({ id: "1", excludedAsEmptyGeneration: true, passed: false })],
    ]);
    expect(formatReliabilityTable(agg)[0]).toBe(
      "  ⊘ 1  excluded (degenerate empty generation) — never scored",
    );
  });
});

describe("readBaselineReliability", () => {
  test("reads a multi-rep baseline by pass^k", () => {
    const baseline = {
      reps: [
        { rep: 1, results: [result({ id: "1" }), result({ id: "2" })] },
        { rep: 2, results: [result({ id: "1" }), result({ id: "2", passed: false })] },
      ],
    };
    const base = readBaselineReliability(baseline);
    expect(base?.reps).toBe(2);
    expect(base?.byId.get("1")?.passedAll).toBe(true);
    expect(base?.byId.get("2")?.passedAll).toBe(false);
  });

  // Baselines recorded before --runs existed carry a flat `results` array and
  // must still join, or every pre-existing baseline reads as "all removed".
  test("reads a pre-reps baseline as a single rep", () => {
    const baseline = {
      results: [result({ id: "1" }), result({ id: "2", passed: false })],
    };
    const base = readBaselineReliability(baseline);
    expect(base?.reps).toBe(1);
    expect(base?.byId.get("1")?.passedAll).toBe(true);
    expect(base?.byId.get("2")?.passedAll).toBe(false);
  });

  test("returns null for a file carrying neither shape", () => {
    expect(readBaselineReliability({ eval: "agent-matrix" })).toBeNull();
    expect(readBaselineReliability(null)).toBeNull();
  });
});

describe("parseTurnOutput", () => {
  test("parses tool calls, reply, and latency from ordinary output", () => {
    const out = [
      "[tools offered] create_node, search_nodes",
      "[tool] create_node [fields=3] {}",
      "assistant> Done — created the node.",
    ].join("\n");
    const turn = parseTurnOutput(out, 1234);
    // The marker was already present in this fixture but never asserted, which
    // is part of why an always-empty `toolsOffered` reached committed traces
    // unnoticed.
    expect(turn.toolsOffered).toBe("create_node, search_nodes");
    expect(turn.toolsCalled).toEqual(["create_node"]);
    expect(turn.toolCalls).toEqual([
      { name: "create_node", isError: false, fieldCount: 3 },
    ]);
    expect(turn.reply).toBe("Done — created the node.");
    expect(turn.latencyMs).toBe(1234);
  });

  test("preserves a multiline raw generation intact (regression)", () => {
    const text = "line one\nline two\nline three";
    const out = `[raw] iteration=0 ${JSON.stringify(text)}\nassistant> ok`;
    const turn = parseTurnOutput(out, 100);
    expect(turn.rawOutput).toBe(`[iteration 0] ${text}`);
  });

  test("does not truncate raw output containing a literal [tool] marker (regression)", () => {
    const text = "I'll use [tool] create_node to do that.";
    const out = [
      `[raw] iteration=0 ${JSON.stringify(text)}`,
      "[tool] create_node [fields=1] {}",
      "assistant> done",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.rawOutput).toBe(`[iteration 0] ${text}`);
    // The real tool call after it must still be parsed, not swallowed.
    expect(turn.toolsCalled).toEqual(["create_node"]);
  });

  test("joins multiple iterations of raw output in order", () => {
    const out = [
      `[raw] iteration=0 ${JSON.stringify("first\nresponse")}`,
      `[raw] iteration=1 ${JSON.stringify("second response")}`,
      "assistant> done",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.rawOutput).toBe(
      "[iteration 0] first\nresponse\n[iteration 1] second response",
    );
  });

  test("rawOutput is undefined when no [raw] lines are present", () => {
    const turn = parseTurnOutput("assistant> hi", 100);
    expect(turn.rawOutput).toBeUndefined();
  });

  test("parses routing decision and stage2 injection markers", () => {
    const out = ["[routing] query", "[stage2 injected] true", "assistant> ok"].join(
      "\n",
    );
    const turn = parseTurnOutput(out, 100);
    expect(turn.routingDecision).toBe("query");
    expect(turn.stage2CandidatesInjected).toBe(true);
  });

  test("parses the routed-skills marker", () => {
    const out = [
      "[routed skills] equipment_log, checkout_flow",
      "assistant> ok",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.routedSkills).toBe("equipment_log, checkout_flow");
  });

  test("routedSkills is undefined (not empty) when the marker is absent", () => {
    // "not recorded" must stay distinguishable from "nothing routed" — a turn
    // from an older results file has no marker at all, and reporting that as
    // an empty routed set would assert something the run never observed.
    const turn = parseTurnOutput("assistant> hi", 100);
    expect(turn.routedSkills).toBeUndefined();
  });

  test("parses stage2 injected as false, distinct from absent", () => {
    const out = ["[stage2 injected] false", "assistant> ok"].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.stage2CandidatesInjected).toBe(false);
  });

  test("marks emptyGeneration from the marker", () => {
    const out = ["[empty-generation]", "assistant> (no assistant reply)"].join(
      "\n",
    );
    const turn = parseTurnOutput(out, 100);
    expect(turn.emptyGeneration).toBe(true);
  });

  test("emptyGeneration is undefined (not false) when the marker is absent", () => {
    const turn = parseTurnOutput("assistant> hi", 100);
    expect(turn.emptyGeneration).toBeUndefined();
  });

  // The marker carries one JSON object. These cover this parser's own
  // behaviour; the emitter → scrape → parse contract is pinned mechanically by
  // decision-roundtrip.test.ts, which parses log lines the real Rust tracing
  // layer emitted rather than strings written here by hand.
  test("parses both decision kinds with their candidate sets", () => {
    const out = [
      '[decision operation] {"candidates":["create_node","search_nodes"],"off_menu":false,"selected":"create_node"}',
      '[decision schema] {"candidates":["invoice","customer"],"off_menu":false,"selected":"invoice"}',
      "assistant> done",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.decisions).toEqual([
      {
        kind: "operation",
        selected: "create_node",
        offMenu: false,
        candidates: ["create_node", "search_nodes"],
      },
      {
        kind: "schema",
        selected: "invoice",
        offMenu: false,
        candidates: ["invoice", "customer"],
      },
    ]);
  });

  test("parses an unselected decision as null", () => {
    // ADR-056's Scenario 6 shape, the failure class the decision record exists
    // to make visible. JSON null carries it directly, so unlike the delimited
    // form there is no sentinel spelling a scorer has to know.
    const out = [
      '[decision operation] {"candidates":["search_nodes","update_node"],"off_menu":false,"selected":null}',
      "assistant> I'd be happy to help.",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.decisions?.[0].selected).toBeNull();
    expect(turn.decisions?.[0].candidates).toEqual([
      "search_nodes",
      "update_node",
    ]);
  });

  test("keeps an empty selection distinct from no selection", () => {
    // Two different outcomes: `null` is declining to pick, `""` is a name the
    // model produced that happens to be blank. Folding them together would
    // erase the difference.
    const out = [
      '[decision schema] {"candidates":["invoice"],"off_menu":true,"selected":""}',
      "assistant> done",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.decisions?.[0].selected).toBe("");
    expect(turn.decisions?.[0].selected).not.toBeNull();
  });

  test("parses a multi-word skill selection intact (regression)", () => {
    const out = [
      '[decision skill] {"candidates":["Schema Creation","Node Creation"],"off_menu":false,"selected":"Schema Creation"}',
      "assistant> done",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.decisions?.[0].kind).toBe("skill");
    expect(turn.decisions?.[0].selected).toBe("Schema Creation");
    expect(turn.decisions?.[0].candidates).toEqual([
      "Schema Creation",
      "Node Creation",
    ]);
  });

  test("a candidate name containing a comma stays one candidate", () => {
    // The bug this format was introduced for: the delimited form split this
    // name in two, producing a corrupted record that reads as a valid one.
    const out = [
      '[decision schema] {"candidates":["Company, Sold To","invoice"],"off_menu":false,"selected":"Company, Sold To"}',
      "assistant> done",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.decisions?.[0].candidates).toEqual([
      "Company, Sold To",
      "invoice",
    ]);
    expect(turn.decisions?.[0].selected).toBe("Company, Sold To");
  });

  test("carries the off-menu flag without it leaking into the selection", () => {
    const out = [
      '[decision schema] {"candidates":["invoice","customer"],"off_menu":true,"selected":"album"}',
      "assistant> created",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.decisions?.[0].selected).toBe("album");
    expect(turn.decisions?.[0].offMenu).toBe(true);
  });

  test("drops a malformed payload rather than recording an empty decision", () => {
    // A record with no candidates and no selection reads downstream as
    // "offered nothing, picked nothing" — a real and meaningfully different
    // outcome from a line that failed to parse.
    const out = ["[decision schema] {not json}", "assistant> hi"].join("\n");
    expect(parseTurnOutput(out, 100).decisions).toBeUndefined();
  });

  // Valid JSON carrying an unexpected shape is dropped for the same reason a
  // parse failure is. Coercing it — defaulting a missing array to [], filtering
  // non-strings out of one — manufactures a plausible record no consumer can
  // tell from a real one: the eval reads `selected === null` as "nothing
  // cleared its bar" and reports `candidates` as what retrieval offered, so it
  // would score a shape mismatch as a model failure with a wrong diagnostic.
  // Unreachable from the current emitter; these pin the boundary check that
  // keeps that guarantee honest rather than assumed.
  test("drops a payload whose candidates are not an array", () => {
    const out = [
      '[decision operation] {"candidates":"search_nodes","off_menu":false,"selected":"search_nodes"}',
      "assistant> hi",
    ].join("\n");
    expect(parseTurnOutput(out, 100).decisions).toBeUndefined();
  });

  test("drops an empty payload rather than reading it as a phantom decision", () => {
    // `{}` would otherwise coerce to "offered nothing, picked nothing" —
    // indistinguishable from the real turn that made exactly that decision.
    const out = ["[decision operation] {}", "assistant> hi"].join("\n");
    expect(parseTurnOutput(out, 100).decisions).toBeUndefined();
  });

  test("drops a payload with a non-string candidate rather than shortening the list", () => {
    // Filtering the stray entry out would silently shorten the candidate list,
    // which is the precise corruption the JSON encoding replaced.
    const out = [
      '[decision schema] {"candidates":["invoice",42],"off_menu":false,"selected":"invoice"}',
      "assistant> hi",
    ].join("\n");
    expect(parseTurnOutput(out, 100).decisions).toBeUndefined();
  });

  test("drops a payload whose off_menu is missing", () => {
    const out = [
      '[decision schema] {"candidates":["invoice"],"selected":"invoice"}',
      "assistant> hi",
    ].join("\n");
    expect(parseTurnOutput(out, 100).decisions).toBeUndefined();
  });

  test("keeps the valid decisions on a turn where one payload is malformed", () => {
    // A single bad line must not discard the turn's other records, which are
    // independently scoreable.
    const out = [
      '[decision skill] {"candidates":["Node Creation"],"off_menu":false,"selected":"Node Creation"}',
      "[decision schema] {}",
      '[decision operation] {"candidates":["create_node"],"off_menu":false,"selected":"create_node"}',
      "assistant> done",
    ].join("\n");
    const decisions = parseTurnOutput(out, 100).decisions;
    expect(decisions).toHaveLength(2);
    expect(decisions?.map((d) => d.kind)).toEqual(["skill", "operation"]);
  });

  test("decisions is undefined (not empty) when no marker is present", () => {
    // Absence means "this build did not record them", which a stale baseline
    // must not be scored against as though the model decided nothing.
    const turn = parseTurnOutput("assistant> hi", 100);
    expect(turn.decisions).toBeUndefined();
  });

  test("does not read a decision marker the model narrated in its own reply", () => {
    // Same anchoring hazard every other marker here guards: `out` carries raw
    // model text, and a model quoting a marker-shaped string must not be
    // mistaken for the harness's own signal.
    const out =
      'assistant> I logged it as [decision schema] {"candidates":[],"off_menu":false,"selected":"album"}';
    const turn = parseTurnOutput(out, 100);
    expect(turn.decisions).toBeUndefined();
  });

  test("keeps an empty candidate set from collapsing into a phantom candidate", () => {
    const out = [
      '[decision operation] {"candidates":[],"off_menu":false,"selected":null}',
      "assistant> hi",
    ].join("\n");
    const turn = parseTurnOutput(out, 100);
    expect(turn.decisions?.[0].candidates).toEqual([]);
  });
});

describe("agent-matrix minProperties scoring", () => {
  // The scenario-9 assertion exists to score a state-change that never reached
  // storage as RED. Both realistic post-fix outcomes previously scored green:
  // a gate-rejected call was skipped as `isError`, and a content-only write was
  // skipped because the tool reports no field count for it. A scenario that
  // passes whether or not the model does the right thing cannot detect the
  // regression it was added for.
  const expect9 = {
    kind: "toolOnce" as const,
    tool: "update_node",
    minProperties: 1,
  };

  test("a rejected update_node scores red, not skipped", () => {
    const verdict = assertExpectation(expect9, ["update_node"], [
      { name: "update_node", isError: true },
    ]);
    expect(verdict.passed).toBe(false);
    expect(verdict.failure).toContain("rejected");
  });

  test("a content-only update scores red, not skipped", () => {
    const verdict = assertExpectation(expect9, ["update_node"], [
      { name: "update_node", isError: false, contentOnly: true },
    ]);
    expect(verdict.passed).toBe(false);
    expect(verdict.failure).toContain("only content");
  });

  test("an update that persisted the property scores green", () => {
    const verdict = assertExpectation(expect9, ["update_node"], [
      { name: "update_node", isError: false, fieldCount: 1 },
    ]);
    expect(verdict.passed).toBe(true);
  });

  test("an update reporting zero persisted properties scores red", () => {
    const verdict = assertExpectation(expect9, ["update_node"], [
      { name: "update_node", isError: false, fieldCount: 0 },
    ]);
    expect(verdict.passed).toBe(false);
  });

  test("parses the content-only marker alongside the error and fields markers", () => {
    const turn = parseTurnOutput(
      "[tool] update_node [content-only] {}\nassistant> ok",
      100,
    );
    expect(turn.toolCalls?.[0]).toEqual({
      name: "update_node",
      isError: false,
      contentOnly: true,
    });
  });
});

// -- uniformity guard vs. a legitimate full pass -------------------------
//
// The guard's original premise ("real runs have never been perfectly uniform")
// held only while outcome scoring mis-read type-keyed properties and suppressed
// passes on correct writes. Once that was fixed a capable model passed
// everything and the guard discarded the run, writing no results file. These
// pin BOTH directions: a varied full pass is believed, and the degenerate
// shapes the guard exists for are still caught.

describe("checkUniformity: full pass with tool diversity", () => {
  test("believes a full pass that called varied tools", () => {
    expect(checkUniformity(21, 21, undefined, 6)).toBeNull();
  });

  test("still flags a full pass that called almost nothing", () => {
    // Every turn passing while the model barely acted is the shape a broken
    // environment produces - e.g. negative assertions scoring green because
    // every send died before inference.
    expect(checkUniformity(21, 21, undefined, 1)).not.toBeNull();
    expect(checkUniformity(21, 21, undefined, 0)).not.toBeNull();
  });

  test("still flags a uniform ZERO regardless of diversity", () => {
    // A model calling many tools and failing every scenario is the "same
    // unhandled code path" case, so diversity must not excuse it.
    expect(checkUniformity(0, 21, undefined, 6)).not.toBeNull();
  });

  test("omitted diversity keeps the strict pre-existing behaviour", () => {
    expect(checkUniformity(21, 21)).not.toBeNull();
  });

  // Straddle MIN_DISTINCT_TOOLS_FOR_REAL_PASS rather than testing only far from
  // it: with cases at 0/1/6 alone the constant could be changed to 4 and every
  // test would still pass, so the threshold would not actually be pinned.
  test("accepts a full pass at exactly the threshold", () => {
    expect(checkUniformity(21, 21, undefined, 3)).toBeNull();
  });

  test("flags a full pass one below the threshold", () => {
    expect(checkUniformity(21, 21, undefined, 2)).not.toBeNull();
  });

  test("a partial result is unaffected either way", () => {
    expect(checkUniformity(17, 21, undefined, 6)).toBeNull();
    expect(checkUniformity(17, 21, undefined, 0)).toBeNull();
  });
});
