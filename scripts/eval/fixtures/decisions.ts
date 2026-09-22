/**
 * Decision eval — scores the two selections the agent makes per round directly,
 * rather than inferring them from whether the scenario as a whole passed.
 *
 *   schema     — which entity type the turn acts on
 *   operation  — which tool the turn calls
 *
 * Why this exists separately from the agent-matrix and routing evals: those
 * score end-to-end outcomes, and ADR-056 states plainly that such scores
 * "describe the harness as much as the model". Both of that ADR's own E4B
 * failures are decision failures — a wrong operation (`execute_query` where
 * `search_nodes` was wanted) and a turn that stopped after the read without the
 * write — and neither is separable from the scenario result containing it. A
 * scenario can also pass for the wrong reason: the right end state reached via a
 * tool nobody would have chosen. This fixture scores the choice itself.
 *
 * What it does NOT do: gate, threshold, or change behaviour. ADR-038 warns
 * against "inventing a third number"; the candidate sets scored here are the
 * ones the pipeline already computed and the outcomes are the ones the model
 * already produced (see `local_agent::decisions`).
 *
 * Scenario wording must stay independent of packages/agent/src/agent_guidance.rs;
 * `guidance_is_not_contaminated_by_eval_prompts` parses the `prompt:` literals
 * out of this file and fails the build if guidance reproduces one.
 */

import type { EvalFixture, Scenario, TurnRecord, Verdict } from "../types.ts";

// ---------------------------------------------------------------------------
// Expectation model
//
// A scenario asserts on ONE decision. Asserting both at once would make a
// failure ambiguous — the whole point of separating them is to tell "picked the
// wrong tool" apart from "picked the wrong type".
// ---------------------------------------------------------------------------

type Expected =
  /** The turn's first operation must be one of these. */
  | { decision: "operation"; oneOf: string[] }
  /** The turn must act on this type. */
  | { decision: "schema"; type: string }
  /**
   * The turn must NOT act on any of these types.
   *
   * The disambiguation shape: several types plausibly match the words in the
   * message, and naming the wrong one is the failure. Expressed as a denial
   * rather than an assertion because more than one answer can be defensible
   * while a specific one is clearly wrong.
   */
  | { decision: "schema"; notOneOf: string[] };

interface DecisionScenario extends Scenario {
  expected: Expected;
  /**
   * Reproduces a failure ADR-056 records against the locked model. These are
   * the cases with a known-bad baseline, so a regression here is unambiguous.
   */
  adr056?: boolean;
  /**
   * Several types in the workspace plausibly match the message's wording — the
   * case the agent has no principled mechanism for today.
   */
  ambiguous?: boolean;
}

// ---------------------------------------------------------------------------
// Workspace setup
//
// Every scenario runs against a workspace carrying several overlapping types,
// because a decision is only scoreable when there was something to decide
// between: a single-schema workspace makes every schema selection trivially
// correct and measures nothing.
//
// Setup turns are marked `setup` so they are recorded but not scored — a
// failure to create a type would otherwise crater every scenario after it and
// count one failure many times.
// ---------------------------------------------------------------------------

const SETUP: DecisionScenario[] = [
  {
    id: "setup-customer",
    scenario: "Setup: customer type",
    prompt:
      "Set up a way to track the companies we sell to, with a name and the date we signed them.",
    setup: true,
    expected: { decision: "operation", oneOf: ["create_schema"] },
  },
  {
    id: "setup-project",
    scenario: "Setup: project type",
    prompt:
      "Set up a way to track pieces of delivery work, each with a name, a target finish date and a status.",
    setup: true,
    expected: { decision: "operation", oneOf: ["create_schema"] },
  },
];

const FIXTURES: DecisionScenario[] = [
  // ── Operation selection ────────────────────────────────────────────────
  {
    id: "op-list-by-type",
    scenario: "Operation: listing instances of a type",
    // ADR-056 Scenario 5: `execute_query` was called where `search_nodes` was
    // wanted. The two tools were later collapsed into one, so the historical
    // failure cannot recur by that name — this scores the surviving choice.
    prompt: "Which delivery projects are still open?",
    expected: { decision: "operation", oneOf: ["search_nodes"] },
    adr056: true,
  },
  {
    id: "op-read-then-write",
    scenario: "Operation: a state change on a node named indirectly",
    // ADR-056 Scenario 6: the read fired and the write never did. Scored on the
    // FIRST operation being a resolution step rather than a blind write — the
    // turn-completion half is what the matrix eval already covers.
    prompt: "The website redesign work is finished now.",
    expected: {
      decision: "operation",
      oneOf: ["search_nodes", "resolve_query", "update_node"],
    },
    adr056: true,
  },
  {
    id: "op-plain-question",
    scenario: "Operation: a question answerable without touching the graph",
    // A turn that should call nothing. Scored because "picked no tool" is a
    // real decision, and an agent that reaches for a tool on every message is
    // failing a decision even when the reply reads fine.
    prompt: "What kinds of things can you help me keep track of?",
    expected: { decision: "operation", oneOf: [] },
  },

  // ── Schema selection ───────────────────────────────────────────────────
  {
    id: "schema-direct-customer",
    scenario: "Schema: unambiguous customer reference",
    prompt: "Add Northwind Trading as a company we sell to.",
    expected: { decision: "schema", type: "customer" },
  },
  {
    id: "schema-direct-project",
    scenario: "Schema: unambiguous project reference",
    prompt: "Add a delivery job called Website Redesign, due in March.",
    expected: { decision: "schema", type: "project" },
  },
  {
    id: "schema-field-disambiguates",
    scenario: "Schema: the named field settles which type is meant",
    // Only one of the two types has a target finish date, so the mention of
    // moving a deadline is decisive — the disambiguating signal is structural
    // (which type even has that field), not semantic similarity.
    prompt: "Push the Northwind target finish date out by two weeks.",
    expected: { decision: "schema", notOneOf: ["customer"] },
    ambiguous: true,
  },
  {
    id: "schema-shared-name-signed",
    scenario: "Schema: shared name, customer-only attribute",
    // The mirror of the case above: the same bare name, but the attribute
    // mentioned exists only on the customer type.
    prompt: "When did we sign Northwind?",
    expected: { decision: "schema", notOneOf: ["project"] },
    ambiguous: true,
  },
];

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/** The first decision of the given kind across the turn's rounds. */
function firstDecision(turns: TurnRecord[], kind: "schema" | "operation") {
  for (const t of turns) {
    const hit = t.decisions?.find((d) => d.kind === kind);
    if (hit) return hit;
  }
  return undefined;
}

function assertFixture(
  fixture: DecisionScenario,
  turns: TurnRecord[],
): Verdict {
  const { expected } = fixture;
  const decision = firstDecision(turns, expected.decision);

  // A build predating the decision markers records none at all. Scoring that as
  // a model failure would blame the model for a harness gap, so it fails loudly
  // as an environment problem instead.
  if (!decision) {
    if (expected.decision === "operation") {
      return {
        passed: false,
        failure:
          "No operation decision recorded. Either the daemon predates the " +
          "[decision] markers, or the turn never reached inference.",
      };
    }
    // A schema decision is legitimately absent when the turn named no type —
    // which is itself the answer for a `notOneOf` expectation.
    if ("notOneOf" in expected) return { passed: true };
    return {
      passed: false,
      failure: `Expected the turn to act on '${expected.type}' but it named no type at all.`,
    };
  }

  if (expected.decision === "operation") {
    const selected = decision.selected;
    if (expected.oneOf.length === 0) {
      return selected === null
        ? { passed: true }
        : {
            passed: false,
            failure: `Expected no tool call, but called '${selected}'. Offered: ${decision.candidates.join(", ")}`,
          };
    }
    if (selected === null) {
      return {
        passed: false,
        failure: `Expected one of [${expected.oneOf.join(", ")}] but no tool was called. Offered: ${decision.candidates.join(", ")}`,
      };
    }
    if (!expected.oneOf.includes(selected)) {
      // Whether the right tool was even on offer changes what the failure
      // means: an unavailable tool is a routing/scoping problem, not a
      // selection one, and the two need different fixes.
      const wasOffered = expected.oneOf.some((t) =>
        decision.candidates.includes(t),
      );
      return {
        passed: false,
        failure: wasOffered
          ? `Chose '${selected}' over [${expected.oneOf.join(", ")}], which were offered: ${decision.candidates.join(", ")}`
          : `Chose '${selected}'; none of [${expected.oneOf.join(", ")}] were offered at all: ${decision.candidates.join(", ")}`,
      };
    }
    return { passed: true };
  }

  // Schema expectations.
  const selected = decision.selected;
  if (decision.offMenu) {
    return {
      passed: false,
      failure: `Named type '${selected}', which retrieval never offered (candidates: ${decision.candidates.join(", ")}).`,
    };
  }
  if ("notOneOf" in expected) {
    if (selected !== null && expected.notOneOf.includes(selected)) {
      return {
        passed: false,
        failure: `Acted on '${selected}', which the message's wording rules out. Candidates: ${decision.candidates.join(", ")}`,
      };
    }
    return { passed: true };
  }
  if (selected !== expected.type) {
    return {
      passed: false,
      failure: `Acted on '${selected ?? "(none)"}' rather than '${expected.type}'. Candidates: ${decision.candidates.join(", ")}`,
    };
  }
  return { passed: true };
}

const fixture: EvalFixture = {
  name: "decisions",
  description: "Decision Eval Results (schema and operation selection)",
  // One group: the setup turns build the multi-schema workspace every scored
  // scenario depends on, and scenarios within a group share a chat node.
  groups: [[...SETUP, ...FIXTURES]],
  score(scenario, turns) {
    return assertFixture(scenario as DecisionScenario, turns);
  },
  extra(scenario, turns) {
    const s = scenario as DecisionScenario;
    return {
      expected: s.expected,
      adr056: s.adr056 ?? false,
      ambiguous: s.ambiguous ?? false,
      operationDecision: firstDecision(turns, "operation") ?? null,
      schemaDecision: firstDecision(turns, "schema") ?? null,
      // Recorded for every scenario, not just failing ones: a type named that
      // was never offered is the single most diagnostic signal here, and it
      // needs to be visible in the results file without a re-run.
      offMenu: turns.some((t) => t.decisions?.some((d) => d.offMenu)),
      toolsCalled: turns.flatMap((t) => t.toolsCalled),
      latencyMs: turns.reduce((sum, t) => sum + t.latencyMs, 0),
    };
  },
  summary(results) {
    const count = (pred: (e: Record<string, unknown>) => boolean) => {
      const rows = results.filter((r) => r.extra && pred(r.extra));
      return `${rows.filter((r) => r.passed).length}/${rows.length}`;
    };
    const offMenu = results.filter((r) => r.extra?.offMenu === true).length;
    return [
      `Operation selection: ${count((e) => (e.expected as { decision?: string })?.decision === "operation")}`,
      `Schema selection:    ${count((e) => (e.expected as { decision?: string })?.decision === "schema")}`,
      `Ambiguous (several types plausible): ${count((e) => e.ambiguous === true)}`,
      `ADR-056 known failures: ${count((e) => e.adr056 === true)}`,
      `Off-menu type named: ${offMenu}`,
    ];
  },
};

export default fixture;
