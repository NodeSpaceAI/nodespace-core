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

import type { EvalEnv } from "../env.ts";
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
  | { decision: "skill"; matches: RegExp }
  /**
   * The turn must not create a second copy of this existing record, and must
   * either ask the user about it (a clarification naming it) or act on it.
   *
   * Not a decision assertion, and deliberately the only one that is not. It
   * exists for the case a write-path guard closes rather than the model: the
   * model's choice stays wrong, the system corrects it, and what is worth
   * pinning is that the user never gets the duplicate. The operation decision
   * is still recorded in the results file for every scenario.
   */
  | { decision: "outcome"; noDuplicateOf: string };

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
  /**
   * Turns on resolving a NAME to a node, not on choosing among types.
   *
   * These were previously scored as schema-selection failures, which measured
   * the wrong layer: the model never reached a choice among the candidates on
   * offer — it could not get from "Northwind" to a node id, so it deferred and
   * asked the user for one. Scoring that as a bad schema pick attributes a
   * lookup failure to a judgment the model never made.
   */
  entityResolution?: boolean;
  /**
   * Turns on declarative phrasing: a sentence that reports a state rather than
   * commanding a change.
   *
   * Scored as its own dimension because the entity tier is not expected to
   * help here. Resolution answers *which node*; it says nothing about whether
   * a change is being requested, and a declarative state-change is
   * surface-identical to a fact-report. Three independently-measured models
   * have shown the same asymmetry between imperative and declarative intent,
   * so a persistent failure on this dimension is a property of the problem
   * rather than of this agent.
   *
   * Always paired: a state-change and a fact-report using the same entity, so
   * the summary distinguishes "reads intent" from "writes on any declarative".
   */
  declarative?: boolean;
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

/// The company instance the entity scenarios act on.
///
/// Seeded through `seedRun` rather than as a scored setup TURN, for two
/// reasons. First, a turn's writes are replayed into every later turn as terse
/// facts carrying the id inline, so seeding by turn would put the answer in the
/// prompt as literal history — seeding out of band keeps instance data out of
/// that channel entirely (see `EvalFixture.seedGroup`'s contract, which
/// `seedRun` shares). Second, a turn lengthens the conversation, and length
/// alone was enough to make the model stop emitting tool calls.
const SEEDED_COMPANY_TITLE = "Northwind Trading";
const SEEDED_COMPANY_SIGNED = "2025-03-14";

/// The company type this fixture creates when a rep starts cold.
///
/// Named literally, unlike every ASSERTION in this file: the seed establishes
/// state rather than scoring a decision, so there is nothing here for a
/// model-derived id to invalidate. The schema assertions still match on a
/// pattern, because those score what the model chose.
const SEEDED_COMPANY_TYPE = "company_sold_to";

/// Words that identify the company type among the schemas the setup turn
/// created, since its id cannot be predicted.
///
/// `create_schema` derives the id from the MODEL's phrasing, not the
/// fixture's: the same `setup-company` prompt produced `company_sold_to` in one
/// run and `client_company` in another. An earlier version of this seed named
/// `company_sold_to` literally, never found it, and silently seeded nothing —
/// so every entity scenario scored against a workspace with no Northwind in
/// it. The file's own expectation model warns about exactly this ("Schema
/// expectations deliberately never name an exact type id") and records that it
/// cost two 3-rep runs to learn the first time.
///
/// Matching on a word stem rather than an id is the same tactic the schema
/// assertions use, for the same reason.
const COMPANY_TYPE_HINTS = ["compan", "client", "customer", "account"];

/// The venue type, excluded from the company match: `setup-venue` runs in the
/// same group, and a naive "first schema mentioning a company-ish word" could
/// pick it up if the model phrased it as "client venue" or similar.
const VENUE_TYPE_HINTS = ["venue", "event", "place"];

function runNs(env: EvalEnv, args: string[]): unknown {
  const r = Bun.spawnSync([env.nsBin, "--socket", env.socket, "--json", ...args], {
    stdout: "pipe",
    stderr: "pipe",
  });
  if (r.exitCode !== 0) {
    throw new Error(
      `nodespace ${args.join(" ")} failed (exit ${r.exitCode}): ` +
        r.stderr.toString().trim(),
    );
  }
  const out = r.stdout.toString().trim();
  return out ? JSON.parse(out) : null;
}

/**
 * Create the Northwind instance the entity scenarios resolve against.
 *
 * Idempotent, and that is load-bearing rather than defensive. `seedRun` fires
 * once per rep, and while `--between-runs` normally wipes the database first,
 * the hook must not depend on that: a run without a between-runs command, or
 * one whose reset failed, would otherwise leave rep 2 with two Northwinds and
 * rep 3 with three. The name would stop resolving to a single node and the
 * resulting failures would read as model non-determinism — corrupting the very
 * measurement `--runs` exists to produce.
 *
 * Runs as `seedRun`, once per rep, BEFORE any group. That timing is the fix
 * for a defect that silently invalidated three separate measured runs:
 * `seedGroup` fires inside the group loop, and `--between-runs` wipes the
 * database between reps, so the first group of every rep saw a workspace whose
 * types its own setup turns had not created yet. A seed that waited for those
 * types found nothing, no-oped, and every scenario scored against a workspace
 * with no instance in it — looking exactly like a model result.
 *
 * Because it runs before the setup turns, it CREATES the company type rather
 * than discovering one. The setup turns still run and are still scored; a
 * `create_schema` against an existing type is idempotent at the store, so
 * `setup-company` asserting on that operation is unaffected either way.
 *
 * The type id is fixed here rather than model-derived, which is the one place
 * this fixture may name one: everything downstream (the schema assertions)
 * still matches on a pattern, because those score what the MODEL selected.
 * This is establishing state, not asserting on it.
 */
function seedNorthwind(env: EvalEnv): void {
  const schemas = runNs(env, ["schema", "list"]) as {
    nodes?: Array<{
      id?: string;
      properties?: { isCore?: boolean; fields?: Array<{ name?: string; type?: string }> };
    }>;
  } | null;

  // Non-core only: a core type matching a hint word (`person`, `project`)
  // would shadow the company type.
  const candidates = (schemas?.nodes ?? []).filter(
    (s) => s?.id && s.properties?.isCore !== true,
  );
  let company = candidates.find((s) => {
    const id = (s.id ?? "").toLowerCase();
    if (VENUE_TYPE_HINTS.some((h) => id.includes(h))) return false;
    return COMPANY_TYPE_HINTS.some((h) => id.includes(h));
  });

  // Cold rep: no company type exists yet, so create the one this fixture's
  // instances hang off. Reusing a model-created type when present keeps a
  // warm rep from ending up with two company-ish types.
  if (!company?.id) {
    runNs(env, [
      "schema",
      "create",
      "--params",
      JSON.stringify({
        name: SEEDED_COMPANY_TYPE,
        description: "A company we sell to, and the date we signed them",
        fields: [
          { name: "name", type: "text" },
          { name: "signed_date", type: "date" },
        ],
      }),
    ]);
    company = {
      id: SEEDED_COMPANY_TYPE,
      properties: { fields: [{ name: "signed_date", type: "date" }] },
    };
  }

  const companyType = company?.id;
  if (!companyType) {
    throw new Error("company type missing after seed — schema create did not take");
  }

  const existing = runNs(env, [
    "node",
    "query",
    "--type",
    companyType,
    "--limit",
    "50",
  ]) as { nodes?: Array<{ content?: string; id?: string }> } | null;
  const present = (existing?.nodes ?? []).some(
    (n) => (n?.content ?? "").toLowerCase() === SEEDED_COMPANY_TITLE.toLowerCase(),
  );
  if (present) return;

  const created = runNs(env, [
    "node",
    "create",
    "--type",
    companyType,
    "--content",
    SEEDED_COMPANY_TITLE,
  ]) as { id?: string } | null;
  const id = created?.id;
  if (!id) throw new Error(`seeding '${SEEDED_COMPANY_TITLE}' returned no id`);

  // The date field's name is model-derived too (`signed_date`, `date_signed`,
  // `signed_on` have all appeared), so find it by type rather than by name.
  // Skipped rather than failed when absent: the scenario that reads it
  // (`schema-shared-name-signed`) is scored on which TYPE the model selects,
  // not on the value it returns, so a missing date weakens one assertion
  // rather than invalidating the seed.
  const dateField = company.properties?.fields?.find(
    (f) => f?.type === "date" && f?.name,
  )?.name;
  if (dateField) {
    runNs(env, [
      "node",
      "update",
      id.replace(/^nodespace:\/\//, ""),
      "--property",
      `${dateField}=${SEEDED_COMPANY_SIGNED}`,
    ]);
  }
}

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
    scenario: "Schema: a name that already exists is an update, not a create",
    // Ambiguous BY DESIGN now that `setup-northwind-instance` seeds the
    // company. "Add X to the companies we sell to" could mean create this or
    // you already have this — and with the instance present, the right answer
    // is the latter. That ambiguity is the point: it is the disambiguation
    // case the entity tier exists to settle, and it is only a real test while
    // both readings are available.
    //
    // Scored on the OPERATION rather than the schema: what changed with the
    // tier is not which type gets picked but whether the model acts on the
    // existing node instead of creating a duplicate.
    // Measured: with Northwind seeded and rendered in MENTIONED ENTITIES, the
    // model called `create_node` on 3 of 3 reps — a silent duplicate. The
    // resolution worked; what was missing was any instruction for what to DO
    // about a collision, and any tool to do it with (`route_clarify` was not
    // on this skill's whitelist).
    //
    // `route_clarify` is the answer this scenario wants: "Add X" when X exists
    // is genuinely ambiguous — a second distinct record is a real thing to
    // want — so the turn should hand the choice back rather than guess.
    //
    // It does not pass today, and the cause is NOT missing instruction.
    // Verified on the locked model with the entity rendered and
    // `route_clarify` on the menu: guidance prose, the `create_node` tool
    // description, and both together all produced `create_node` and a silent
    // duplicate. That third result is the notable one — ADR-064 measured the
    // tool-schema channel at 100% compliance where prose reached 87.5%, and it
    // moved this not at all. "Do not do the obvious thing when a condition
    // holds" appears to be a harder ask than the output-shape constraints that
    // measurement covered.
    //
    // Closed by a deterministic guard on the write path instead: the system
    // holds the resolved entity, so a `create_node` whose type and title match
    // it is refused before it executes, the model is told to ask, and if it
    // does not, the system asks for it. So this scores the OUTCOME — no
    // duplicate, and the user asked — rather than the operation decision,
    // which the guard cannot change: the model still picks `create_node`, and
    // `operationDecision` in the results file keeps recording that miss.
    prompt: "Add Northwind Trading to the companies we sell to.",
    expected: { decision: "outcome", noDuplicateOf: SEEDED_COMPANY_TITLE },
    entityResolution: true,
  },
  {
    id: "schema-field-disambiguates",
    scenario: "Entity: the named field settles which type is meant",
    // Both setup types carry a date, but only the venue has a capacity — so a
    // message about seating can only mean the venue. The disambiguating signal
    // is structural (which type even has that field), not semantic similarity,
    // which is the case embedding distance alone cannot resolve.
    //
    // Scored on the OPERATION, not the schema, and that is a fixture fix
    // rather than a weakening. Measured: the model called `update_node`, which
    // is right — setting a capacity on an existing venue is an update — but
    // `update_node` takes a node id, so no schema decision is ever recorded
    // and a `decision: "schema"` assertion could not pass however well the
    // model reasoned. It was asserting on a decision the correct operation
    // does not make. What this scenario can actually observe is whether the
    // turn updates the existing record rather than creating a second one.
    prompt: "Northwind can seat 200 people now.",
    expected: {
      decision: "operation",
      oneOf: ["update_node", "search_nodes", "resolve_query", "get_node"],
    },
    ambiguous: true,
    entityResolution: true,
  },
  {
    id: "schema-shared-name-signed",
    scenario: "Entity: shared name, company-only attribute",
    // The mirror: the same bare name, but the attribute mentioned exists only
    // on the company type. With the instance seeded this is a READ of an
    // existing node, which is what makes it an entity-resolution case — the
    // recorded failure was "I do not have a specific node ID for Northwind
    // Trading, so I cannot tell you the signing date."
    prompt: "When did we sign Northwind?",
    expected: { decision: "schema", matches: /compan/i },
    ambiguous: true,
    entityResolution: true,
  },
  {
    id: "entity-no-match-is-a-create",
    scenario: "Entity: a name that resolves to nothing means CREATE",
    // The other half of the tier, and the reason its output is three-state.
    // "Resolved to nothing" is a positive fact — this thing does not exist, so
    // make it — and it must not read the same as "the resolver did not run".
    // Tailspin is deliberately absent from the seeded workspace.
    prompt: "Add Tailspin Toys to the companies we sell to.",
    expected: { decision: "operation", oneOf: ["create_node"] },
    entityResolution: true,
  },

  // ── Declarative state-changes, with the entity resolved ────────────────
  //
  // Isolates the one question the entity tier does NOT obviously answer.
  //
  // A declarative state-change ("Northwind has moved their booking") is
  // surface-identical to a fact-report ("Northwind moved offices last year").
  // The tier tells the model WHICH NODE is meant; it does not tell it that a
  // CHANGE IS BEING REQUESTED. So resolution may fix the imperative case while
  // leaving the declarative one exactly as it was.
  //
  // This matters beyond the fixture. A prior Laya spike measured the same
  // asymmetry in a non-autoregressive decision model: imperative-vs-question
  // AUC 0.974, declarative-vs-question 0.702, with declarative state-changes
  // scoring 2/14. An in-repo 2025 experiment found it in a fine-tuned Gemma 3
  // 12B too. Three models, one shape — so it reads as a property of the
  // problem rather than of any model.
  //
  // These scenarios name `Northwind Trading`, which the seeded workspace
  // contains, so resolution succeeds and the only thing left to judge is
  // whether a declarative sentence is a request to act. If the agent acts on
  // these, entity grounding solved more than its own failure and the residual
  // judgment is smaller than the spike implied; if it still declines, that
  // residual is isolated and measurable.
  {
    id: "declarative-state-change-resolved",
    scenario: "Declarative: a state change phrased as a report, entity resolves",
    // Deliberately parallel to `op-read-then-write` — same shape, but that one
    // ran before the tier existed and could fail for want of an entity. Here
    // the entity is seeded, so a failure isolates the phrasing.
    prompt: "Northwind Trading signed on a different date — it was the 20th of March.",
    expected: {
      decision: "operation",
      oneOf: ["update_node", "search_nodes", "resolve_query"],
    },
    declarative: true,
    entityResolution: true,
  },
  {
    id: "declarative-fact-report-resolved",
    scenario: "Declarative: a fact report that is NOT a change request",
    // The control, and the reason the pair is scored together. If the agent
    // writes here it is over-acting on declarative mood rather than reading
    // intent, which is the opposite failure and equally worth catching. An
    // agent that passes the scenario above by treating every declarative as a
    // write would fail this one.
    prompt: "Northwind Trading has been a customer of ours for a long time.",
    expected: { decision: "operation", oneOf: [] },
    declarative: true,
    entityResolution: true,
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

/**
 * How every clarification the agent ends a turn with opens — its own
 * `route_clarify` and the duplicate guard's alike. Mirrors
 * `CLARIFICATION_OPENER` in `packages/agent/src/local_agent/agent_loop.rs`.
 */
const CLARIFICATION_OPENER = "I can take that a couple of ways";

/**
 * Score a turn that must not duplicate `title`: no `create_node` may have
 * succeeded, and the turn must have asked the user about the record or acted
 * on it with an update.
 */
function assertNoDuplicate(title: string, turns: TurnRecord[]): Verdict {
  // `toolCalls` is what tells a refused create (isError) from one that landed.
  // A turn that called tools but recorded no outcomes cannot be scored here —
  // treating it as "no create succeeded" would pass a real duplicate.
  if (turns.some((t) => t.toolsCalled.length > 0 && t.toolCalls === undefined)) {
    return {
      passed: false,
      failure:
        "Tools were called but no per-call outcomes were recorded, so a refused " +
        "create cannot be told from one that succeeded.",
    };
  }
  const calls = turns.flatMap((t) => t.toolCalls ?? []);
  if (calls.some((c) => c.name === "create_node" && !c.isError)) {
    return {
      passed: false,
      failure: `A create_node succeeded, so "${title}" now exists twice. Tools: ${turns.flatMap((t) => t.toolsCalled).join(", ")}`,
    };
  }
  const reply = turns.at(-1)?.reply ?? "";
  if (reply.startsWith(CLARIFICATION_OPENER) && reply.includes(title)) {
    return { passed: true };
  }
  if (calls.some((c) => c.name === "update_node" && !c.isError)) {
    return { passed: true };
  }
  return {
    passed: false,
    failure: `No duplicate was created, but the user was neither asked about "${title}" nor was it updated. Reply: ${reply.slice(0, 300)}`,
  };
}

function assertFixture(
  fixture: DecisionScenario,
  turns: TurnRecord[],
): Verdict {
  const { expected } = fixture;
  if (expected.decision === "outcome") {
    return assertNoDuplicate(expected.noDuplicateOf, turns);
  }
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
  // One group PER SCORED SCENARIO, each preceded by the setup turns.
  //
  // Previously a single group held all of them, so every scenario shared one
  // chat — and by the later ones the model stopped emitting tool calls at all.
  // A measured run showed every scenario after the fourth recording
  // `toolsCalled: []`, including `skill-instance-request`, which has nothing to
  // do with the entity work and passes in isolation. That is conversation
  // length being scored as decision quality: the scenarios at the end of the
  // list were never measured on their own merits.
  //
  // The setup turns are cheap to repeat and idempotent in effect — `schema
  // list` is consulted before creating, and a second `create_schema` for an
  // existing type is a no-op — so paying them per group buys scenario
  // isolation without changing what any scenario is asked.
  //
  // This makes the fixture's numbers NOT comparable to any run recorded before
  // the split: each scenario now starts from a short conversation rather than
  // inheriting however many turns preceded it.
  groups: FIXTURES.map((scenario) => [...SETUP, scenario]),
  // Per REP, not per group. `--between-runs` wipes the database between reps,
  // and `seedGroup` runs inside the group loop where a cold rep's first group
  // has no types yet — so a group-scoped seed silently no-ops and every
  // scenario scores against a workspace with no instance in it. That defect
  // invalidated three measured runs before it was found; `seedRun` exists to
  // make it unrepresentable.
  seedRun(env: EvalEnv) {
    seedNorthwind(env);
  },
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
      entityResolution: s.entityResolution ?? false,
      declarative: s.declarative ?? false,
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
      // Reported separately from everything else: an aggregate hides the one
      // asymmetry three independently-measured models have all shown, and this
      // is the dimension the entity tier is NOT expected to have fixed.
      `Declarative intent (entity resolved): ${count((e) => e.declarative === true)}`,
      `Ambiguous (several types plausible): ${count((e) => e.ambiguous === true)}`,
      `Entity resolution: ${count((e) => e.entityResolution === true)}`,
      `ADR-056 known failures: ${count((e) => e.adr056 === true)}`,
      `Off-menu type named: ${offMenu}`,
      `Stage-1 decision cost: ${meanRouting}ms mean (one generative pass for a 3-way structural choice)`,
    ];
  },
};

export default fixture;
