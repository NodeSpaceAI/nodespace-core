/**
 * Decision eval — scores the three selections an agent turn makes directly,
 * rather than inferring them from whether the scenario as a whole passed.
 *
 *   skill      — which skill retrieval matched (upstream; scopes the other two)
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

// Schema expectations deliberately never name an exact type id. `create_schema`
// derives the id from the MODEL's phrasing of the request, not from the
// fixture's: "a new type for the companies we sell to" produced
// `company_sold_to`, and "the places we hold events" produced a type the model
// called "Event Venue". An assertion naming `company` or `venue` fails on a
// naming mismatch rather than a decision error, which is a fixture bug wearing
// a model bug's clothes — and it cost two full 3-rep runs to notice. Assert on
// the PROPERTY being scored (did it stay on the menu? did it pick the right one
// of two?) rather than on an id the fixture cannot predict.
type Expected =
  /** The turn's first operation must be one of these. */
  | { decision: "operation"; oneOf: string[] }
  /**
   * The turn must act on a type retrieval actually offered.
   *
   * The scoreable property for an unambiguous request: not *which* id, but
   * whether the model stayed within the candidate set at all. An off-menu
   * selection is the one schema failure that cannot be explained as a close
   * call between plausible options.
   */
  | { decision: "schema"; onMenu: true }
  /**
   * The selected type must match this pattern.
   *
   * The disambiguation shape: two types are on offer and only one fits the
   * message's wording. Matched loosely because the id is model-derived — the
   * question is which of the two it picked, not what it named them.
   */
  | { decision: "schema"; matches: RegExp }
  /**
   * Retrieval must lead with a skill matching this pattern.
   *
   * Scored separately from the operation because this is the layer the
   * observed failures occur at, and the two are otherwise indistinguishable: a
   * type-definition request routed to Node Creation never sees `create_schema`
   * at all (`stage2_tools` scopes it out), so it reads as a wrong-tool failure
   * when retrieval was what went wrong.
   */
  | { decision: "skill"; matches: RegExp };

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
  /**
   * Turns on the instance-vs-type boundary: is this one record, or a new kind
   * of record? A schema IS a node, so the distinction is abstraction level
   * rather than kind of thing — and it carries the larger blast radius, since a
   * spurious type definition is worse than a spurious record.
   */
  instanceVsType?: boolean;
  /** Covers a thin-evidence area an ADR calls out explicitly. */
  loadBearing?: boolean;
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
    id: "setup-company",
    scenario: "Setup: company type",
    prompt:
      "Set up a new type for the companies we sell to, with a name and the date we signed them.",
    setup: true,
    expected: { decision: "operation", oneOf: ["create_schema"] },
  },
  {
    id: "setup-venue",
    scenario: "Setup: venue type",
    // Deliberately parallel in shape to `setup-company`, which routes to
    // Schema Creation reliably. An earlier wording ("Set up a way to track
    // pieces of delivery work...") routed to Node Creation instead on all three
    // reps — the model read it as creating one instance — so `create_schema`
    // was never offered and every downstream scenario was blocked rather than
    // measured.
    prompt:
      "Set up a new type for the places we hold events, with a name, a booking date and a capacity.",
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
    prompt: "Which companies did we sign this year?",
    expected: { decision: "operation", oneOf: ["search_nodes"] },
    adr056: true,
  },
  {
    id: "op-read-then-write",
    scenario: "Operation: a state change on a node named indirectly",
    // ADR-056 Scenario 6: the read fired and the write never did. Scored on the
    // FIRST operation being a resolution step rather than a blind write — the
    // turn-completion half is what the matrix eval already covers.
    prompt: "Northwind Trading has moved their booking to April.",
    expected: {
      decision: "operation",
      oneOf: ["search_nodes", "resolve_query", "update_node"],
    },
    adr056: true,
  },

  // ── Skill routing (instance vs type) ───────────────────────────────────
  //
  // The layer the observed failures actually occur at. Stage 1 matches an
  // embedding of the message against skill `description` properties (ADR-064
  // rule 3: those are retrieval index keys, not documentation) — tool
  // descriptions are not consulted, and by the time they are, the whitelist is
  // already scoped.
  //
  // Both skill descriptions do encode the instance-vs-type distinction
  // ("example of an existing type" vs "a kind of thing the user hasn't stored
  // before"), and retrieval still led with Node Creation for the second
  // scenario below during development. So these score embedding similarity,
  // not instruction quality — the same class as ADR-038 Finding 4 and #1980.
  {
    id: "skill-type-request-direct",
    scenario: "Skill: an explicit type request routes to Schema Creation",
    // Worded to avoid reusing `create_schema`'s own description verbatim — the
    // contamination guard rejected "Define a new entity type called Sponsor…"
    // for sharing five consecutive words with it, which would have let this
    // scenario pass by keyword recall rather than by generalizing.
    prompt: "Sponsors aren't something we can record yet — set that up, with a tier and a renewal date.",
    expected: { decision: "skill", matches: /schema/i },
    instanceVsType: true,
  },
  {
    id: "skill-type-request-indirect",
    scenario: "Skill: an indirectly-phrased type request",
    // The exact shape that misrouted during development: phrased as "a way to
    // track <plural things>", which embeds near Node Creation's vocabulary
    // (create, record, item, entry) despite being a type-definition request.
    prompt: "I need a way to keep track of sponsorship deals, each with a tier and a renewal date.",
    expected: { decision: "skill", matches: /schema/i },
    instanceVsType: true,
    loadBearing: true,
  },
  {
    id: "skill-instance-request",
    scenario: "Skill: a single-instance request must NOT route to Schema Creation",
    // The mirror, and the direction Laya failed on: an explicitly single named
    // entity of a type that already exists.
    prompt: "Add Contoso Ltd to the companies we sell to.",
    expected: { decision: "skill", matches: /node creation|graph editing|organization/i },
    instanceVsType: true,
  },

  // ── Schema selection ───────────────────────────────────────────────────
  //
  // Every type asserted on here is USER-DEFINED, created by the setup turns
  // above. Core seeded types (`project`, `task`, `person`, ...) cannot be used:
  // `parse_and_filter_non_core_schemas` strips them from schema candidates, so
  // a turn acting on one gets an EMPTY candidate set and the model emits the
  // type name from general context with nothing to select among. Measured
  // directly — "How many projects do we have?" produced
  // `selected=project [off-menu] candidates=`. That is unconstrained
  // generation, not a selection, and scoring it as one would measure the
  // `is_core` filter rather than the model.
  {
    id: "schema-on-menu-company",
    scenario: "Schema: acts on a retrieved candidate, not an invented id",
    prompt: "Add Northwind Trading to the companies we sell to.",
    expected: { decision: "schema", onMenu: true },
  },
  {
    id: "schema-field-disambiguates",
    scenario: "Schema: the named field settles which type is meant",
    // Both setup types carry a date, but only the venue has a capacity — so a
    // message about seating can only mean the venue. The disambiguating signal
    // is structural (which type even has that field), not semantic similarity,
    // which is the case embedding distance alone cannot resolve.
    prompt: "Northwind can seat 200 people now.",
    expected: { decision: "schema", matches: /venue|event/i },
    ambiguous: true,
  },
  {
    id: "schema-shared-name-signed",
    scenario: "Schema: shared name, company-only attribute",
    // The mirror: the same bare name, but the attribute mentioned exists only
    // on the company type.
    prompt: "When did we sign Northwind?",
    expected: { decision: "schema", matches: /compan/i },
    ambiguous: true,
  },
];

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

/** The first decision of the given kind across the turn's rounds. */
function firstDecision(turns: TurnRecord[], kind: "skill" | "schema" | "operation") {
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
    if (expected.decision === "skill") {
      return {
        passed: false,
        failure:
          "No skill decision recorded — either the daemon predates the " +
          "[decision skill] marker, or the turn never reached retrieval.",
      };
    }
    if (expected.decision === "operation") {
      return {
        passed: false,
        failure:
          "No operation decision recorded. Either the daemon predates the " +
          "[decision] markers, or the turn never reached inference.",
      };
    }
    return {
      passed: false,
      failure:
        "No schema decision recorded — the turn named no type at all. " +
        "(Note: core seeded types are filtered out of schema candidates, so a " +
        "turn acting on one records an empty candidate set.)",
    };
  }

  if (expected.decision === "skill") {
    if (decision.selected === null) {
      return {
        passed: false,
        failure:
          `Nothing cleared its score bar, so the turn fell open to the full ` +
          `tool surface. Retrieved: ${decision.candidates.join(", ") || "(nothing)"}`,
      };
    }
    if (!expected.matches.test(decision.selected)) {
      // Whether the right skill was even retrieved separates "retrieval missed
      // it" from "retrieval found it and ranked it below" — different failures
      // with different fixes.
      const retrieved = decision.candidates.some((c) => expected.matches.test(c));
      return {
        passed: false,
        failure: retrieved
          ? `Led with '${decision.selected}' over a retrieved candidate matching ${expected.matches}. Retrieved: ${decision.candidates.join(", ")}`
          : `Led with '${decision.selected}'; nothing matching ${expected.matches} was retrieved at all. Retrieved: ${decision.candidates.join(", ")}`,
      };
    }
    return { passed: true };
  }

  if (expected.decision === "operation") {
    const selected = decision.selected;
    // An empty `oneOf` asserts the turn should call NOTHING. No scenario uses
    // it today — the one that did ("What kinds of things can you help me keep
    // track of?") never reached inference and was removed rather than left
    // failing for a harness reason. Kept because "picked no tool" is a real
    // decision the record already distinguishes from "picked wrongly", and an
    // agent that reaches for a tool on every message is failing a decision even
    // when its reply reads fine.
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
      failure: `Named type '${selected}', which retrieval never offered (candidates: ${decision.candidates.join(", ") || "(none)"}).`,
    };
  }
  if ("onMenu" in expected) {
    if (selected === null) {
      return {
        passed: false,
        failure: `Named no type. Candidates offered: ${decision.candidates.join(", ") || "(none)"}`,
      };
    }
    // offMenu already returned above, so reaching here means it is on the menu.
    return { passed: true };
  }
  if (selected === null) {
    return {
      passed: false,
      failure: `Named no type; expected one matching ${expected.matches}. Candidates: ${decision.candidates.join(", ") || "(none)"}`,
    };
  }
  if (!expected.matches.test(selected)) {
    return {
      passed: false,
      failure: `Acted on '${selected}', which does not match ${expected.matches}. Candidates: ${decision.candidates.join(", ")}`,
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
      instanceVsType: s.instanceVsType ?? false,
      loadBearing: s.loadBearing ?? false,
      skillDecision: firstDecision(turns, "skill") ?? null,
      operationDecision: firstDecision(turns, "operation") ?? null,
      schemaDecision: firstDecision(turns, "schema") ?? null,
      // Recorded for every scenario, not just failing ones: a type named that
      // was never offered is the single most diagnostic signal here, and it
      // needs to be visible in the results file without a re-run.
      offMenu: turns.some((t) => t.decisions?.some((d) => d.offMenu)),
      toolsCalled: turns.flatMap((t) => t.toolsCalled),
      latencyMs: turns.reduce((sum, t) => sum + t.latencyMs, 0),
      // The decision's own cost, separate from the turn's. One generative pass
      // to emit a structural choice among three routing tools — the term any
      // decision-model comparison turns on alongside accuracy.
      routingMs: turns.reduce((sum, t) => sum + (t.routingMs ?? 0), 0),
    };
  },
  summary(results) {
    const count = (pred: (e: Record<string, unknown>) => boolean) => {
      const rows = results.filter((r) => r.extra && pred(r.extra));
      return `${rows.filter((r) => r.passed).length}/${rows.length}`;
    };
    const offMenu = results.filter((r) => r.extra?.offMenu === true).length;
    const routing = results
      .map((r) => Number(r.extra?.routingMs ?? 0))
      .filter((n) => n > 0);
    const meanRouting = routing.length
      ? Math.round(routing.reduce((a, b) => a + b, 0) / routing.length)
      : 0;
    return [
      `Skill routing:       ${count((e) => (e.expected as { decision?: string })?.decision === "skill")}`,
      `Operation selection: ${count((e) => (e.expected as { decision?: string })?.decision === "operation")}`,
      `Schema selection:    ${count((e) => (e.expected as { decision?: string })?.decision === "schema")}`,
      `Instance-vs-type boundary: ${count((e) => e.instanceVsType === true)}`,
      `Ambiguous (several types plausible): ${count((e) => e.ambiguous === true)}`,
      `ADR-056 known failures: ${count((e) => e.adr056 === true)}`,
      `Off-menu type named: ${offMenu}`,
      `Stage-1 decision cost: ${meanRouting}ms mean (one generative pass for a 3-way structural choice)`,
    ];
  },
};

export default fixture;
