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
import { awaitSkillIndex } from "../preflight.ts";
import type {
  EvalFixture,
  Scenario,
  ToolCallRecord,
  TurnRecord,
  Verdict,
} from "../types.ts";

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
  | { decision: "outcome"; noDuplicateOf: string }
  /**
   * The turn must search the user's stored knowledge before its reply reaches
   * them.
   *
   * An outcome rather than an operation decision for the same reason
   * `noDuplicateOf` is: the loop closes this case when the model does not, by
   * running the search itself. A turn whose first round replied in prose
   * still searched before the user saw anything, which is what is worth
   * pinning. Whether the model searched unprompted stays visible in
   * `operationDecision`.
   */
  | { decision: "outcome"; searchesBeforeReplying: true }
  /**
   * The reply must match this pattern, whatever the turn did to get there.
   *
   * For a question whose answer is in the prompt rather than the graph, where
   * the tools called say nothing about whether the user got it: asked to find
   * one of the agent's own skills, a turn may or may not search first, and
   * either way the reply has to name the skill.
   */
  | { decision: "outcome"; replyMatches: RegExp }
  /**
   * The reply must name every one of these types, off a `search_nodes` call
   * that succeeded.
   *
   * An outcome assertion for the same reason as the one above: what is worth
   * pinning is what the user is told. Asked which types exist, the right tool
   * with the wrong scope still answers with a partial list, and so does no
   * tool at all — the turn reads the custom types out of its context block and
   * stops. Requiring the successful search keeps a reply that happens to name
   * the right types from passing without having looked.
   */
  | { decision: "outcome"; listsTypes: RegExp[] }
  /**
   * For a request whose record is of a type outside the linked set: the turn
   * must be held to its skills' linked types, no call naming a type outside
   * them may run, and the record must not then be written as an offered type.
   *
   * An outcome assertion because the property is the system's, not the
   * model's: the model may still name a type that was never offered, and what
   * is worth pinning is that dispatch stopped it and that the refusal did not
   * turn a wrong type into a wrong record. Fails when the turn was not held at
   * all, which is a retrieval result (an unlinked skill cleared its bar
   * alongside the linked ones) rather than a model one, and says so.
   */
  | { decision: "outcome"; heldToOfferedTypes: true };

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
  /**
   * Asks about what the user has stored, or about the agent itself, rather
   * than asking for a change.
   *
   * Scored as its own dimension because these fail at a layer the write
   * scenarios never reach. A question carries no verb for retrieval to match,
   * so it routes on whichever skill shares a noun with its topic, and a turn
   * that reaches no lookup skill answers from the conversation or asks the
   * user for context instead of searching.
   */
  knowledgeQuestion?: boolean;
  /**
   * Runs against skills linked to a schema through `applies_to`, seeded for
   * this scenario's group (see `seedLinkedSkills`).
   *
   * No built-in skill links to a schema, so without these a turn never has an
   * offered set and nothing here measures one.
   */
  linkedSkills?: boolean;
}

// ---------------------------------------------------------------------------
// Workspace
//
// Every scenario runs against a workspace carrying two overlapping custom
// types, because a decision is only scoreable when there was something to
// decide between: a single-schema workspace makes every schema selection
// trivially correct and measures nothing.
//
// The types are seeded once per rep (`seedWorkspace`), not created by turns in
// each chat. They were once two setup turns ahead of every scenario, and that
// made a scenario's score depend on the chats before it:
//
// - Chats in a rep share a database, so from the second chat on the turns met
//   types that already existed. Each scenario was then asked after two turns
//   that had failed ("I couldn't complete the schema creation"), which is the
//   history in which `schema-on-menu-company` stopped passing.
// - `create_schema` names a type from the model's own phrasing, so a later
//   chat could add a second venue type (`event_place` beside `event_venue`)
//   and every scenario after it chose among three types, not two.
// - A setup turn that replied "that type already exists" without a call failed
//   its assertion, and the scenario after it was not scored. A full run lost
//   up to five scenarios that way.
// - A `--only` run and a full run put different histories ahead of the same
//   scenario, so the two could disagree.
//
// Seeded, every scenario is the first turn of its own chat (after its
// `priorTurns`, if any) against the same two types, whichever chats ran first.
// ---------------------------------------------------------------------------

/// The two custom types every scenario runs against.
///
/// Their ids are named literally, unlike every ASSERTION in this file: a seed
/// establishes state, so there is nothing here for a model-derived id to
/// invalidate. The schema assertions still match on a pattern, because those
/// score what the model chose.
///
/// The venue type has no `name` field: a record's name is its title, and that
/// is the shape the agent gives a type asked for "with a name, a booking date
/// and a capacity". The company type keeps the `name` field it has always been
/// seeded with, so its scenarios are scored against the type they were
/// measured on.
const COMPANY_TYPE = "company_sold_to";
const VENUE_TYPE = "event_venue";
const SIGNED_DATE_FIELD = "signed_date";

const SEEDED_TYPES: Array<{ id: string; params: Record<string, unknown> }> = [
  {
    id: COMPANY_TYPE,
    params: {
      name: COMPANY_TYPE,
      description: "A company we sell to, and the date we signed them",
      fields: [
        { name: "name", type: "text" },
        { name: SIGNED_DATE_FIELD, type: "date" },
      ],
    },
  },
  {
    id: VENUE_TYPE,
    params: {
      name: "Event Venue",
      description: "A place we hold events, with its booking date and capacity",
      fields: [
        { name: "booking_date", type: "date" },
        { name: "capacity", type: "number" },
      ],
    },
  },
];

/// The company instance the entity scenarios act on.
///
/// Seeded out of band rather than by a turn, for two reasons. First, a turn's
/// writes are replayed into every later turn as terse facts carrying the id
/// inline, so seeding by turn would put the answer in the prompt as literal
/// history (see `EvalFixture.seedGroup`'s contract, which `seedRun` shares).
/// Second, a turn lengthens the conversation, and length alone was enough to
/// make the model stop emitting tool calls.
const SEEDED_COMPANY_TITLE = "Northwind Trading";
const SEEDED_COMPANY_SIGNED = "2025-03-14";
/// That date as a reply may write it: ISO, or the day and month in words.
const SEEDED_COMPANY_SIGNED_IN_REPLY =
  /2025-03-14|\bMar(ch|\.)?\s+14(th)?\b|\b14(th)?\s+(of\s+)?Mar(ch|\.)?\b|\b0?3\/14\/(20)?25\b|\b14\/0?3\/(20)?25\b/i;

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
 * The schema ids in `schema list --json` output. Pure, so the shape this
 * fixture depends on is pinned by a test rather than discovered on a warm
 * database: a reader of a shape the CLI no longer emits finds no schema at
 * all, which looks like an empty workspace and re-creates every type.
 */
export function listedSchemaIds(output: unknown): string[] {
  const schemas = (output as { schemas?: unknown } | null)?.schemas;
  if (!Array.isArray(schemas)) return [];
  return schemas.flatMap((s) => {
    const id = (s as { id?: unknown } | null)?.id;
    return typeof id === "string" && id !== "" ? [id] : [];
  });
}

/** The id of every schema in the workspace. */
function listSchemaIds(env: EvalEnv): Set<string> {
  return new Set(listedSchemaIds(runNs(env, ["schema", "list"])));
}

/**
 * Create the two custom types and the Northwind instance the scenarios run
 * against.
 *
 * Runs as `seedRun`, once per rep, before any chat. `--between-runs` wipes the
 * database between reps, so nothing a rep needs can be left to an earlier one,
 * and a seed hung on `seedGroup` would run inside the chat loop.
 *
 * Idempotent, and that is load-bearing rather than defensive: a run without a
 * between-runs command, or one whose reset failed, must not leave rep 2 with
 * two Northwinds and rep 3 with three. The name would stop resolving to a
 * single node, and the failures would read as model non-determinism.
 */
function seedWorkspace(env: EvalEnv): void {
  const types = listSchemaIds(env);
  for (const type of SEEDED_TYPES) {
    if (types.has(type.id)) continue;
    runNs(env, ["schema", "create", "--params", JSON.stringify(type.params)]);
  }
  // The daemon derives a type's id from its name. A seed that came back under
  // another id would leave every scenario choosing among types the fixture
  // does not know it has.
  const after = listSchemaIds(env);
  const missing = SEEDED_TYPES.filter((t) => !after.has(t.id)).map((t) => t.id);
  if (missing.length > 0) {
    throw new Error(`seeded type(s) missing after schema create: ${missing.join(", ")}`);
  }

  const existing = runNs(env, [
    "node",
    "query",
    "--type",
    COMPANY_TYPE,
    "--limit",
    "50",
  ]) as { nodes?: Array<{ content?: string; id?: string }> } | null;
  const present = (existing?.nodes ?? []).find(
    (n) => (n?.content ?? "").toLowerCase() === SEEDED_COMPANY_TITLE.toLowerCase(),
  );

  // An existing Northwind is given the date too: a rep whose update failed
  // after the create would otherwise leave it dateless for every rep after,
  // and `outcome-record-field-is-answered` is scored on the reply carrying it.
  const id =
    present?.id ??
    (
      runNs(env, [
        "node",
        "create",
        "--type",
        COMPANY_TYPE,
        "--content",
        SEEDED_COMPANY_TITLE,
      ]) as { id?: string } | null
    )?.id;
  if (!id) throw new Error(`seeding '${SEEDED_COMPANY_TITLE}' returned no id`);

  runNs(env, [
    "node",
    "update",
    id.replace(/^nodespace:\/\//, ""),
    "--property",
    `${SIGNED_DATE_FIELD}=${SEEDED_COMPANY_SIGNED}`,
  ]);
}

/// The type the linked-skill scenarios act on, and the only one their turns
/// are held to.
const LINKED_TYPE = "warranty_claim";

/// Skills linked to `LINKED_TYPE` through `applies_to`.
///
/// Three, because a turn has an offered set only when every tool-bearing
/// candidate that clears its bar is linked, and Stage 2 judges the top three:
/// fewer would leave room for an unlinked built-in, and the turn would not be
/// held. Their descriptions share the scenarios' vocabulary so that they, and
/// not the built-ins, lead retrieval for a warranty request.
const LINKED_SKILLS: Array<{ name: string; description: string; tools: string[] }> = [
  {
    name: "Warranty Claim Intake",
    description:
      "File a warranty claim: record a new warranty claim for a product with its status and the date it was filed.",
    tools: ["search_nodes", "create_node"],
  },
  {
    name: "Warranty Claim Follow-up",
    description:
      "Follow up on a warranty claim: find the warranty claim and change its status or add what happened next.",
    tools: ["search_nodes", "update_node", "create_node"],
  },
  {
    name: "Warranty Claim Lookup",
    description:
      "Look up warranty claims: list warranty claims by product, status or the date they were filed.",
    tools: ["search_nodes"],
  },
];

/**
 * Create `LINKED_TYPE` and the skills linked to it, then wait until the skills
 * are retrievable.
 *
 * Idempotent, for the reason `seedWorkspace` is: a rep whose database was not
 * wiped must not end up with two of each skill.
 *
 * Runs as `seedGroup` for the linked-skill scenarios only, which sit last in
 * the fixture. Three more skills and one more custom type change what
 * retrieval returns for every request, so seeding them per run would change
 * what every other scenario is scored against.
 *
 * That holds within a rep. Across reps it needs `--between-runs` to wipe the
 * database: without it, rep 2 starts with what rep 1's last groups seeded.
 *
 * A skill whose `applies_to` call failed is found by name on the next seed and
 * left unlinked. Its scenario then fails as not held, which names the cause.
 */
function seedLinkedSkills(env: EvalEnv): void {
  if (!listSchemaIds(env).has(LINKED_TYPE)) {
    runNs(env, [
      "schema",
      "create",
      "--params",
      JSON.stringify({
        name: "Warranty Claim",
        description: "A warranty claim filed against a product we sold",
        fields: [
          { name: "product", type: "text" },
          {
            name: "status",
            type: "enum",
            coreValues: [
              { value: "open", label: "Open" },
              { value: "approved", label: "Approved" },
              { value: "rejected", label: "Rejected" },
            ],
          },
          { name: "filed_date", type: "date" },
        ],
      }),
    ]);
  }

  const existing = runNs(env, ["node", "query", "--type", "skill", "--limit", "100"]) as {
    nodes?: Array<{ content?: string }>;
  } | null;
  const present = new Set((existing?.nodes ?? []).map((n) => n?.content ?? ""));
  for (const skill of LINKED_SKILLS) {
    if (present.has(skill.name)) continue;
    const created = runNs(env, [
      "node",
      "create",
      "--type",
      "skill",
      "--content",
      skill.name,
      "--property",
      `description=${skill.description}`,
      "--property",
      `tool_whitelist=${JSON.stringify(skill.tools)}`,
    ]) as { id?: string } | null;
    const id = created?.id?.replace(/^nodespace:\/\//, "");
    if (!id) throw new Error(`seeding skill '${skill.name}' returned no id`);
    runNs(env, [
      "relationship",
      "create",
      "--from",
      id,
      "--type",
      "applies_to",
      "--to",
      LINKED_TYPE,
    ]);
  }

  // A skill is retrievable only once its embedding lands, on a ~30s debounce.
  // A turn sent before that would route to the built-ins and not be held.
  awaitSkillIndex(env);
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
  {
    id: "outcome-list-types",
    scenario: "Outcome: asked which types exist, the reply names built-in and custom ones",
    // The context block lists only the custom types related to the message, so
    // a turn that answers from it names those and none of the built-in ones.
    // Measured on the locked model before `search_nodes` said where the full
    // list comes from: 3 of 3 reps answered with the two custom types and
    // called nothing. The custom type is matched loosely because its id is
    // model-derived on a warm rep, and the model escapes underscores in a list.
    prompt: "What schemas do we have here?",
    expected: {
      decision: "outcome",
      listsTypes: [/\btask\b/i, /\bperson\b/i, /\bproject\b/i, /compan/i],
    },
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
  // not instruction quality — the same class as ADR-038 Finding 4.
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

  // ── Skill routing (questions about stored knowledge) ───────────────────
  //
  // A request to find something and a question about how something works are
  // both lookups, and neither says so in a way retrieval can match unaided:
  // "find" appears in several skills' vocabulary, and a question has no verb
  // at all. These score whether the turn leads with the lookup skill. None of
  // the topics below exists in the seeded workspace, and none needs to — the
  // skill decision is recorded before any search runs.
  //
  // The topics deliberately avoid every seeded skill's own subject (merging,
  // conflicts, deleting, importing). A question about one of those is
  // genuinely close to two skills, and scoring it would measure which noun
  // the fixture happened to pick.
  {
    id: "skill-find-request",
    scenario: "Skill: a request to find stored material routes to Research & Search",
    prompt: "Could you find the write-up on how we rotate API keys?",
    expected: { decision: "skill", matches: /research/i },
    knowledgeQuestion: true,
  },
  {
    id: "skill-question-how",
    scenario: "Skill: a how-does-it-work question routes to Research & Search",
    prompt: "How does our retry policy for failed uploads work?",
    expected: { decision: "skill", matches: /research/i },
    knowledgeQuestion: true,
  },
  {
    id: "skill-question-what",
    scenario: "Skill: a what-is question routes to Research & Search",
    prompt: "What is the rollout plan for the billing change?",
    expected: { decision: "skill", matches: /research/i },
    knowledgeQuestion: true,
  },
  {
    id: "skill-question-explain",
    scenario: "Skill: an explain-why request routes to Research & Search",
    prompt: "Explain why the scheduler got split into two services.",
    expected: { decision: "skill", matches: /research/i },
    knowledgeQuestion: true,
  },
  {
    id: "outcome-question-searches-first",
    scenario: "Outcome: a question about stored knowledge is searched before it is answered",
    // The failure this pins is a turn that made no call at all: it replied
    // that nothing in the conversation covered the topic and asked the user
    // for more. Whether the search then finds anything is not scored — the
    // workspace holds nothing on this topic, and "I searched and found
    // nothing" is the correct answer.
    prompt: "How do we decide which venue gets a deposit refund?",
    expected: { decision: "outcome", searchesBeforeReplying: true },
    knowledgeQuestion: true,
  },
  {
    id: "op-own-skills-no-search",
    scenario: "Operation: a question about the agent's own skills is not a search",
    // Skills are system nodes, outside the default search scope, so a search
    // for one comes back empty and the turn reports that no such skill
    // exists. The registry is listed in the prompt; the answer is there.
    prompt: "Which skills do you have?",
    expected: { decision: "operation", oneOf: [] },
    knowledgeQuestion: true,
  },
  {
    id: "outcome-find-own-skill",
    scenario: "Outcome: asked to find one of its own skills, the agent names it",
    // Phrased as a find request, so Stage 1 routes it as a lookup and the turn
    // is searched. A search cannot return a skill, and the reported failure
    // was the reply that followed: "I couldn't find a specific skill". The
    // skill list is in the prompt; the reply has to come from it.
    prompt: "Could you find the skill you would use to look something up?",
    // The skill's name, not the word: "I researched your notes and couldn't
    // find a specific skill" is the reported failure with one word changed.
    expected: { decision: "outcome", replyMatches: /research\s*(&|and)\s*search/i },
    knowledgeQuestion: true,
  },
  {
    id: "outcome-record-field-is-answered",
    scenario: "Outcome: a question about a record's field is answered with its value",
    // The date is a field of the record, not text in it, so the reply carries
    // it only when the fields reached the model. A record read as a document
    // came back as its title alone, and the reply was that the signing date
    // "is not visible"; which way the model reads a record varies between
    // runs of the same prompt, so this is scored on the reply, not the call.
    //
    // Needs the seeded date, which `seedWorkspace` sets or fails the run.
    prompt: "When did we sign Northwind Trading?",
    expected: { decision: "outcome", replyMatches: SEEDED_COMPANY_SIGNED_IN_REPLY },
    knowledgeQuestion: true,
    entityResolution: true,
  },
  {
    id: "outcome-follow-up-keeps-its-answer",
    scenario: "Outcome: a follow-up on the last answer still answers about it",
    // A follow-up has nothing to look up and nothing to change, so the reply
    // is the model's own, from the conversation. Two things in the loop could
    // take that reply away: a follow-up routed as a lookup is searched first,
    // and a chat that has only asked questions had a tool-free reply put back
    // once to act. Measured then: the model answered "March 14, 2025", was
    // told to act, and said it did not know what was meant. A question is no
    // longer put back in an intent that composed no clarifying question, so
    // this one scores the first path, and the scenario after it the second.
    priorTurns: ["When did we sign Northwind Trading?"],
    prompt: "Can you say that again more simply?",
    // The company, not the date. Whether the turn before found the date is
    // `outcome-record-field-is-answered`'s subject, not this one's. Either
    // answer said again names the company.
    // Every reply this scenario exists to catch ("I'm not sure what you
    // mean", "I don't see a record of that in this conversation", a request
    // to confirm) names nothing.
    expected: { decision: "outcome", replyMatches: /northwind/i },
    knowledgeQuestion: true,
    entityResolution: true,
  },
  {
    id: "outcome-follow-up-statement-keeps-its-answer",
    scenario:
      "Outcome: a follow-up that is not a question still answers about the last answer",
    // The same follow-up without the shape of a question. This one is put
    // back to act when the reply makes no call, and the reply set aside is
    // the one that stands unless the put-back produces a call.
    //
    // The put-back needs two turns behind it that wrote nothing, so there are
    // two questions ahead of the follow-up: one question alone would leave
    // the reply to stand untouched.
    priorTurns: [
      "When did we sign Northwind Trading?",
      "Which company was that about?",
    ],
    prompt: "Say that again more simply.",
    expected: { decision: "outcome", replyMatches: /northwind/i },
    knowledgeQuestion: true,
    entityResolution: true,
  },

  // ── Schema selection ───────────────────────────────────────────────────
  //
  // Every type asserted on here is USER-DEFINED, one of the two seeded
  // above. Core seeded types (`project`, `task`, `person`, ...) cannot be used:
  // `non_core_schema_hits` strips them from schema candidates, so
  // a turn acting on one gets an EMPTY candidate set and the model emits the
  // type name from general context with nothing to select among. Measured
  // directly — "How many projects do we have?" produced
  // `selected=project [off-menu] candidates=`. That is unconstrained
  // generation, not a selection, and scoring it as one would measure the
  // `is_core` filter rather than the model.
  {
    id: "schema-on-menu-company",
    scenario: "Schema: a name that already exists is an update, not a create",
    // Ambiguous BY DESIGN now that the workspace is seeded with the
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
    // Both seeded types carry a date, but only the venue has a capacity — so a
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

  // ── Turns held to their skills' linked types ───────────────────────────
  //
  // Last in the fixture, and they must stay last: their group seeds three
  // linked skills and a custom type (`seedLinkedSkills`), which every later
  // group in the rep would then be scored against.
  //
  // A turn is held only when every tool-bearing candidate that clears its bar
  // is linked. That is a retrieval outcome, so both scenarios can fail on it
  // before the model decides anything, and the failure text says which.
  {
    id: "held-on-menu-type",
    scenario: "Held turn: a request for the linked type acts on it",
    // The control. Holding a turn to its skills' types must not stop it doing
    // what those skills are for.
    prompt: "File a warranty claim for the Aurora blender, its motor failed, filed today.",
    expected: { decision: "schema", onMenu: true },
    linkedSkills: true,
  },
  {
    id: "held-off-menu-type",
    scenario: "Held turn: a type outside the linked set does not run",
    // Asks for a kind of record the linked skills are not about, in their
    // vocabulary, so retrieval still leads with them. Unheld, the model names
    // `task` and the call runs. Held, a call naming `task` is refused, and the
    // turn should end with the user asked or told. Creating the reminder as a
    // warranty claim instead fails, whether the model does it after a refusal
    // or by following the `enum` on its first call.
    prompt:
      "For the Aurora blender warranty claim, add a task to ring the buyer back on Friday.",
    expected: { decision: "outcome", heldToOfferedTypes: true },
    linkedSkills: true,
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

/** The tools that look things up in the user's stored knowledge. */
const SEARCH_TOOLS = ["search_semantic", "search_nodes"];

/** Score a turn that must have searched before replying. */
function assertSearched(turns: TurnRecord[]): Verdict {
  const called = turns.flatMap((t) => t.toolsCalled);
  if (called.some((name) => SEARCH_TOOLS.includes(name))) {
    return { passed: true };
  }
  const reply = turns.at(-1)?.reply ?? "";
  return {
    passed: false,
    failure:
      `Replied without searching. Tools: ${called.join(", ") || "(none)"}. ` +
      `Reply: ${reply.slice(0, 300)}`,
  };
}

/**
 * Score a turn that must list the workspace's types: a `search_nodes` call
 * must have succeeded, and the reply must name every type in `types`.
 */
function assertListsTypes(types: RegExp[], turns: TurnRecord[]): Verdict {
  // Same reasoning as `assertNoDuplicate`: without per-call outcomes a search
  // that errored cannot be told from one that returned the list.
  if (turns.some((t) => t.toolsCalled.length > 0 && t.toolCalls === undefined)) {
    return {
      passed: false,
      failure:
        "Tools were called but no per-call outcomes were recorded, so a failed " +
        "search cannot be told from one that returned the types.",
    };
  }
  const reply = turns.at(-1)?.reply ?? "";
  const calls = turns.flatMap((t) => t.toolCalls ?? []);
  if (!calls.some((c) => c.name === "search_nodes" && !c.isError)) {
    return {
      passed: false,
      failure:
        `No search_nodes call succeeded, so the reply was not read off the workspace's types. ` +
        `Tools: ${turns.flatMap((t) => t.toolsCalled).join(", ") || "(none)"}. Reply: ${reply.slice(0, 300)}`,
    };
  }
  const missing = types.filter((t) => !t.test(reply));
  if (missing.length > 0) {
    return {
      passed: false,
      failure: `The reply does not name ${missing.join(", ")}. Reply: ${reply.slice(0, 300)}`,
    };
  }
  return { passed: true };
}

/**
 * Whether dispatch held this turn's type arguments to an offered set.
 *
 * Read from the schema decisions: one is `enforced` when the turn has an
 * offered set and the round's selection, if any, came from a call dispatch
 * holds. A held turn records one for every round, so any is enough.
 */
function turnHeld(turn: TurnRecord): boolean {
  return turn.decisions?.some((d) => d.kind === "schema" && d.enforced) ?? false;
}

/**
 * Whether a call naming an off-menu type reached the executor on this turn.
 *
 * Observed, not inferred: the daemon reports it per call. Counting off-menu
 * decisions against refused calls looked equivalent and was not, because a
 * round's decision is recorded before dispatch and several guards stop a call
 * without refusing its type (an identical re-send, a `route_clarify` in the
 * same round, a duplicate write). Each of those read as a call that ran.
 */
function offMenuCallRan(turn: TurnRecord): boolean {
  return (turn.toolCalls ?? []).some((c) => c.offMenuRan);
}

/**
 * Whether a call persisted something.
 *
 * `isError` alone does not say so: a call skipped for a `route_clarify` in the
 * same round, or answered by the duplicate-write guard, is not an error and
 * wrote nothing. A write that landed reports a field count or says it was
 * content-only.
 */
function persisted(call: ToolCallRecord): boolean {
  return !call.isError && (call.fieldCount !== undefined || call.contentOnly === true);
}

/** Whether a `create_node` landed anywhere on these turns. */
function createdRecord(turns: TurnRecord[]): boolean {
  return turns
    .flatMap((t) => t.toolCalls ?? [])
    .some((c) => c.name === "create_node" && persisted(c));
}

/**
 * What a turn did after dispatch first refused a call for its type.
 *
 *   created    a `create_node` landed afterwards: the record the user asked
 *              for was created as one of the offered types
 *   clarified  the turn put the choice to the user
 *   replied    neither: the turn ended in a prose reply
 *
 * `undefined` when nothing was refused. This is one cost of holding a turn: a
 * refusal that is followed by a create has turned a wrong type into a wrong
 * record. The other cost needs no refusal (see `createdRecord`, which counts
 * the same event wherever in the turn it happens).
 *
 * Only a create is counted. An `update_node` after a refusal changes a record
 * that exists; it does not write the requested one as another type.
 */
function afterRefusal(turns: TurnRecord[]): "created" | "clarified" | "replied" | undefined {
  const calls = turns.flatMap((t) => t.toolCalls ?? []);
  const first = calls.findIndex((c) => c.typeRefused);
  if (first === -1) return undefined;
  const later = calls.slice(first + 1);
  if (later.some((c) => c.name === "create_node" && persisted(c))) return "created";
  const reply = turns.at(-1)?.reply ?? "";
  if (later.some((c) => c.name === "route_clarify") || reply.startsWith(CLARIFICATION_OPENER)) {
    return "clarified";
  }
  return "replied";
}

/** The verdict for a linked-skill scenario whose turn was not held. */
function notHeld(turns: TurnRecord[]): Verdict {
  const skill = firstDecision(turns, "skill");
  return {
    passed: false,
    failure:
      `No schema decision on this turn was enforced, so nothing here was measured. ` +
      `A turn is held only when every tool-bearing skill that clears its bar is linked. ` +
      `Retrieved: ${skill?.candidates.join(", ") || "(nothing)"}`,
  };
}

/**
 * Score a turn whose request is for a record of a type outside the linked set.
 * The turn must be held, no call on an off-menu type may run, and no record
 * may be created at all: on a held turn any record created is of an offered
 * type, which is not the type the user asked for. That holds whether the model
 * was refused first or followed the `enum` on its first call.
 */
function assertHeldToOfferedTypes(turns: TurnRecord[]): Verdict {
  if (!turns.some(turnHeld)) return notHeld(turns);
  const tools = turns.flatMap((t) => t.toolsCalled).join(", ");
  if (turns.some(offMenuCallRan)) {
    const schema = firstDecision(turns, "schema");
    return {
      passed: false,
      failure:
        `A call naming a type outside the offered set ran on a held turn ` +
        `(offered: ${schema?.candidates.join(", ")}). Tools: ${tools}`,
    };
  }
  if (createdRecord(turns)) {
    return {
      passed: false,
      failure:
        `A record was created on a turn held to types the request was not for, so it ` +
        `was written as a type the user did not ask for ` +
        `(${afterRefusal(turns) === "created" ? "after a refusal" : "not after a refusal"}). ` +
        `Tools: ${tools}`,
    };
  }
  return { passed: true };
}

function assertFixture(
  fixture: DecisionScenario,
  turns: TurnRecord[],
): Verdict {
  const { expected } = fixture;
  if (expected.decision === "outcome") {
    if ("heldToOfferedTypes" in expected) return assertHeldToOfferedTypes(turns);
    if ("listsTypes" in expected) {
      return assertListsTypes(expected.listsTypes, turns);
    }
    if ("replyMatches" in expected) {
      const reply = turns.at(-1)?.reply ?? "";
      return expected.replyMatches.test(reply)
        ? { passed: true }
        : {
            passed: false,
            failure: `The reply does not match ${expected.replyMatches}. Reply: ${reply.slice(0, 300)}`,
          };
    }
    return "noDuplicateOf" in expected
      ? assertNoDuplicate(expected.noDuplicateOf, turns)
      : assertSearched(turns);
  }
  // A linked-skill scenario scores a held turn. On an open one its assertion
  // can still pass, having measured nothing about holding.
  if (fixture.linkedSkills && !turns.some(turnHeld)) return notHeld(turns);
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
      if (selected !== null) {
        return {
          passed: false,
          failure: `Expected no tool call, but called '${selected}'. Offered: ${decision.candidates.join(", ")}`,
        };
      }
      // The first round is not the whole turn: the loop can make a call the
      // model did not. "No tool" means none anywhere in it.
      const called = turns.flatMap((t) => t.toolsCalled);
      return called.length === 0
        ? { passed: true }
        : {
            passed: false,
            failure: `Expected no tool call. The first round made none, but the turn went on to call: ${called.join(", ")}`,
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
  // One chat per scenario.
  //
  // A single chat once held all of them, and by the later ones the model
  // stopped emitting tool calls at all: a measured run showed every scenario
  // after the fourth recording `toolsCalled: []`, including
  // `skill-instance-request`, which passes in isolation. That is conversation
  // length being scored as decision quality.
  //
  // The chats still share the rep's database, so a record or a type one
  // scenario creates is there for the ones after it. The two seeded types and
  // the seeded company are the same for all of them (see `seedWorkspace`).
  groups: FIXTURES.map((scenario) => [scenario]),
  // Per REP, before any chat: `--between-runs` wipes the database between
  // reps, so every rep starts cold and has to be seeded again.
  seedRun(env: EvalEnv) {
    seedWorkspace(env);
  },
  // Per group, and only for the linked-skill scenarios: see `seedLinkedSkills`
  // for why these are kept out of every other scenario's workspace.
  seedGroup(env: EvalEnv, group) {
    if (group.some((s) => (s as DecisionScenario).linkedSkills)) {
      seedLinkedSkills(env);
    }
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
      knowledgeQuestion: s.knowledgeQuestion ?? false,
      linkedSkills: s.linkedSkills ?? false,
      skillDecision: firstDecision(turns, "skill") ?? null,
      operationDecision: firstDecision(turns, "operation") ?? null,
      schemaDecision: firstDecision(turns, "schema") ?? null,
      // Recorded for every scenario, not just failing ones: a type named that
      // was never offered is the single most diagnostic signal here, and it
      // needs to be visible in the results file without a re-run.
      offMenu: turns.some((t) => t.decisions?.some((d) => d.offMenu)),
      // Whether dispatch held the turn to an offered set, how many calls it
      // refused for naming a type outside it, and whether an off-menu call
      // reached the executor anyway. The last two are the daemon's own
      // per-call report, not something worked out from the decisions.
      held: turns.some(turnHeld),
      typeRefusals: turns
        .flatMap((t) => t.toolCalls ?? [])
        .filter((c) => c.typeRefused).length,
      offMenuRan: turns.some(offMenuCallRan),
      // A record created on a linked-skill turn, refusal or not. For the
      // off-menu scenario that is the record written as the wrong type.
      createdRecord: createdRecord(turns),
      afterRefusal: afterRefusal(turns) ?? null,
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
    const held = results.filter((r) => r.extra?.held === true).length;
    const typeRefusals = results.reduce(
      (sum, r) => sum + Number(r.extra?.typeRefusals ?? 0),
      0,
    );
    const offMenuRan = results.filter(
      (r) => r.extra?.offMenuRan === true,
    ).length;
    const after = (what: string) =>
      results.filter((r) => r.extra?.afterRefusal === what).length;
    // Held turns whose request was for a type outside the set, and how many of
    // them created a record anyway: of an offered type, so the wrong one.
    const offMenuRequests = results.filter(
      (r) =>
        r.extra?.held === true &&
        (r.extra.expected as { heldToOfferedTypes?: boolean })?.heldToOfferedTypes === true,
    );
    const wrongTypeRecords = offMenuRequests.filter(
      (r) => r.extra?.createdRecord === true,
    ).length;
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
      `Questions about stored knowledge: ${count((e) => e.knowledgeQuestion === true)}`,
      // Reported separately from everything else: an aggregate hides the one
      // asymmetry three independently-measured models have all shown, and this
      // is the dimension the entity tier is NOT expected to have fixed.
      `Declarative intent (entity resolved): ${count((e) => e.declarative === true)}`,
      `Ambiguous (several types plausible): ${count((e) => e.ambiguous === true)}`,
      `Entity resolution: ${count((e) => e.entityResolution === true)}`,
      `ADR-056 known failures: ${count((e) => e.adr056 === true)}`,
      `Off-menu type named: ${offMenu}`,
      `Held to an offered set: ${held} (${count((e) => e.linkedSkills === true)} of the linked-skill scenarios passed); ` +
        `${typeRefusals} off-menu call(s) refused at dispatch, ${offMenuRan} ran`,
      `After a refusal: ${after("clarified")} asked the user, ${after("replied")} replied in prose, ` +
        `${after("created")} created the record as an offered type`,
      `Requests for a type outside a held turn's set: ${offMenuRequests.length}; ` +
        `${wrongTypeRecords} created a record as an offered type instead`,
      `Stage-1 decision cost: ${meanRouting}ms mean (one or two generative passes for a 4-way structural choice)`,
    ];
  },
};

export default fixture;
