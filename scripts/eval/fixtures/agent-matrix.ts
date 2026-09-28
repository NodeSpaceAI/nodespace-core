/**
 * Agent-behavior eval — end-to-end graph outcomes.
 *
 * Asserts a structured, machine-checkable expectation per scenario rather than
 * capturing prose for a human to read: what the user's graph looks like when
 * the turn ends.
 *
 * This is the third eval layer, distinct from the other two:
 *   - packages/agent/src/golden     — tool-call EMISSION shape (golden corpus)
 *   - scripts/eval/fixtures/routing — skill ROUTING accuracy (which skill fires)
 *   - this                          — END-TO-END behavior (did the requested
 *                                     change actually reach storage)
 *
 * OUTCOME, NOT TRAJECTORY
 *
 * This fixture used to score TRAJECTORY — which tool fired, how many times, in
 * what order — and that disagreed with the product in three measured ways:
 *
 *   - A correct result scored as a failure. A model reaching the right end
 *     state by a shorter path (update_node with no preceding resolve_query)
 *     lost a point for the path rather than the result.
 *   - Self-correction scored as a failure. A create_node rejected for a missing
 *     node_type, corrected by the model and persisted on the second call, red-
 *     lined on an exactly-once rule — punishing the recovery behavior we want.
 *   - Severity was flat. Two search_nodes calls (wasted latency, nothing
 *     persisted) scored identically to two create_schema calls (a spurious type
 *     the user has to clean up).
 *
 * Each scenario's `end` clause is now THE score, and severity falls out of it
 * for free rather than needing a severity table: a repeated read changes
 * nothing and passes, while a repeated schema creation leaves an extra type
 * behind and fails `createdSchemas`. See ../end-state.ts for the clause model
 * and ../graph.ts for how end state is captured.
 *
 * `expect` — the old trajectory assertion — is KEPT on every scenario and
 * still evaluated, but as a diagnostic recorded beside the score rather than as
 * the score. It answers the question outcome grading cannot ("how did the model
 * get there"), which is what a debugging session actually needs, and a scenario
 * where the two disagree is the signal worth reading after a run.
 *
 * Under ADR-038 routing happens in a separate stage before the acting turn, so
 * trajectory assertions check for the TARGET tool tolerating routing calls,
 * never raw tool count.
 *
 * THE DOMAIN IS PART OF THE MEASUREMENT, NOT DECORATION
 *
 * Model-lock decisions (ADR-046, ADR-056) and every model re-evaluation since
 * are scored off this matrix, so whatever domain the scenarios are written in
 * is the domain native-model selection is actually being decided on. That made
 * the original scenario set — equipment checkouts, album and venue trackers —
 * an active problem rather than a cosmetic one: it selected models on their
 * ability to track laser cutters, while the product's claim is context
 * infrastructure for AI-native development.
 *
 * The scenarios are therefore written in NodeSpace's own working domain:
 * feature write-ups and their sign-off state, the calls a team makes about how
 * a system is built, planning cycles, work tied to the decision that
 * constrains it.
 *
 * The MECHANICS are unchanged by that re-theme and are the reason the scenario
 * set was re-themed in place rather than duplicated into a second fixture: the
 * expectation clauses are properties of the expectation model, not of the
 * vocabulary, so re-theming keeps every one of them — and keeps every scenario
 * `id`, so a pre-re-theme baseline still joins against a post-re-theme run.
 *
 * GROUPS CASCADE, SO SETUP IS NOT SCORED
 *
 * Scenarios in a group share a chat node and run in order, so an early failure
 * craters the rest and the results stop being independent observations. One
 * ambiguous verb in 11a once cost three points: itself, plus 11c (which had no
 * node to link) and 11d (which had no edge to traverse) — one failure counted
 * three times, inflating variance and making per-scenario counts misleading.
 *
 * Turns that exist only to establish state (11a, 11b) are therefore marked
 * `setup: true`: run and recorded, asserted, warned about if they fail, but
 * NOT scored. Their state still reaches their successors; their phantom
 * failures no longer reach the denominator.
 *
 * WINNABILITY IS A HARD CONSTRAINT ON WORDING
 *
 * Each chain builds its own schema in its first scenario, and every later
 * prompt in that chain must name only values that schema can actually hold. A
 * prompt asking for a field the type has nowhere to put is unwinnable: the
 * model degrades reasonably (folding the value into the node's text) and
 * scores red for it, and the fixture is then measuring itself rather than the
 * model. See scenario 9's note and #1846.
 *
 * Scenario wording must stay independent of packages/agent/src/agent_guidance.rs.
 * `guidance_is_not_contaminated_by_eval_prompts` enforces it by parsing the
 * `prompt:` literals out of this file: a prompt that also appears in guidance
 * turns a passing scenario into proof that the model can copy a memorized
 * example, and prompt tuning then has a degenerate solution. The dev-workflow
 * domain raises that risk rather than lowering it — the seeded skill
 * descriptions already name Spec, ADR and Ticket — so prompts here are written
 * around those terms and checked against the guard, not assumed clear of it.
 */

import type { EvalEnv } from "../env.ts";
import type {
  EvalFixture,
  Scenario,
  ScenarioGroup,
  ToolCallRecord,
  TurnRecord,
  Verdict,
} from "../types.ts";
import { assertEndState, turnAskedForClarification, type EndState } from "../end-state.ts";

// ---------------------------------------------------------------------------
// Structured expectation model
// ---------------------------------------------------------------------------

export type Expectation =
  // No graph-action tool fired at all (routing tools tolerated).
  | { kind: "noTools" }
  // The named tool fired exactly once (ignoring routing tools).
  //
  // `minProperties` additionally requires the call to have persisted at least
  // that many schema field values. Without it a create_node that recorded none
  // of the user's particulars — no cost, no date, no status — scores identically
  // to one that recorded them all, because the tool name is all that is checked.
  // Set it on any scenario whose prompt supplies particulars a later scenario
  // depends on, or that chain silently keys on a value nothing ever stored.
  | { kind: "toolOnce"; tool: string; minProperties?: number }
  // Tools fired in this order as a subsequence (ignoring routing tools) — other
  // tools may appear between/around them, but these must appear in this order.
  //
  // `minProperties` carries the same meaning as on `toolOnce`, applied to
  // `propertiesOn` (defaulting to the last tool in the sequence). A
  // resolve-then-act chain can call exactly the right tools in exactly the
  // right order and still drop the user's request: an update_node that
  // resolved the correct node but sent only its id changes nothing, yet
  // scores identically to one that persisted the state change, because the
  // tool name is all that is checked. Set it on any chain whose final call
  // must carry a value for the turn to have accomplished anything.
  | {
      kind: "toolSequence";
      tools: string[];
      minProperties?: number;
      propertiesOn?: string;
    }
  // The named tool did not fire more than once in a row (no blind retry loop).
  //
  // `minCalls` additionally requires the tool to have fired at least that many
  // times. Without it this expectation is satisfied by a turn in which the tool
  // never fired at all — the loop that detects a repeat never executes — so a
  // model that stopped and asked the user instead of searching scores
  // identically to one that searched correctly. That is the failure mode on the
  // read side, so a scenario testing for it must opt in. Left off by default
  // rather than made unconditional: changing the shared semantics would also
  // re-score existing scenarios, and single-run matrix numbers are not
  // decision-grade enough to absorb that silently.
  | { kind: "noRetry"; tool: string; minCalls?: number }
  // Exactly one create_schema call in this turn (no proactive related-type creation).
  | { kind: "noExtraTypes" };

/**
 * Tools that participate in ADR-038 pull-model routing but are not any
 * scenario's target action — tolerated anywhere in the sequence, never
 * asserted as "extra".
 *
 * Cross-referenced against Tool::ALL in
 * packages/agent/src/local_agent/tools.rs — update here if the registry changes.
 */
// Stage-1 routing calls (ADR-038). These are not actions the scenario is
// asserting on, so they are filtered out before the action-tool check.
// `search_skills` remains listed because the tool still exists for external
// agents; the local model is no longer offered it, so it should not appear in
// a local trace — tolerating it costs nothing and avoids a false failure if it
// ever does.
const ROUTING_TOOLS = ["route_query", "route_clarify", "search_skills"];

export function actionTools(toolsCalled: string[]): string[] {
  return toolsCalled.filter((t) => !ROUTING_TOOLS.includes(t));
}

/**
 * Check that a create_schema call actually produced a usable type.
 *
 * Counting the tool name is not enough, and the gap is not hypothetical: the
 * model has called create_schema with a title_template and no fields, been
 * rejected outright by title-template validation, and still scored a pass
 * because the name appeared once.
 *
 * Two distinct ways to call create_schema and end up with nothing usable:
 *   - the call is REJECTED (is_error) — nothing persisted at all;
 *   - the call SUCCEEDS with an empty field list. A call carrying neither
 *     `fields` nor `description` is valid by design and persists a type with no
 *     properties, against which the user cannot record anything. This one is
 *     invisible to any check that only looks at whether the call failed.
 *
 * `fieldCount` is absent (rather than 0) on results recorded before it was
 * captured, so absence is treated as unknown and passes — a stale baseline must
 * not read as a fresh failure.
 */
function schemaCallsAreSound(calls: ToolCallRecord[]): Verdict {
  for (const c of calls) {
    if (c.name !== "create_schema") continue;
    if (c.isError) {
      return {
        passed: false,
        failure:
          "create_schema was called but REJECTED — no schema persisted (the call " +
          "scores as a pass on tool name alone)",
      };
    }
    if (c.fieldCount === 0) {
      return {
        passed: false,
        failure:
          "create_schema succeeded but persisted a type with NO fields — nothing " +
          "can be recorded against it",
      };
    }
  }
  return { passed: true };
}

/**
 * Check that a call persisted at least `min` schema field values.
 *
 * The instance-side counterpart to `schemaCallsAreSound`, and the same failure
 * shape one level down: that one catches a schema with no fields to record
 * against, this catches a record with no field values in it. A create_node
 * carrying only `content` and `node_type` succeeds, persists a bare shell, and
 * scores green on tool name alone — while every later scenario that keys on one
 * of those missing values becomes unwinnable, and looks like a model failure
 * rather than a fixture that never stored the value.
 *
 * `fieldCount` is absent (rather than 0) on results recorded before the tool
 * reported it, so absence is treated as unknown and passes — for the same
 * reason `schemaCallsAreSound` does it: a stale baseline must not read as a
 * fresh failure.
 */
function callPersistedProperties(
  calls: ToolCallRecord[],
  tool: string,
  min: number,
): Verdict {
  for (const c of calls) {
    if (c.name !== tool) continue;
    // An errored call to the TARGET tool is a failure, not something to skip.
    // Once the tool-boundary gate rejects a no-op update_node, the reproducing
    // shape arrives here as `isError` — skipping it made the scenario score
    // green on exactly the defect it was added to catch. `schemaCallsAreSound`
    // already treats isError this way; this is the missing instance-side half.
    if (c.isError) {
      return {
        passed: false,
        failure:
          `${tool} was rejected, so the requested change never reached storage — ` +
          `the turn did not accomplish what the prompt asked for`,
      };
    }
    // The write reported that it had no properties to persist. That is a
    // complete success for a plain note or a rename, but this assertion is
    // only set on scenarios whose prompt DOES supply a value to store, so
    // here it means the value never made it into `properties`.
    if (c.contentOnly) {
      return {
        passed: false,
        failure:
          `${tool} changed only content and persisted no property values, but the ` +
          `prompt supplied a value to store — the requested state change was not recorded`,
      };
    }
    if (c.fieldCount === undefined) continue;
    if (c.fieldCount < min) {
      return {
        passed: false,
        failure:
          `${tool} succeeded but persisted ${c.fieldCount} property value(s), ` +
          `expected at least ${min} — the node was created without the ` +
          `particulars the prompt supplied, so anything keyed on them cannot resolve`,
      };
    }
  }
  return { passed: true };
}

/**
 * Check that the scenario's TARGET tool was not rejected.
 *
 * The name-counting assertions all shared a blind spot: a tool that fired the
 * right number of times but was REJECTED scores identically to one that
 * succeeded, because the tool name is all that is checked. `isError` was only
 * ever inspected on two narrow paths — `schemaCallsAreSound` (create_schema
 * only) and `callPersistedProperties` (only when a scenario opted into
 * `minProperties`) — so every scenario without `minProperties` was blind to it.
 *
 * That is not a hypothetical gap. It is the most likely failure shape for
 * `create_relationship`: the tool takes two node ids, the model has to recover
 * both, and two invented ids are rejected outright. Scoring that green means
 * the one scenario added to measure linking would report success on precisely
 * the failure it exists to catch. The same hole covered `search_nodes` on the
 * read-side scenarios, where a malformed filter is rejected and the turn
 * answers from nothing.
 *
 * Deliberately checks only the TARGET tool, not every call in the turn: a
 * model that tries a bad search, gets an error, recovers and then does the
 * right thing has still done the right thing. `schemaCallsAreSound` keeps its
 * own broader create_schema rule, which is about a DIFFERENT failure (a schema
 * that persisted with no fields) and is not subsumed by this.
 */
function targetToolWasNotRejected(
  calls: ToolCallRecord[],
  tool: string,
): Verdict {
  const own = calls.filter((c) => c.name === tool);
  if (own.length === 0) return { passed: true };
  // A rejection only sinks the turn if NOTHING to this tool ever succeeded.
  //
  // The distinction is load-bearing on `noRetry`, which deliberately tolerates
  // several non-adjacent calls: a first call rejected for a malformed argument,
  // corrected by the model, and retried successfully is a turn that ACCOMPLISHED
  // what the prompt asked for. Failing it would contradict that kind's own
  // semantics and score self-correction — the behaviour we want — as failure.
  //
  // Observed live on 11a: create_node was rejected for a missing node_type,
  // the model supplied it and the second call persisted. (That scenario still
  // reds, on `toolOnce`'s pre-existing exactly-once rule, which is a separate
  // and deliberate judgement about write tools; this helper is not what decides
  // it.)
  //
  // What stays red is the case this exists for: every call to the target tool
  // rejected, so the turn ends with nothing written while the tool name still
  // appears in the trace.
  if (own.some((c) => !c.isError)) return { passed: true };
  return {
    passed: false,
    failure:
      `${tool} fired ${own.length > 1 ? `${own.length} times, all ` : "but was "}` +
      `REJECTED — the turn scores as a pass on tool name alone while nothing ` +
      `it asked for actually happened`,
  };
}

/**
 * Decide whether a turn met its expectation.
 *
 * Pure and daemon-free so it is unit-testable without a model — see
 * scripts/eval/fixtures/agent-matrix.test.ts, which runs in `bun run test:all`.
 */
export function assertExpectation(
  expect: Expectation,
  toolsCalled: string[],
  toolCalls: ToolCallRecord[] = [],
): Verdict {
  const actions = actionTools(toolsCalled);

  switch (expect.kind) {
    case "noTools": {
      if (actions.length > 0) {
        return {
          passed: false,
          failure: `Expected no graph-action tools, but got: ${actions.join(",")}`,
        };
      }
      return { passed: true };
    }

    case "toolOnce": {
      const count = actions.filter((t) => t === expect.tool).length;
      if (count !== 1) {
        return {
          passed: false,
          failure: `Expected '${expect.tool}' exactly once, got ${count} (tools: ${actions.join(",")})`,
        };
      }
      // Scenarios 8a/8b target create_schema through this branch, so the
      // count-only hole this closes for noExtraTypes exists here too.
      const schemaVerdict = schemaCallsAreSound(toolCalls);
      if (!schemaVerdict.passed) return schemaVerdict;
      // `minProperties` first when a scenario opted into it: it inspects the
      // same `isError` and returns a strictly more specific diagnosis (which
      // value failed to reach storage, not merely that the call failed).
      // `targetToolWasNotRejected` is the fallback for every scenario that did
      // NOT opt in, which is where the blind spot actually was.
      if (expect.minProperties !== undefined) {
        return callPersistedProperties(
          toolCalls,
          expect.tool,
          expect.minProperties,
        );
      }
      return targetToolWasNotRejected(toolCalls, expect.tool);
    }

    case "toolSequence": {
      let idx = 0;
      for (const t of actions) {
        if (t === expect.tools[idx]) idx++;
        if (idx === expect.tools.length) break;
      }
      if (idx !== expect.tools.length) {
        return {
          passed: false,
          failure: `Expected sequence [${expect.tools.join(",")}] as a subsequence, got: ${actions.join(",")}`,
        };
      }
      const seqTarget =
        expect.propertiesOn ?? expect.tools[expect.tools.length - 1];
      if (expect.minProperties !== undefined) {
        return callPersistedProperties(
          toolCalls,
          seqTarget,
          expect.minProperties,
        );
      }
      return targetToolWasNotRejected(toolCalls, seqTarget);
    }

    case "noRetry": {
      let runLength = 0;
      for (const t of actions) {
        runLength = t === expect.tool ? runLength + 1 : 0;
        if (runLength > 1) {
          return {
            passed: false,
            failure: `Expected no repeated '${expect.tool}' calls (retry loop), got: ${actions.join(",")}`,
          };
        }
      }
      // Checked after the retry loop, not before: a scenario that opts in wants
      // both "fired at least minCalls times" and "never twice in a row", and the
      // retry failure is the more specific diagnosis of the two.
      if (expect.minCalls !== undefined) {
        const count = actions.filter((t) => t === expect.tool).length;
        if (count < expect.minCalls) {
          return {
            passed: false,
            failure: `Expected at least ${expect.minCalls} '${expect.tool}' call(s), got ${count} (tools: ${actions.join(",")})`,
          };
        }
      }
      return targetToolWasNotRejected(toolCalls, expect.tool);
    }

    case "noExtraTypes": {
      const count = actions.filter((t) => t === "create_schema").length;
      if (count !== 1) {
        return {
          passed: false,
          failure: `Expected exactly one create_schema (no extra related types), got ${count} (tools: ${actions.join(",")})`,
        };
      }
      return schemaCallsAreSound(toolCalls);
    }
  }
}


// ---------------------------------------------------------------------------
// Out-of-band seeding (scenario 13)
// ---------------------------------------------------------------------------

/**
 * The type scenario 13's seeded records carry, and the property its reference
 * keys off.
 *
 * Named here rather than inline so the seed, the end-state clause and the
 * winnability test cannot drift apart — the value the prompt refers to and the
 * value the seed writes have to be the same one, and nothing else enforces it.
 */
const SEEDED_TYPE = "incident_report";
const SEEDED_ONCALL_FIELD = "on_call";

/** The engineer named ONLY in seeded state — never in any prompt. */
const SEEDED_ONCALL = "rowan";

/**
 * The three seeded records. Exactly one carries `SEEDED_ONCALL`, which is what
 * makes "the one Rowan was on call for" resolve to a single node.
 *
 * The titles are deliberately unrelated to the on-call name: a model cannot
 * guess the target from the reference's wording, only by looking the property
 * up. That is the whole point of the scenario.
 */
const SEEDED_INCIDENTS: Array<{ title: string; onCall: string }> = [
  { title: "checkout latency spike", onCall: "dana" },
  { title: "search index corruption", onCall: SEEDED_ONCALL },
  { title: "auth token expiry storm", onCall: "sam" },
];

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
 * Seed scenario 13's incident records, outside any scored turn.
 *
 * WHY OUT OF BAND, in one sentence: a referent the AGENT wrote is replayed into
 * later turns as a terse fact carrying its property values and id inline, so it
 * is never actually indirect — which is the defect #2242 found in scenario 6
 * and #2250 hit again in a subtler form. Nothing here goes through a turn, so
 * `completed_writes` records none of it and the rendered prompt contains no
 * trace of these nodes at all.
 *
 * IDEMPOTENT AND RESETTING, both because of `--runs`. Repetition shares one
 * database across reps and calls `seedGroup` on every rep, so this has to be
 * safe to run repeatedly — and "safe" here means two different things:
 *
 *   - Do not DUPLICATE. A create-unconditionally seed would give rep 2 two
 *     `rowan` incidents and rep 3 three, at which point the on-call filter no
 *     longer identifies a single node.
 *   - Do RESET. 13's scored turn sets `resolved: true`, and `updatedNode`
 *     scores off the diff between the pre- and post-turn snapshots — so an
 *     already-resolved node makes rep 2's correct write a no-op that produces
 *     no diff at all.
 *
 * Both failure modes surface as a scenario that passes on rep 1 and fails
 * afterwards, which reads as model non-determinism and corrupts precisely the
 * pass^k measurement `--runs` exists to produce. Hence: query by type, create
 * what is missing, and reset what is already there.
 */
function seedIncidents(env: EvalEnv): void {
  const existing = runNs(env, ["schema", "list"]) as
    | { nodes?: Array<{ id?: string }> }
    | Array<{ id?: string }>
    | null;
  const ids = (Array.isArray(existing) ? existing : (existing?.nodes ?? [])).map(
    (s) => s?.id,
  );

  if (!ids.includes(SEEDED_TYPE)) {
    runNs(env, [
      "schema",
      "create",
      "--params",
      JSON.stringify({
        name: SEEDED_TYPE,
        description: "A production incident and who was on call for it",
        fields: [
          { name: SEEDED_ONCALL_FIELD, type: "text" },
          { name: "resolved", type: "boolean" },
        ],
      }),
    ]);
  }

  // Idempotent per title, and this is load-bearing rather than defensive.
  // `--runs` (repetition for pass^k) shares ONE database across reps and calls
  // `seedGroup` on every rep, so a create-unconditionally seed would leave rep
  // 2 with two `rowan` incidents and rep 3 with three. The on-call filter would
  // stop identifying a single node, 13 would start failing, and the failure
  // would read as model non-determinism — corrupting exactly the measurement
  // `--runs` exists to produce.
  //
  // Querying by type rather than tracking ids in module state keeps this
  // correct across separate runner invocations too, which module state would
  // not survive.
  const existingIncidents = runNs(env, [
    "node",
    "query",
    "--type",
    SEEDED_TYPE,
    "--limit",
    "50",
  ]) as
    | { nodes?: Array<{ content?: string; id?: string }> }
    | Array<{ content?: string; id?: string }>
    | null;
  const present = new Map(
    (Array.isArray(existingIncidents)
      ? existingIncidents
      : (existingIncidents?.nodes ?? [])
    ).map((n) => [(n?.content ?? "").toLowerCase(), n?.id ?? ""]),
  );

  for (const { title, onCall } of SEEDED_INCIDENTS) {
    const existing = present.get(title.toLowerCase());
    if (existing) {
      // Already seeded by an earlier rep. RESET it rather than skipping: 13's
      // scored turn sets `resolved: true`, and `updatedNode` scores off the
      // DIFF between the pre- and post-turn snapshots. Left as rep 1 finished
      // it, rep 2's correct write would be a no-op, produce no `changedNodes`
      // entry, and score red — turning a passing scenario into a run of
      // failures that read as model non-determinism.
      runNs(env, [
        "node",
        "update",
        existing.replace(/^nodespace:\/\//, ""),
        "--property",
        `${SEEDED_ONCALL_FIELD}=${onCall}`,
        "--property",
        "resolved=false",
      ]);
      continue;
    }
    const created = runNs(env, [
      "node",
      "create",
      "--type",
      SEEDED_TYPE,
      "--content",
      title,
    ]) as { id?: string } | null;
    const id = created?.id;
    if (!id) throw new Error(`seeding '${title}' returned no id`);
    runNs(env, [
      "node",
      "update",
      id.replace(/^nodespace:\/\//, ""),
      "--property",
      `${SEEDED_ONCALL_FIELD}=${onCall}`,
      "--property",
      "resolved=false",
    ]);
  }
}

// ---------------------------------------------------------------------------
// Scenarios
//
// Each group shares a chat node so later scenarios see earlier turns. Ids are
// the baseline join key and must stay stable; prompts may be reworded freely.
// ---------------------------------------------------------------------------

export interface MatrixScenario extends Scenario {
  /**
   * What the graph must look like when the turn ends. THIS IS THE SCORE.
   */
  end: EndState;
  /**
   * The trajectory expectation, retained as a DIAGNOSTIC rather than a score.
   *
   * Kept because it answers a question the outcome score cannot — how the
   * model got there — which is what a debugging session actually needs, and
   * because a scenario where the two disagree is the signal worth reading
   * after a run. It no longer decides pass/fail; see the module docstring.
   */
  expect: Expectation;
}

const GROUPS: MatrixScenario[][] = [
  [
    {
      id: "1",
      scenario: "1. Greeting",
      prompt: "Hi there",
      expect: { kind: "noTools" },
      // Nothing to assert on the graph beyond "it did not change": a greeting
      // that silently created a node is the failure worth catching here.
      end: { expectNoWrites: true },
    },
  ],
  [
    {
      id: "2",
      scenario: "2. Capability",
      prompt: "What can you do?",
      expect: { kind: "noTools" },
      end: { expectNoWrites: true },
    },
  ],
  // Single-custom-type CRUD chain (scenarios 3-7, then 9) shares one chat node.
  // Scenario 9 is deliberately last: it needs the spec that scenario 4 creates,
  // and referring to it by name keeps its own resolution a direct string match
  // rather than the indirect reference scenario 6 exercises.
  [
    {
      id: "3",
      scenario: "3. Schema creation",
      // Every field a later scenario keys on must be implied here, or that
      // scenario is unwinnable by construction and scores a correct refusal as
      // a failure. Two are load-bearing downstream:
      //   - drafted vs signed off → the state scenario 6 sets.
      //   - the day count         → the value scenario 4 supplies, and the
      //     discriminator scenarios 6 ("the five-day one") and 7 ("longer than
      //     forty days") both resolve against.
      // The day-count clause is still required, but no longer for the reason
      // it was written. It was there because scenario 6 tested resolve_query on
      // an *indirect* reference, and re-keying that scenario to the spec's own
      // name would have made the referent a direct string match. The #2242
      // audit found the referent was ALREADY a direct match — the rendered
      // history replays scenario 4's write with its property values and id
      // inline — so 6 now asserts the outcome rather than the route. The day
      // count remains load-bearing as scenario 7's filter value and as the
      // phrase scenario 6's prompt names its target by, which is why the clause
      // stays.
      prompt:
        "I want somewhere to keep the feature write-ups my team drafts, whether each has been signed off, and how many days we think it takes",
      expect: { kind: "noExtraTypes" },
      // The type must EXIST when the turn ends, and be the only one created.
      // This replaces the old exactly-one-CALL rule and is strictly better
      // aligned with what the user experiences: a model that called
      // create_schema twice for the SAME type ends with one type and now
      // passes, while one that proactively invented a related type leaves it
      // behind and fails — the outcome that actually costs real cleanup.
      //
      // A COUNT rather than a name, deliberately: the model chooses the type's
      // identifier, so asserting one would red-line a model that named the
      // same concept differently, measuring this fixture's vocabulary guess
      // instead of the model's behavior (the #1846 trap, one level up).
      end: { createdSchemas: 1 },
    },
    {
      id: "4",
      scenario: "4. Instance creation",
      // `minProperties` is what makes scenarios 6 and 7 winnable *in principle*:
      // both discriminate on the day count this turn is supposed to store.
      // Without it, create_node persisting a bare shell scores green here and
      // the failure surfaces two scenarios later as an unresolvable reference —
      // indistinguishable from the model declining a genuinely ambiguous one.
      // 1, not 2, so this asserts "the particulars reached storage" rather than
      // pinning which of the state or the estimate the model chose to record.
      prompt: "Put one down for offline sync, still a draft, we reckon five days",
      expect: { kind: "toolOnce", tool: "create_node", minProperties: 1 },
      // minProperties: 1 rather than a named field — scenario 6 and 7
      // discriminate on the day count, but which of the state or the estimate
      // the model chose to record is not what this turn is asserting.
      end: {
        createdNode: { contentMatches: "offline sync", minProperties: 1 },
        noUnexpectedNodes: true,
      },
    },
    {
      id: "5",
      scenario: "5. List/query",
      prompt: "What write-ups are on the books?",
      expect: { kind: "toolOnce", tool: "search_nodes" },
      // A query must not write. This is where severity now falls out for free:
      // two search_nodes calls persist nothing and pass, while a model that
      // answers by creating a node to hold the answer fails.
      end: { expectNoWrites: true },
    },
    {
      id: "6",
      scenario: "6. Update",
      // resolve_query performs the search internally and returns the resolved
      // node directly (see ADR-064 rule 4) — the model acts on it via
      // update_node without a separate search_nodes call of its own.
      prompt: "The five-day one got signed off — mark it that way",
      // ASSERTS THE OUTCOME, NOT THE ROUTE — changed by the #2242 audit, which
      // found this scenario failing for EVERY model measured including one
      // passing 17/20 overall. It previously required the
      // `[resolve_query, update_node]` subsequence.
      //
      // The scenario's design intent was that "the five-day one" is an
      // INDIRECT reference only `resolve_query` can resolve — deliberately
      // chosen over the write-up's own name so a plain lookup could not
      // shortcut it. That intent does not survive contact with the rendered
      // history. Scenario 4's create_node is replayed into this turn as a
      // terse fact carrying its property values AND its id inline —
      // "properties estimated_days 5, signed_off false (id nodespace://fw1)" —
      // so the discriminator and the target id are both sitting in the prompt
      // as plain text. See
      // `scenario_6_history_resolves_its_indirect_reference_directly` in
      // daemon/src/services/local_agent_service.rs, which renders the real
      // history and pins both.
      //
      // So a model that goes straight to update_node with the right id has not
      // skipped a step — there is no indirection left for resolve_query to
      // resolve. Requiring the call anyway scored a correct end state red for
      // taking a shorter route that the fixture's own setup made available,
      // which measures the fixture rather than the model.
      //
      // What is still worth asserting is everything the outcome depends on,
      // and `toolOnce` + `minProperties: 1` keeps all of it: the right node was
      // updated exactly once, the write was not rejected, and the sign-off
      // value actually reached storage rather than an update carrying only an
      // id. `scenario_6_ideal_update_is_accepted_and_persists_the_state_change`
      // in packages/agent/tests/it/matrix_scenario_winnability.rs verifies against
      // a live backend that this ideal call is accepted and reports a persisted
      // property, so the assertion is satisfiable.
      //
      // The cost, stated rather than hidden: this no longer measures whether
      // the model can decompose an indirect reference. That behavior needs a
      // referent not recoverable from history, which this chain cannot provide
      // — every write it makes is replayed with its particulars. Scenario 13
      // covers it instead, by seeding its referent outside any scored turn so
      // nothing about it reaches the prompt.
      //
      // Scenario 12 is the nearest cover and is deliberately NOT claimed as a
      // replacement: it requires ranking three estimates rather than matching
      // one, which is strictly harder than 6, but its values are inline in
      // history too, so the ranking can be done in-context. Read that group's
      // header before treating a red there as a decomposition finding.
      //
      // 6 and 12d are the same write (a boolean sign-off) reached by different
      // resolutions, so a red on 12d beside a green on 6 isolates the
      // resolution rather than the update.
      expect: {
        kind: "toolOnce",
        tool: "update_node",
        minProperties: 1,
      },
      // Path-agnostic by construction, which is the point: reaching this state
      // via resolve_query+update_node and reaching it via update_node alone are
      // the same outcome for the user, and the shorter route no longer costs a
      // point. `updatedNode` (not `createdNode`) is what keeps the assertion
      // honest — a turn that records the sign-off on a NEW node leaves the
      // original untouched and is a real failure, named as such.
      end: {
        updatedNode: { contentMatches: "offline sync", minProperties: 1 },
        noUnexpectedNodes: true,
      },
    },
    {
      id: "7",
      scenario: "7. Empty-result query",
      prompt: "Is anything on our plate longer than forty days?",
      expect: { kind: "noRetry", tool: "search_nodes" },
      end: { expectNoWrites: true },
    },
    {
      id: "9",
      scenario: "9. Set property on existing node",
      // Now the same assertion shape as scenario 6, which the #2242 audit
      // moved off its `[resolve_query, update_node]` subsequence and onto the
      // outcome. The two are no longer distinguished by WHAT they assert, and
      // pretending otherwise would be the stale claim this comment used to
      // make (that 6 tests indirect-reference resolution — it does not, because
      // the rendered history hands the model the id).
      //
      // They remain worth keeping as separate observations. 6 updates a
      // BOOLEAN state the prompt names obliquely ("mark it that way"); this
      // one overwrites an existing NUMBER with a correction the prompt states
      // outright. Different value types and different phrasings of the same
      // write, scored independently — but a red here and a red on 6 now mean
      // the same thing, and should be read as two samples of one behavior
      // rather than two behaviors.
      //
      // This is the shape that reached production returning `updated: true`
      // with `property_count: 0`: the model resolved the right node, called
      // update_node, echoed the node's existing title back as `content`, and
      // sent no properties at all. The tool reported success, and the model
      // reported the write as done with a fabricated value. minProperties is
      // what makes that outcome score red rather than green — without it, a
      // call that persists nothing is indistinguishable from one that
      // persisted the value, because the tool name is all that is checked.
      //
      // WINNABILITY (the constraint an earlier draft of this scenario broke):
      // the prompt must name a value this chain's schema can actually hold.
      // Scenario 3 builds the write-up type from a prompt mentioning only
      // sign-off and a day count, so those two fields are all that exist. An
      // earlier draft of the equipment-themed ancestor of this scenario asked
      // to set a DUE DATE — a field the schema has nowhere to put — which made
      // the scenario unwinnable: the model folded the date into the node's text
      // (a reasonable degradation, and it reported it honestly) and scored red
      // for it. A scenario that reds out correct behavior measures the fixture,
      // not the model. Same trap as the album/artist case in #1846.
      //
      // The day count is chosen over the sign-off state because scenario 6
      // already owns that transition; re-testing it here would score the same
      // model behavior twice. "eight" is unambiguous — no relative-date or unit
      // inference stands between the request and the write, so a red here means
      // the value did not reach `properties`, which is the one thing this
      // scenario is for.
      prompt: "Correction: offline sync is eight days, not five",
      expect: {
        kind: "toolOnce",
        tool: "update_node",
        minProperties: 1,
      },
      // The VALUE the prompt supplies must reach storage — that is the whole
      // scenario, so the clause pins the value and not merely "something
      // changed". This is the shape that reached production reporting
      // `updated: true` with `property_count: 0`: the model resolved the right
      // node, echoed its title back as content, and sent no properties at all.
      // A clause asserting only that the node changed would score that green
      // the moment the model rewrote the content instead.
      //
      // `8` is matched across the string/number boundary and the field is NOT
      // named, for the same winnability reason scenario 3's type is not named:
      // scenario 3 lets the model choose the key it stores the day count
      // under, so pinning one would measure the fixture's guess. What is
      // asserted is that eight reached SOME property of the right node.
      end: {
        updatedNode: { contentMatches: "offline sync", hasPropertyValue: 8 },
        noUnexpectedNodes: true,
      },
    },
  ],
  // Multi-custom-type CRUD (scenario 8) shares its own chat node.
  [
    {
      id: "8a",
      scenario: "8a. Create type: first",
      // Names the field 8c keys on, for the same reason scenario 3 names its
      // downstream fields. 8c asserts `minProperties: 1` on whose call it was,
      // so leaving this type's fields entirely to chance made 8c winnable only
      // if the model happened to invent that field — and a type with nowhere
      // to put the value means the model folds it into the node's text,
      // degrades honestly, and scores red for the fixture's omission rather
      // than its own behavior. The sibling chain already got this
      // right; this one inherited the gap from the scenario it replaced.
      prompt:
        "Start keeping the calls we make on how the system is built, and who made each one",
      expect: { kind: "toolOnce", tool: "create_schema" },
      end: { createdSchemas: 1 },
    },
    {
      id: "8b",
      scenario: "8b. Create type: second",
      prompt: "I also need somewhere for the two-week cycles we plan",
      expect: { kind: "toolOnce", tool: "create_schema" },
      end: { createdSchemas: 1 },
    },
    {
      id: "8c",
      scenario: "8c. Instance: first type",
      // minProperties: 1 requires the particular this prompt supplies — who
      // made the call — to actually reach storage. Without it, create_node
      // persisting a bare shell (no such property — unwinnable if the type's
      // own schema has no field for it, see #1846) scores identically to one
      // that recorded it.
      prompt: "Put down that we went with event-based cache clearing, Priya's call",
      expect: { kind: "toolOnce", tool: "create_node", minProperties: 1 },
      end: {
        createdNode: { contentMatches: "cache", minProperties: 1 },
        noUnexpectedNodes: true,
      },
    },
    {
      id: "8d",
      scenario: "8d. Instance: second type",
      prompt: "New cycle: Harbour, it wraps up on the 30th",
      expect: { kind: "toolOnce", tool: "create_node" },
      end: {
        // The prompt gives an END date and no start date, and the field list
        // comes from the type the model itself defined in 8b — so a required
        // start date cannot be satisfied from the prompt. Asking is then the
        // correct move, and was the one scoring zero: measured here, Laguna
        // asked and FAILED while E4B guessed, had the write REJECTED, and
        // passed on tool name alone.
        clarifyOk: true,
        createdNode: { contentMatches: "harbour" },
        noUnexpectedNodes: true,
      },
    },
    {
      id: "8e",
      scenario: "8e. Query across types",
      // WINNABILITY — reworded by the #2242 audit, which found this failing for
      // every model measured. The previous prompt was "Run through those calls
      // for me", and the referent was the problem.
      //
      // "Those calls" points back THREE turns, past 8d, which created a
      // Planning Cycle named Harbour. The rendered history the turn receives
      // ends on that cycle — see the terse-fact replay in
      // `node_history_from_messages` — so the nearest antecedent to a bare
      // demonstrative is the wrong type. That is not a hypothetical reading:
      // one model's reply on this turn described creating a Planning Cycle,
      // i.e. it answered 8d's prompt rather than this one.
      //
      // Two further problems compounded it. "Calls" is a pun the chain sets up
      // deliberately — 8a introduces the decision type as "the calls we make on
      // how the system is built" — but it is also the ordinary word for
      // telephone calls and for tool invocations, and it never appears as the
      // stored type's name. And "run through" reads as much like "walk me
      // through what you did" (a meta question, which `TOOL_STRATEGY_RULES`
      // explicitly says to answer with NO tools) as like "list them".
      //
      // The rewording fixes all three: it names the type by the words 8a used
      // to define it rather than by a demonstrative, and it asks with an
      // unambiguous retrieval verb. A type-filtered read is verified against a
      // live backend by
      // `scenario_8e_ideal_cross_type_read_is_accepted_and_discriminates` in
      // packages/agent/tests/it/matrix_scenario_winnability.rs — it returns the
      // decision and NOT the planning cycle, so the read this asks for is both
      // legal and correctly discriminating.
      //
      // The scenario title still says "Query across types" and that is still
      // what it measures: two custom types exist, and a correct answer reads
      // only one of them. Asking about both at once would make `toolOnce`
      // wrong, since two type-filtered reads is a legitimate shape for that.
      prompt: "List every decision we've recorded about how the system is built",
      expect: { kind: "toolOnce", tool: "search_nodes" },
      end: { expectNoWrites: true },
    },
  ],
  // Core-type schema fields (scenario 10) shares its own chat node.
  //
  // Every group above builds its own CUSTOM type first, which is precisely why
  // this gap went unmeasured: a custom type's fields reach the model through
  // the RELEVANT ENTITY TYPES block, and that block excludes core types by
  // construction. So `task`'s own defined fields — due_date, priority,
  // assignee — were invisible from every direction, and the matrix could not
  // see it because no scenario ever acted on a core type.
  //
  // These use `task` deliberately and create it with `properties` unset beyond
  // the minimum, so the fields under test are defined-but-unset — the exact
  // state where "field exists" and "field does not exist" were
  // indistinguishable.
  [
    {
      id: "10a",
      scenario: "10a. Core-type instance creation",
      // Winnability: due_date, priority and assignee are all defined on the
      // seeded core task schema, so 10b and 10c are answerable in principle.
      // This turn deliberately supplies NONE of them — the following scenarios
      // are about writing and filtering a field that has no value yet, which is
      // only a real test if it starts unset.
      prompt: "Add a task to swap the image resizer over to the new pipeline",
      expect: { kind: "toolOnce", tool: "create_node" },
      end: {
        createdNode: { type: "task", contentMatches: "resizer" },
        noUnexpectedNodes: true,
      },
    },
    {
      id: "10b",
      scenario: "10b. Set a defined-but-unset core field",
      // The reported failure: the model asked the user "what field name is
      // used on this task node that tracks dates?" for `due_date` — a field
      // defined on the core task schema all along. It was not being obtuse;
      // get_node returned only populated properties, so the field genuinely
      // was not visible to it, and the "use the node's own existing property
      // keys" rule then made declining the correct move.
      //
      // The prompt says "due date" in the user's words and never names the
      // key, so a pass requires the field list to have reached the model
      // rather than the key having been handed over in the prompt.
      //
      // minProperties: 1 is what makes this scenario mean anything: the whole
      // defect is a turn that ends without the value reaching `properties`.
      // Without it, a content-only update_node — or the model narrating the
      // change it did not make — scores identically to a real write.
      prompt: "Set that task's due date to 6 August 2026",
      expect: { kind: "toolOnce", tool: "update_node", minProperties: 1 },
      // `due_date` by name: the whole defect this scenario exists for is the
      // model failing to see a field defined on the core task schema, and the
      // prompt deliberately says "due date" in the user's words without ever
      // naming the key. Asserting the key is what proves the field list
      // reached the model rather than the key having been handed to it.
      end: {
        updatedNode: { type: "task", properties: { due_date: true } },
        noUnexpectedNodes: true,
      },
    },
    {
      id: "10c",
      scenario: "10c. Filter core type by enum field",
      // The read-side half of the same root cause. Observed: the model asked
      // the user to confirm that `status` was the field and `open` a legal
      // value — both defined on the core task schema (status is required, with
      // core values open / in_progress / done / cancelled).
      //
      // `noRetry` rather than `toolOnce`: an empty or narrowing result may
      // legitimately prompt one follow-up search, so a hard count of 1 would
      // red out correct behavior. What it must not do is loop blindly.
      //
      // `minCalls: 1` covers the other half, and is the half this scenario
      // exists for: the reported failure is the model stopping to interrogate
      // the user rather than searching, which shows up as the search never
      // firing. Bare `noRetry` scores that outcome GREEN — its repeat-detecting
      // loop never executes over zero calls — so without `minCalls` this
      // scenario would pass on the exact production behavior it was added to
      // catch, which is worse than not measuring it at all.
      prompt: "How many tasks are still open?",
      expect: { kind: "noRetry", tool: "search_nodes", minCalls: 1 },
      end: { expectNoWrites: true },
    },
  ],
  // Relationship traversal (scenario 11) shares its own chat node.
  //
  // The matrix's one structural blind spot until now: every group above acts on
  // a node's OWN fields, so nothing measured whether the model can record a
  // link between two nodes or follow one back. That is the half of the data
  // model the product's own framing rests on — a decision means nothing without
  // the work it constrains — and `create_relationship`/`get_related_nodes` were
  // never once exercised end-to-end despite both being registered tools.
  //
  // Kept as its own group rather than appended to the chain above because it
  // needs TWO nodes of DIFFERENT types to exist before the link is askable, and
  // building those inside another group would silently re-score that group's
  // create_node behavior a third time.
  [
    {
      id: "11a",
      scenario: "11a. Link setup: first node",
      // Not scored for relationship behavior — it exists so 11c has two real,
      // differently-typed endpoints to connect. `text` is the fallback the
      // model reaches for when no custom type fits, and that is fine here:
      // what 11c asserts is the LINK, and create_relationship takes two ids
      // regardless of what types they carry.
      //
      // AMBIGUOUS-VERB TRAP, caught on this group's first live run. An earlier
      // draft opened with "Note that we settled on ..." — which reads as a
      // preamble to a statement rather than a request to record one. The model
      // called search_semantic looking for an existing note and answered
      // "Found 0 nodes matching your request", scoring red for a reasonable
      // reading of the words it was given.
      //
      // That failure then cascaded: with no node created here, 11c's
      // create_relationship went out with `to_id: null` and was rejected, and
      // 11d had no edge to traverse. All three of this group's failures traced
      // back to this one word choice, which is why the opening verb has to be
      // an unambiguous record-creation request — the same correction scenario 4
      // already needed.
      //
      // The cascade is now bounded structurally as well as by wording: this
      // turn is `setup`, so a repeat of that failure costs the successors it
      // genuinely invalidates rather than also scoring itself as a third
      // independent observation. Careful wording is still required — a setup
      // turn that establishes nothing makes 11c and 11d unwinnable, which is
      // why its verdict is asserted and warned about rather than ignored.
      prompt: "Log a decision: the reports page uses server-side rendering",
      expect: { kind: "toolOnce", tool: "create_node" },
      // SETUP, not scored: this exists so 11c has two real endpoints to
      // connect. Scoring it made one ambiguous verb here cost three points —
      // this turn, plus 11c and 11d, which it left with nothing to link or
      // traverse. Its verdict is still recorded and warned about, because a
      // setup turn that quietly does nothing makes its successors unwinnable.
      setup: true,
      end: { createdNode: { contentMatches: "reports page" } },
    },
    {
      id: "11b",
      scenario: "11b. Link setup: second node",
      prompt: "Add a task to rebuild the reports page",
      expect: { kind: "toolOnce", tool: "create_node" },
      setup: true,
      end: { createdNode: { type: "task", contentMatches: "reports page" } },
    },
    {
      id: "11c",
      scenario: "11c. Record a link between two nodes",
      // The failure this is built to catch is NOT "no tool fired" — it is the
      // model expressing the link by writing prose into one of the two nodes,
      // which reports success and records nothing traversable. Asserting
      // create_relationship by name is what separates those two outcomes.
      //
      // `toolOnce` rather than `toolSequence`, deliberately, even though the
      // model must recover both endpoint ids before it can link them: the
      // lookup has three legitimate spellings (search_nodes, search_semantic,
      // get_node) and pinning any one of them into a sequence would red out
      // the other two. The retrieval is not left unmodelled by that choice —
      // 11a and 11b created both endpoints in this same chat, so the ids are
      // recoverable — it is simply not the thing being scored here. What is
      // being scored is that the link was recorded as an EDGE and exactly once.
      //
      // The known cost, stated rather than hidden: this cannot distinguish a
      // create_relationship carrying two real ids from one carrying two
      // invented ones. Catching that needs the tool result, not the tool name,
      // and `ToolCallRecord` does not carry the endpoint ids today. A run's
      // trace file does, so a suspicious pass is checkable by hand.
      //
      // No minProperties: create_relationship's payload is two ids and a
      // relation name, none of which are schema field values, so `fieldCount`
      // does not apply to it — asserting it would fail on a correct call. The
      // `minProperties is never asserted on create_relationship` invariant in
      // the test file pins that.
      // WINNABILITY — the constraint this scenario's first live run exposed,
      // and it is a hard one. `create_relationship` does NOT accept an
      // arbitrary relation name: the validator requires the type to be defined
      // on the SOURCE node's schema, plus four universal built-ins
      // (member_of, has_child, mentions, has_role). See
      // node_service/relationship.rs's "are universal" error.
      //
      // The first draft asked to "connect the two", and the model answered
      // with `related_to` — two real ids, correct direction, everything about
      // the turn right — and the write was REJECTED, because neither `task`
      // nor `text` defines `related_to`. Worse, the tool's own description
      // named `related_to` as a valid generic fallback at the time, so the
      // model was rejected for following its instructions exactly. (That
      // description has since been corrected to name `mentions`, and a
      // regression guard now keys the wording off the validator's own
      // built-in list.) That combination made
      // the scenario unwinnable by construction: no wording of "link these"
      // could have succeeded for an ad-hoc relation between these two types.
      //
      // The prompt therefore asks for a link the system can actually record.
      // "Point the task at it" maps onto `mentions`, which is universal and
      // legal between any two nodes, so a correct model CAN pass — which is
      // the minimum bar for a scenario to measure anything.
      //
      // The underlying affordance gap (a documented generic label the
      // validator refuses) is a known product bug; it is not
      // this fixture's job to encode a workaround for it beyond staying
      // winnable. If that lands, revisit whether this prompt should go back to
      // asking for a link in the user's own words.
      prompt: "Point that rebuild task at the decision it has to respect",
      expect: {
        kind: "toolOnce",
        tool: "create_relationship",
      },
      // The edge must EXIST. This is what separates the failure worth catching
      // — the model expressing the link as prose inside one of the two nodes,
      // which reports success and records nothing traversable — from a real
      // link, and it does so without pinning how the endpoints were recovered.
      end: { createdEdge: { relation: "mentions" } },
    },
    {
      id: "11c2",
      scenario: "11c2. Record a second link to the same node",
      // Exists so 11d's question has a real answer to aggregate rather than a
      // single edge the history already spells out in one line. See 11d's
      // WINNABILITY note: with one edge, "which records point at that
      // decision" is answerable by reading the one create_relationship fact;
      // with two edges recorded a turn apart, nothing in history states the
      // set, and assembling it is the traversal's job.
      //
      // Scored the same way as 11c, deliberately. It is a second observation of
      // the same linking behavior on a DIFFERENT source type (a text note
      // rather than a task), which is worth having on its own — 11c's `mentions`
      // pass could otherwise be a property of `task` specifically — and it costs
      // nothing to assert given the turn has to happen for 11d's sake.
      //
      // Creates its own source node in the same turn, so this is one of the few
      // scenarios where a second action tool is expected. `toolOnce` tolerates
      // that: it counts only calls to the named tool and ignores the rest.
      prompt:
        "Jot down that the caching layer also depends on it, and point that at the same decision",
      expect: {
        kind: "toolOnce",
        tool: "create_relationship",
      },
      // Both halves asserted, because this turn does both: the note has to
      // exist AND the edge has to reach it. Asserting only the edge would pass
      // on a turn that linked the wrong node; asserting only the node would
      // pass on a turn that wrote a note and never linked it — which is the
      // prose-instead-of-an-edge failure 11c exists to catch, one scenario on.
      //
      // This is the create-and-link-in-one-turn shape the runner's snapshot
      // bracketing is built to see: the source node does not exist in the
      // `before` snapshot, so its edges are only discoverable because the
      // `after` walk admits nodes new since `before`. A "walk only the
      // pre-turn set" rule would score this red no matter what the model did.
      //
      // No `noUnexpectedNodes`: the turn legitimately creates a node AND the
      // model may reasonably reach for a type this fixture has not pinned, so
      // the clause would be asserting the fixture's guess rather than the
      // model's behavior.
      end: {
        createdNode: { contentMatches: "caching" },
        createdEdge: { relation: "mentions" },
      },
    },
    {
      id: "11d",
      scenario: "11d. Traverse a link back",
      // The read half — the query the product's framing is built on, asked
      // from the decision's side: which pieces of work are bound by it. It is
      // answered by following the edges 11c and 11c2 recorded, not by matching
      // text.
      //
      // `noRetry` with `minCalls: 1`. The `minCalls` half is what this
      // scenario is for: the failure worth catching is the model answering
      // from the conversation it can still see in its own history instead of
      // reading the graph, which shows up precisely as the traversal never
      // firing. Bare `noRetry` scores that GREEN, since its repeat-detecting
      // loop never executes over zero calls.
      //
      // The `noRetry` half is the weaker of the two and is NOT justified by
      // the same reasoning as 10c, despite the surface similarity. There, a
      // second `search_nodes` is a genuine blind retry of the same lookup.
      // Here, a second `get_related_nodes` could be a legitimate walk of the
      // OTHER endpoint of a bidirectional edge — correct exploration, scored
      // red. This prompt asks about ONE node's inbound edges ("which records
      // point at that decision"), which `get_related_nodes` answers in a
      // single call regardless of how many edges come back — the two links
      // 11c and 11c2 recorded share a target, so enumerating them is one
      // traversal, not two. One call therefore remains the expected shape, and
      // the false-positive below stays as narrow as it was.
      //
      // If a run reds out here with exactly two `get_related_nodes` calls,
      // check the trace for the two-endpoint shape BEFORE recording it as a
      // model failure — that is the known false-positive, and it is a fixture
      // defect rather than a model one.
      //
      // NOTE ON DEPENDENCE: if 11c failed to record an edge, this scenario can
      // still pass — it asserts that the traversal was ATTEMPTED, not that it
      // came back non-empty. That is deliberate. Making it conditional on 11c's
      // success would fold two independent behaviors into one score and make a
      // link-side regression read as two failures instead of one.
      //
      // WINNABILITY — the constraint the #2242 audit exposed, and the reason
      // this prompt is worded the way it is rather than the way it reads most
      // naturally.
      //
      // An earlier draft asked "What did we settle on that the rebuild has to
      // respect?" and failed for EVERY model measured, including one passing
      // 17/20 overall. The audit's first hypothesis was that the prompt
      // template drops `role="tool"` history, making the edge invisible and the
      // scenario unwinnable. That is REFUTED — see
      // `scenario_11d_history_already_contains_the_link_it_asks_about` in
      // daemon/src/services/local_agent_service.rs, which renders the real
      // history for this chain. Tool-role messages are dropped, but the writes
      // they carried are re-rendered as terse "Fact: ..." lines plus a
      // system-role write record, so the turn could see BOTH endpoint ids, the
      // `mentions` edge, and the decision's own text.
      //
      // The failure was the opposite of a missing fact: the history HANDED the
      // model the answer. "What did we settle on" was answerable by reading the
      // decision's text three messages up, and `TOOL_STRATEGY_RULES`'s first
      // bullet tells the model to answer such a turn directly in text. Every
      // model returning `tools: []` was following its instructions against a
      // prompt that did not need the graph.
      //
      // A scenario measuring traversal must therefore ask for something the
      // conversation CANNOT answer. Within this chain that is a hard
      // constraint: the terse fact for each created node states its title, its
      // type AND its id, so every read-only question about either endpoint is
      // answerable from history. Re-wording alone cannot fix that — which is
      // why this scenario now asks for the CURRENT set of links on the
      // decision, and 11c2 records a second edge for the set to be non-trivial.
      //
      // A set is the one thing the history genuinely does not state. It holds
      // two separate "Fact: create_relationship completed" lines, recorded a
      // turn apart; nothing anywhere says how many edges the decision now
      // carries, and assembling that from two scattered lines is exactly the
      // aggregation a traversal exists to do. A model that answers from
      // history can still get it right by counting — so this is a weaker
      // guarantee than "unanswerable", and it is stated rather than hidden —
      // but it is the strongest available without adding a turn whose only
      // purpose is to defeat the history renderer.
      //
      // The ideal call is verified end to end against a live backend by
      // `scenario_11d_ideal_traversal_is_accepted_and_finds_the_link` in
      // packages/agent/tests/it/matrix_scenario_winnability.rs — the traversal is
      // accepted and returns the linked records, so a correct model CAN pass,
      // which is the minimum bar for the scenario to measure anything.
      prompt: "Which records point at that rendering decision right now?",
      expect: { kind: "noRetry", tool: "get_related_nodes", minCalls: 1 },
      // A read: the traversal must not write. Whether it came back non-empty
      // depends on 11c having recorded an edge, which is deliberately NOT
      // folded in — that would make one link-side regression read as two
      // failures.
      end: { expectNoWrites: true },
    },
  ],
  // Comparative reference resolution (scenario 12), in its own chat.
  //
  // WHAT THIS DOES AND DOES NOT MEASURE — read this before treating a red here
  // as a decomposition finding.
  //
  // It measures whether the model resolves a reference stated as a COMPARATIVE
  // ("whichever is the biggest job") to the right node and writes to THAT node.
  // It does NOT measure multi-step decomposition, and it is not the coverage
  // scenario 6 gave up. That gap is still open; see #2248.
  //
  // The distinction is the whole reason this comment is long. An earlier draft
  // of this group claimed the comparative forced a read, on the strength of a
  // daemon test showing the ordering words are absent from history. That
  // reasoning does not hold, and the counter-example is one glance at the
  // rendered history:
  //
  //   Fact: a feature_writeup node was created with title 'checkout rewrite'
  //         and properties estimated_days 9 (id nodespace://fw10).
  //   Fact: a feature_writeup node was created with title 'search indexer'
  //         and properties estimated_days 21 (id nodespace://fw11).
  //   Fact: a feature_writeup node was created with title 'audit log export'
  //         and properties estimated_days 4 (id nodespace://fw12).
  //
  // Three adjacent lines, uniform format, every estimate AND every id present
  // as literal text. `max(9, 21, 4)` is an in-context comparison, so a model
  // can go straight to update_node with the right id and never read anything —
  // and that is a CORRECT answer, not a shortcut. Absence of the word "biggest"
  // proves the ordering is not STATED; it does not prove the ordering is not
  // DERIVABLE, and derivability is what makes a reference indirect. This is a
  // subtler repeat of the scenario 6 defect #2242 found, and it survived an
  // absence proof precisely because that proof asserted the wrong property.
  //
  // Every scalar a scored turn writes is replayed inline by
  // `terse_write_fact`, so no value this chain writes can be made underivable
  // by wording alone. Closing #2248 properly needs a referent the agent never
  // wrote — which needs out-of-band seeding the runner deliberately does not
  // support ("Assert, do not own", runner.ts; "never starts or seeds
  // anything", env.ts).
  //
  // So what is this group worth keeping for? It is still strictly harder than
  // scenario 6: 6's referent was a direct string match on a value the prompt
  // itself named ("the five-day one" against `estimated_days 5`), whereas this
  // one requires ranking three values before any id can be chosen. That is a
  // real capability and nothing else in the matrix covers it.
  //
  // `toolSequence` deliberately does NOT live here, though a draft of 12d used
  // it. Naming `[search_nodes, update_node]` pins one of four legitimate read
  // spellings AND reds out the shortest correct route (update_node alone, which
  // the inline estimates make valid) — both the mistake #2242 was cleaning up.
  // It lives on scenario 13 instead, whose route is genuinely forced because
  // its referent is seeded out of band and therefore absent from history.
  //
  // WINNABILITY, proved rather than assumed, per the #2242 discipline:
  //   - `scenario_12_history_states_the_values_but_not_the_ordering`
  //     (daemon/src/services/local_agent_service.rs) renders the REAL history
  //     and pins what is and is not in it. Read its docstring for the scope of
  //     what it actually establishes — deliberately narrower than this group's
  //     first draft assumed.
  //   - `scenario_12_ideal_comparative_chain_is_accepted_and_the_read_carries_the_values`
  //     (agent/tests/it/matrix_scenario_winnability.rs) proves against a live
  //     backend that a model which DOES read gets all three instances with
  //     their estimates, and that the write on the winner persists.
  //
  // OWN GROUP, not appended to the 3-7/9 chain. Three setup instances would
  // re-score that chain's create_node behavior three more times and stretch its
  // history, and the 11-group set the precedent for isolating a scenario whose
  // setup is substantial. 12a-12c are `setup: true` for that group's reason
  // too: they establish state, and a phantom failure in one of them must not
  // count three times in the denominator.
  [
    {
      id: "12a",
      scenario: "12a. Comparative setup: type",
      // Names BOTH fields 12d needs: the estimate its comparison ranges over,
      // and the sign-off it writes. Same reason scenario 3 names its downstream
      // fields — a type with nowhere to put a value makes every later turn in
      // this group unwinnable, and the model degrades honestly and scores red
      // for the fixture's omission.
      //
      // The sign-off half was missing until a DeepSeek run made the cost
      // visible: 12d failed while the model reasoned correctly, ranking the
      // three estimates, naming the right node, then declining to invent a
      // field the type did not declare ("the Build Job type only tracks
      // estimated_days — it has no signed off field"). That is the behaviour
      // this suite wants; the fixture was scoring it red.
      prompt:
        "I want to track the build jobs we take on, roughly how many days each needs, and whether each one is signed off",
      expect: { kind: "noExtraTypes" },
      end: { createdSchemas: 1 },
      setup: true,
    },
    {
      id: "12b",
      scenario: "12b. Comparative setup: first instance",
      prompt: "Log the checkout rewrite, we think nine days",
      expect: { kind: "toolOnce", tool: "create_node", minProperties: 1 },
      end: {
        createdNode: {
          contentMatches: "checkout rewrite",
          hasPropertyValue: 9,
        },
        noUnexpectedNodes: true,
      },
      setup: true,
    },
    {
      id: "12b2",
      scenario: "12b2. Comparative setup: largest instance",
      // The node 12d has to find. Created in the MIDDLE of the three, so
      // neither "the first one" nor "the last one" picks it — only ranking the
      // estimates does. A model that resolves by recency lands on 12c's node
      // and 12d's `contentMatches` catches it.
      //
      // One instance per turn, not two-in-one as an earlier draft had it. That
      // draft paired a two-node prompt with `toolOnce create_node`, which
      // requires EXACTLY one call — so the correct behavior (two calls, or one
      // create_nodes_from_markdown) scored the diagnostic red while a model
      // that created only one node scored it green. Splitting the turn is what
      // lets each one carry both a truthful diagnostic and a `noUnexpectedNodes`
      // clause, which a two-instance turn cannot state.
      prompt: "Also the search indexer, that one's twenty-one days",
      expect: { kind: "toolOnce", tool: "create_node", minProperties: 1 },
      end: {
        createdNode: {
          contentMatches: "search indexer",
          hasPropertyValue: 21,
        },
        noUnexpectedNodes: true,
      },
      setup: true,
    },
    {
      id: "12c",
      scenario: "12c. Comparative setup: third instance",
      // Smallest, and created LAST, so recency and magnitude disagree: a model
      // reaching for the freshest fact lands on the wrong node.
      prompt: "One more: the audit log export, call it four days",
      expect: { kind: "toolOnce", tool: "create_node", minProperties: 1 },
      end: {
        createdNode: {
          contentMatches: "audit log export",
          hasPropertyValue: 4,
        },
        noUnexpectedNodes: true,
      },
      setup: true,
    },
    {
      id: "12d",
      scenario: "12d. Comparative reference",
      // THE SCENARIO. The prompt names no title, no id and no estimate — only
      // the ordering, so the model must rank the three estimates before it can
      // choose an id. Read the group header for what that does and does NOT
      // establish: the estimates are all inline in history, so the ranking can
      // be done in-context and a bare `update_node` is a correct route.
      //
      // "signed off" is the same state transition scenario 6 sets, chosen
      // deliberately: 6 and 12d differ ONLY in how the target is identified
      // (direct string match vs. ranking), so a red here beside a green on 6
      // isolates the resolution rather than the write.
      prompt: "Whichever is the biggest job — that one's signed off now",
      // DIAGNOSTIC, not the score.
      //
      // `noRetry` on update_node rather than a read-then-write SUBSEQUENCE,
      // and the difference matters. An earlier draft named
      // `[search_nodes, update_node]`, which pins ONE of four legitimate read
      // spellings — `search_nodes`, `search_semantic`, `resolve_query` and
      // `get_node` are all offered — and, worse, reds out the shortest correct
      // route entirely: as the group header explains, the estimates are all
      // inline in history, so `update_node` alone is a correct answer here.
      // Scenario 11c documents the same "do not pin the spelling" reasoning.
      //
      // What is left worth asserting on the trajectory is that the write
      // happened and was not a blind retry loop. `minCalls: 1` is required —
      // without it, bare `noRetry` passes on a turn where update_node never
      // fired at all.
      expect: { kind: "noRetry", tool: "update_node", minCalls: 1 },
      // THE SCORE. `contentMatches` pinned to the 21-day instance is what makes
      // this measure the comparison rather than the write: a model that resolves
      // the reference WRONGLY still calls update_node and still persists a
      // property, so a clause that only counted properties would score picking
      // the wrong node green. Naming the node is the whole assertion.
      //
      // `updatedNode` rather than `createdNode` for scenario 6's reason: a turn
      // that records the sign-off on a NEW node leaves the original untouched
      // and is a real failure.
      //
      // `minProperties: 2` rather than `properties: { signed_off: true }`
      // because this group's type is model-built — naming the key would measure
      // the fixture's vocabulary guess rather than the model's behavior (the
      // same reason `createdSchemas` is a count).
      //
      // 2, not 1, because the node already arrives carrying its estimate from
      // 12b2: `minProperties: 1` would be satisfied by that pre-existing value
      // alone and score green on a turn that resolved the right node and then
      // persisted NOTHING — the `updated: true` with `property_count: 0` shape
      // scenario 9 documents reaching production.
      //
      // KNOWN RESIDUAL WEAKNESS, stated rather than papered over. This asserts
      // that a second property became populated, NOT that it holds the right
      // value: a model writing `signed_off: false` would still pass, since
      // `populatedCount` counts `false` as present. The clause vocabulary
      // cannot express "some property equals boolean true" — `hasPropertyValue`
      // intercepts `true` as "any present value" (end-state.ts:161) and its
      // string branch rejects booleans outright (:177), so neither `true` nor
      // `"true"` expresses it. Naming the key would reintroduce the vocabulary
      // guess this group's type is deliberately free of. The gap is narrow (a
      // model that resolves correctly and then writes the OPPOSITE of what was
      // asked) and is left rather than closed by weakening something else.
      end: {
        updatedNode: {
          contentMatches: "search indexer",
          minProperties: 2,
        },
        noUnexpectedNodes: true,
      },
    },
  ],
  // Indirect-reference decomposition (scenario 13), on SEEDED state.
  //
  // THIS IS THE SCENARIO #2248 ASKED FOR, and the reason it needs seeding is
  // the finding two prior attempts produced.
  //
  // Scenario 6 asserted a resolve-then-act chain for "the five-day one". #2242
  // found the referent was a direct string match: a create_node write is
  // replayed into later turns as a terse fact carrying its property values AND
  // its id inline, so the discriminator was already in the prompt. Scenario 12
  // then tried a COMPARATIVE over three written values; #2250's review found
  // that fails the same way for a subtler reason — the values are all inline,
  // so the ranking is derivable in-context without reading anything.
  //
  // The generalisation, which is what makes this group's shape necessary:
  // NO VALUE A SCORED TURN WRITES CAN BE THE REFERENT. Every scalar the agent
  // writes is replayed (`terse_write_fact`), so the target is always in the
  // prompt as literal text. A genuinely indirect reference needs a referent the
  // agent NEVER WROTE.
  //
  // Hence `seedGroup`: these incident records are created through the CLI
  // before the group's first turn. `completed_writes` is built only from a
  // turn's own tool executions, so seeded nodes are absent from the rendered
  // chat history entirely — no title, no property value, no id.
  //
  // "ABSENT FROM HISTORY" IS NOT "ABSENT FROM THE PROMPT", and the difference
  // is exactly where the two prior attempts went wrong, so state the boundary
  // rather than restate the claim. Seeding creates a SCHEMA as well as the
  // instances, and workspace context retrieves schemas semantically and
  // interpolates them into the system prompt — so the model DOES see that an
  // `incident_report` type exists with an `on_call` field. What it does not see
  // is any instance: only `"schema"`-type nodes are retrieved that way, so the
  // three titles and the `rowan` -> `search index corruption` mapping stay out.
  //
  // That is the property 13 actually rests on. Knowing the type and field names
  // tells the model HOW TO ASK; it does not tell it WHICH incident to update.
  // The read is still forced. Recorded by
  // `scenario_13_seeded_schema_reaches_the_prompt_but_its_instances_do_not`
  // (packages/core/src/ops/context_ops.rs), which proves the rendering half —
  // that schema vocabulary reaches the prompt.
  //
  // WHAT NOTHING CURRENTLY GUARDS, so it is written down rather than assumed:
  // 13 depends on only `"schema"`-type nodes being retrieved into workspace
  // context (`semantic_search_nodes_of_type` in context_ops). If that ever
  // widens to instance nodes, 13's referent becomes directly matchable from
  // the system prompt and the scenario degrades into the string match that
  // cost scenarios 6 and 12 their indirection — and no test in this repo would
  // fail. That retrieval call is the thing to check before changing workspace
  // context.
  //
  // Pinned by `scenario_13_seeded_referent_is_absent_from_history` in
  // daemon/src/services/local_agent_service.rs, which renders the real history
  // for this group and asserts the referent, the type and the on-call name are
  // all absent — the inverse of scenario 6's test, which pins that ITS referent
  // is present.
  //
  // This is also the only scenario where A READ is genuinely forced, which is
  // why `toolSequence` lives here: with the referent absent from history, no
  // direct write can reach the right id. That is the assertion kind a prior
  // attempt left dead and neither subsequent attempt could honestly revive.
  //
  // WHICH read is not forced, and saying otherwise would repeat the overclaim
  // that sank both prior attempts: `resolve_query` reaches the same node. The
  // subsequence names `search_nodes` because that is the route the winnability
  // test proves; a model resolving otherwise shows as a trajectory mismatch
  // against a passing outcome. That is acceptable only because `expect` is a
  // diagnostic rather than the score.
  [
    {
      id: "13",
      scenario: "13. Indirect reference: seeded referent",
      // Names the on-call engineer, which appears ONLY in seeded state, and the
      // incident by no other identifier. There is nothing in history to match
      // against, so a lookup is the only route to the id.
      //
      // WINNABILITY: the seeded type carries `on_call`, so the value the prompt
      // names is one the schema can actually hold and filter on. The
      // ideal `search_nodes` + `update_node` pair is proved acceptable against a
      // live backend by
      // `scenario_13_ideal_lookup_then_write_is_accepted` in
      // agent/tests/it/matrix_scenario_winnability.rs.
      prompt: "The incident Rowan was on call for — mark it resolved",
      // DIAGNOSTIC, and the one place a SUBSEQUENCE is honest. Both prior
      // attempts had to drop `toolSequence` because a direct write was a
      // legitimate route; here it is not, because the id cannot be known
      // without a read. `search_nodes` is named as the read because the seeded
      // records are queryable by property and it is the tool the winnability
      // test proves; a model resolving via `resolve_query` instead will show as
      // a trajectory mismatch against a passing outcome, which is exactly the
      // disagreement worth reading after a run.
      expect: {
        kind: "toolSequence",
        tools: ["search_nodes", "update_node"],
        minProperties: 1,
      },
      // THE SCORE. `contentMatches` pins the ONE seeded incident Rowan was on
      // call for, so resolving to either of the other two — both real nodes,
      // both updatable, both scoring green under a clause that only counted
      // properties — fails. Naming the node is what makes this measure the
      // resolution rather than the write.
      //
      // `updatedNode`, not `createdNode`: the target already exists (seeded
      // before the turn, so it is in the pre-turn snapshot). A turn that
      // records the resolution on a NEW node has left the incident untouched.
      //
      // `minProperties: 2` for scenario 12d's reason — the seeded node already
      // carries `on_call` and `resolved`, so a lower bar would pass on a turn
      // that resolved correctly and then wrote nothing. Same residual weakness
      // as 12d, documented there: this asserts a property became populated,
      // not that it holds the right value.
      end: {
        updatedNode: {
          contentMatches: "search index corruption",
          minProperties: 2,
        },
        noUnexpectedNodes: true,
      },
    },
  ],
];

const fixture: EvalFixture = {
  name: "agent-matrix",
  description: "Agent Eval Results (end-to-end tool-call behavior)",
  groups: GROUPS,
  /**
   * Establish scenario 13's referent outside any scored turn.
   *
   * Keyed on the group containing scenario 13 rather than run unconditionally:
   * every other group builds its own state through its scenarios, and seeding
   * for them would put nodes in the graph that `noUnexpectedNodes` would then
   * count against the turn under test.
   */
  seedGroup(env: EvalEnv, group: ScenarioGroup) {
    if (group.some((s) => s.id === "13")) seedIncidents(env);
  },
  // Trajectory. No longer the score — the runner records this as
  // `ScenarioResult.trajectory` and scores `graph.scoreOutcome` instead.
  score(scenario, turns) {
    const toolsCalled = turns.flatMap((t) => t.toolsCalled);
    const toolCalls = turns.flatMap((t) => t.toolCalls ?? []);
    return assertExpectation(
      (scenario as MatrixScenario).expect,
      toolsCalled,
      toolCalls,
    );
  },
  graph: {
    /**
     * Extra types to enumerate beyond the registered schemas.
     *
     * Empty, and correctly so: `captureSnapshot` enumerates `schema list`
     * first, the daemon refuses to create a node whose type has no schema, and
     * `task`/`text` are core schemas already in that list. So every type a
     * scenario can touch — including one the model invents mid-run — is
     * covered without naming any here, and a named type that is not a schema
     * (an earlier draft listed `note`) buys nothing but a failed CLI
     * round-trip per snapshot.
     *
     * The seed is kept as a mechanism for a type that could exist without a
     * schema; nothing needs it today.
     */
    types: [],
    scoreOutcome(scenario, diff, turns?) {
      // ROUTING_TOOLS is this fixture's notion of "not an action", so the
      // shared helper takes it as a predicate rather than importing the list
      // and drifting from it.
      // Defaulted: a caller that only has a diff (unit tests, and any future
      // consumer scoring a recorded run) still gets the graph assertions.
      const seen = turns ?? [];
      const toolsCalled = seen.flatMap((t) => t.toolsCalled);
      const reply = seen.length ? seen[seen.length - 1].reply : undefined;
      const asked = turnAskedForClarification(toolsCalled, reply, (t) =>
        ROUTING_TOOLS.includes(t),
      );
      return assertEndState((scenario as MatrixScenario).end, diff, asked);
    },
  },
  // The action tools this scenario's expectation makes reachable.
  //
  // `noTools` returns nothing because it asserts an ABSENCE — withholding a
  // tool cannot make "no tools fired" unreachable.
  //
  // `noRetry` returns nothing for a narrower reason: this fixture grades on
  // graph outcome, so its trajectory verdict is a diagnostic rather than the
  // score. (Its `minCalls` variant IS unreachable without the tool — scenario
  // 11 uses it — so if `noRetry` ever became load-bearing again, it would need
  // to report `[tool]` when `minCalls` is set.)
  requiredTools(scenario) {
    const e = (scenario as MatrixScenario).expect;
    if (e.kind === "toolOnce") return [e.tool];
    if (e.kind === "toolSequence") return e.tools;
    return [];
  },
  extra(scenario, turns: TurnRecord[]) {
    return {
      // The scored expectation, and the trajectory one kept beside it as a
      // diagnostic — a results file should say what was actually asserted.
      end: (scenario as MatrixScenario).end,
      expect: (scenario as MatrixScenario).expect,
      // Per turn, not `turns[0]` alone: a scenario's turns can be offered
      // different tool surfaces (routing runs per turn and scopes Stage 2's
      // tools from that turn's candidates), so collapsing to the first turn
      // reports a surface later turns never saw. Same for the routed skill.
      //
      // Anticipatory today — every caller currently passes a single scored
      // turn, so these arrays hold one element. Kept per-turn because the
      // collapsing is what would be silently wrong the moment a scenario scores
      // more than one turn, and that is invisible in the results file.
      toolsOffered: turns.map((t) => t.toolsOffered),
      routedSkills: turns.map((t) => t.routedSkills ?? ""),
      toolsCalled: turns.flatMap((t) => t.toolsCalled),
      // Recorded so a failure carries its evidence: which call errored, and how
      // many fields it actually persisted. Reading a results file should not
      // require re-running the eval to find out why a scenario failed.
      toolCalls: turns.flatMap((t) => t.toolCalls ?? []),
      latencyMs: turns.reduce((sum, t) => sum + t.latencyMs, 0),
    };
  },
};

export default fixture;
