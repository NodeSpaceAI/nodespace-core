/**
 * Shared types for the eval harness.
 *
 * An eval is a fixture module: a scenario list plus a scoring function. Every
 * other concern — the environment contract, the preflight gate, results
 * assembly, the summary table, baseline diffing, and exit codes — belongs to
 * scripts/eval/runner.ts and is not reimplemented per eval.
 */

import type { EvalEnv } from "./env.ts";
import type { GraphDiff } from "./graph.ts";

/**
 * One tool call's outcome, beyond its name.
 *
 * Sourced from the executor's own report of what it did — `isError` is only
 * false once the write returned Ok, and `fieldCount` is read from the tool
 * RESULT rather than its arguments. Asserting on arguments would measure what
 * the model asked for, which is a property of the model's output and so the
 * same class of evidence as counting tool names; the result is what the system
 * actually persisted.
 */
export interface ToolCallRecord {
  name: string;
  isError: boolean;
  /**
   * Length of the result's `fields` array, for tools that return one.
   *
   * Absent when the tool does not report fields at all; `0` when a schema
   * persisted with no properties — a real failure that is indistinguishable
   * from success by tool name alone, and which `create_schema` reaches by
   * design (a call carrying neither `fields` nor `description` succeeds).
   */
  fieldCount?: number;
  /**
   * The write had no properties to persist in the first place — a plain text
   * node, or an update that changed only content. Both legitimately report a
   * count of zero, so the tool omits `fieldCount` for them.
   *
   * Without this, that absence is indistinguishable from a baseline recorded
   * before the field existed, and `callPersistedProperties` skips the call as
   * "unknown" — which made a `minProperties` assertion pass whether or not the
   * model actually persisted anything.
   */
  contentOnly?: boolean;
}

/** One turn's observable outcome, scraped from an aichat.ts run. */
export interface TurnRecord {
  /**
   * The tools the model was actually offered this turn, comma-separated.
   *
   * Post-scoping: this is `routing::stage2_tools`'s output, so a turn routed to
   * a skill reports the narrowed surface rather than the full registry. An
   * assertion that a scenario failed *because a tool was unavailable* is
   * checkable from a results file only via this field.
   */
  toolsOffered: string;
  /**
   * Skills routed to this turn, comma-separated; empty when none cleared the
   * score gate or the turn never reached retrieval.
   *
   * Optional because results files recorded before this field existed do not
   * carry it — absence means "not recorded", not "nothing routed". A caller
   * distinguishing the two wants `stage2CandidatesInjected`.
   */
  routedSkills?: string;
  /**
   * Tool names in call order.
   *
   * Kept as a plain string[] rather than folded into `toolCalls` below because
   * it has a live reader: the routing fixture scores turns off these names
   * directly and never calls into the matrix fixture's assertions. This is a
   * field in current use, not a compatibility shim the greenfield rule would
   * tell us to delete.
   */
  toolsCalled: string[];
  /**
   * Per-call outcomes, parallel to `toolsCalled`.
   *
   * Optional because results files recorded before this field existed do not
   * carry it; a scoring function must treat absence as "unknown", not "failed".
   */
  toolCalls?: ToolCallRecord[];
  reply: string;
  latencyMs: number;
  /**
   * Wall-clock of Stage-1 routing alone, in ms.
   *
   * The cost of *deciding* rather than of answering: one full generative pass
   * whose entire output is a structural choice among three routing tools. Kept
   * apart from `latencyMs` because a decision-model comparison turns on both
   * terms independently — cheaper-but-worse and better-but-no-cheaper are
   * different propositions, and a turn-level total cannot separate them.
   *
   * Undefined when the turn never routed (no embedding service, or a path that
   * returns before retrieval).
   */
  routingMs?: number;
  /**
   * The turn never reached the model — the send itself failed.
   *
   * Distinguished from a turn that ran and called no tools, because the two are
   * indistinguishable by tool calls alone and a negative assertion ("no tools
   * called") scores a failed send as a PASS. The runner uses this to abort when
   * sends fail consecutively rather than scoring a dead environment.
   */
  sendFailed?: boolean;
  /**
   * Stage 1's routing outcome for this turn: `"query"`, `"clarify"`,
   * `"clarify_suppressed"`, `"none"`, `"unavailable"`, or `"failed"`.
   *
   * Undefined when the daemon log carried no routing line at all (a build
   * predating this field, or a log slice that was truncated). A scenario whose
   * final action still passed can have routed by falling through
   * (`"none"`/`"unavailable"`) rather than genuinely matching a skill — this
   * field is what lets that distinction survive into the results file instead
   * of collapsing into the same pass/fail as a real match.
   */
  routingDecision?: string;
  /**
   * Whether Stage 2's prompt actually carried a candidate block.
   *
   * `false` and `undefined` are NOT the same: `false` means routing ran and
   * produced no eligible candidate (an observed outcome), `undefined` means
   * the log line was not found at all (older build, truncated slice). Without
   * this, a turn that silently skipped injection is indistinguishable from one
   * that never routed — both just fall through to the same tool surface.
   */
  stage2CandidatesInjected?: boolean;
  /**
   * Raw model output for this turn's final reply-producing generation,
   * verbatim before any narration/tool-call normalization.
   *
   * Every false result on record so far was diagnosable from one raw
   * generation and invisible in any aggregate score — this is what lets a
   * results file answer "what did the model actually say" without re-running
   * the eval. Absent when the daemon log carried no raw-response span for this
   * turn (an older build, or a turn whose generation never completed).
   */
  rawOutput?: string;
  /**
   * The turn's reply was a degenerate empty generation — the documented
   * failure mode where the model opens a turn and emits neither text nor a
   * tool call (see `agent_loop.rs`'s "Agent returned empty response with no
   * tool calls"). Distinguished from `sendFailed` because the daemon's polling
   * loop reports this the same way it reports a real timeout: `status` never
   * reaches `idle` with a new assistant message.
   *
   * Scored scenarios with this set true are excluded from the pass/fail
   * denominator rather than counted as failures, per the eval's own
   * documentation of the behavior.
   */
  emptyGeneration?: boolean;
  /**
   * The named schema/operation decisions this turn made, in round order.
   *
   * Recorded per ReAct round rather than per turn: a turn that searched and
   * then wrote made two operation decisions, and both are scoreable. Optional
   * because results files recorded before this field existed do not carry it —
   * absence means "not recorded", not "no decision made".
   */
  decisions?: DecisionRecord[];
}

/**
 * One recorded decision: what the model could have picked, and what it picked.
 *
 * The candidate set is carried alongside the outcome because an outcome alone
 * is not scoreable — whether calling `search_nodes` was right depends on what
 * else was on offer. Mirrors `local_agent::decisions::DecisionRecord` on the
 * Rust side; the two are joined by the `[decision ...]` log marker.
 */
export interface DecisionRecord {
  /**
   * Which selection this describes.
   *
   * `skill` is upstream of the other two and is not a model decision at all —
   * it is deterministic retrieval plus a score bar. It is recorded because it
   * constrains what the model could then choose: a type-definition request
   * that retrieves Node Creation never sees `create_schema`, so the turn reads
   * as an operation failure when it is a retrieval failure.
   */
  kind: "skill" | "schema" | "operation";
  /** Everything on offer, in presentation order. */
  candidates: string[];
  /**
   * What was picked, or `null` when nothing was.
   *
   * `null` is a real outcome, not a missing value: a turn offered tools that
   * called none is ADR-056's Scenario 6 shape, and the failure class that
   * motivated recording these at all.
   */
  selected: string | null;
  /**
   * The selection names something the candidate set never offered.
   *
   * Impossible for operations (the surface is scoped before the model sees it,
   * and the grammar constrains the call envelope to a registered tool name),
   * possible for schemas (nothing constrains a `node_type` argument to a
   * retrieved candidate). The single most diagnostic signal here: unlike a
   * close call between plausible options, it cannot be explained as a hard
   * choice.
   */
  offMenu: boolean;
}

/** The verdict a fixture's scoring function returns for one scenario. */
export interface Verdict {
  passed: boolean;
  failure?: string;
}

/**
 * One scored unit of an eval.
 *
 * `id` is the key baseline diffing joins on, so it must be stable across runs —
 * rewording a `prompt` is safe, renaming an `id` silently reads as one scenario
 * removed and another added.
 */
export interface Scenario {
  id: string;
  /** Human-readable description shown in the summary table. */
  scenario: string;
  /** The message whose turn gets scored. */
  prompt: string;
  /**
   * Turns run before `prompt` to establish context. They share the scenario's
   * chat node and are NOT scored.
   */
  priorTurns?: string[];
  /**
   * This scenario exists only to establish state for later ones — run and
   * recorded, but NOT scored.
   *
   * Scenarios within a group share a chat node, so an early failure craters
   * every scenario after it and the results stop being independent
   * observations: one ambiguous verb in a setup turn has been traced through
   * three downstream failures that were all the same failure counted three
   * times. Marking setup as setup keeps the state it establishes while
   * removing the phantom failures from the denominator.
   *
   * A setup turn is still recorded in full, and the runner reports whether it
   * did what it was supposed to — a setup turn that silently did nothing makes
   * its successors unwinnable, so it must not vanish from the results file.
   */
  setup?: boolean;
  /**
   * Free-form per-eval fields (expectation shape, load-bearing flags, ...).
   * The runner passes these back to `score` untouched and records them in the
   * results file; it never interprets them.
   */
  [key: string]: unknown;
}

/**
 * A group of scenarios sharing one chat node, run in order so later scenarios
 * see earlier turns. A single-element group is an isolated scenario.
 */
export type ScenarioGroup = Scenario[];

/** One scenario's full record in the results file. */
export interface ScenarioResult {
  id: string;
  scenario: string;
  prompt: string;
  passed: boolean;
  failure?: string;
  /** The scored turn plus any prior-context turns, in order. */
  turns: TurnRecord[];
  /** Eval-specific fields the fixture chose to record. */
  extra?: Record<string, unknown>;
  /**
   * The scored turn hit the documented degenerate-empty-generation failure
   * mode (see `TurnRecord.emptyGeneration`) rather than a real pass/fail.
   *
   * `passed` is still populated (as `false`, since the assertion had nothing
   * to work with) so older tooling that ignores this field degrades safely,
   * but the run summary excludes scenarios with this set from the
   * total/passed/failed denominator — scoring an inference bug as a model
   * failure silently deflates every cell it appears in.
   */
  excludedAsEmptyGeneration?: boolean;
  /**
   * This scenario was fixture setup (see `Scenario.setup`) — run and recorded,
   * excluded from the scored denominator.
   *
   * `passed` still carries the verdict so a setup turn that failed to establish
   * its state is visible rather than silently invalidating its successors; it
   * simply does not count as a scored observation.
   */
  excludedAsSetup?: boolean;
  /**
   * What the graph actually did this turn: the diff between the pre-turn and
   * post-turn snapshots.
   *
   * Recorded so a verdict carries its evidence. Reading a results file should
   * not require re-running the eval to find out what was written.
   */
  graphDiff?: GraphDiff;
  /**
   * The verdict the retired TRAJECTORY assertions would have returned, kept as
   * a diagnostic alongside the outcome score that replaced them.
   *
   * Not scored. It is here because trajectory answers a question outcome
   * cannot — HOW the model got there, which is what a debugging session
   * actually needs — and because a scenario where the two disagree is the
   * signal that either the expectation or the model changed. Absent on results
   * files written before outcome grading landed.
   */
  trajectory?: { passed: boolean; failure?: string };
  /**
   * The scenario asserted a tool that Stage-2 routing never offered this turn,
   * so the assertion was unreachable. Excluded from the denominator for the
   * same reason as `excludedAsEmptyGeneration`.
   */
  excludedAsToolNotOffered?: boolean;
  /**
   * A `setup: true` turn earlier in this scenario's group failed its own
   * assertion, so the state this scenario depends on was never established and
   * its assertion is unreachable. Excluded from the denominator for the same
   * reason as `excludedAsToolNotOffered`: the harness knows the precondition is
   * missing, so a red here would describe the fixture, not the model.
   */
  excludedAsSetupFailed?: boolean;
}

/**
 * An eval definition. `scripts/eval/fixtures/*.ts` default-export one of these
 * and contain nothing else of substance.
 */
export interface EvalFixture {
  /** Short slug used in the default results filename and log prefix. */
  name: string;
  /** One-line description shown in the summary header. */
  description: string;
  /**
   * Scenario groups. Each group gets a fresh chat node; scenarios within a
   * group share it.
   */
  groups: ScenarioGroup[];
  /**
   * Establish graph state for a group BEFORE its first turn runs.
   *
   * Called once per group, after its chat node exists and before any scenario
   * in it is dispatched. Returns silently for groups that need nothing.
   *
   * WHY THIS EXISTS, since it is the one place the harness writes fixture data
   * rather than asserting on it. A scenario that tests resolving an INDIRECT
   * reference needs a referent the agent never wrote, and nothing a scored turn
   * creates can be one: every write a turn makes is replayed into later turns
   * as a terse fact carrying its property values and its id inline (see
   * `terse_write_fact` in the daemon), so the referent is always sitting in the
   * prompt as literal text. Seeding outside the scored turns is what keeps the
   * referent out of that channel — `completed_writes` is built only from a
   * turn's own tool executions, so a node written this way is absent from the
   * rendered chat history entirely.
   *
   * SCOPE, because "absent from history" is not "absent from the prompt" and
   * conflating the two is how two prior attempts at this shipped a false claim.
   * A seed that also creates a SCHEMA puts that schema's type name and field
   * names in front of the model: workspace context retrieves schemas
   * semantically and interpolates them into the system prompt. What stays out
   * is INSTANCE data — titles, property VALUES, ids — because only
   * `"schema"`-type nodes are retrieved that way.
   *
   * That boundary is what a seeded scenario must be designed against: seeding
   * hides the answer, not the vocabulary. Recorded by
   * `scenario_13_seeded_schema_reaches_the_prompt_but_its_instances_do_not`
   * (packages/core/src/ops/context_ops.rs).
   *
   * NOT a contradiction of the harness's "assert, do not own" stance, which is
   * about DAEMON AND DATABASE LIFECYCLE — the caller starts the daemon and owns
   * the DB; this harness never does. Writing nodes through the CLI is something
   * the runner already does for every group (`newChat` creates the chat node
   * the same way); this hook is that same capability, named.
   *
   * Runs through the CLI rather than touching a store directly, so seeded state
   * is byte-identical to what the daemon would have produced.
   */
  seedGroup?(env: EvalEnv, group: ScenarioGroup): void;
  /**
   * Score one scenario from its turns. `turns` excludes prior-context turns,
   * which the runner strips before calling this.
   *
   * Must be pure and daemon-free so it is unit-testable without a model.
   */
  score(scenario: Scenario, turns: TurnRecord[]): Verdict;
  /**
   * Opt in to graph end-state grading.
   *
   * Present only on evals whose scenarios act on the graph. The routing eval
   * scores which SKILL fired and writes nothing, so snapshotting it would cost
   * a CLI round-trip per turn to diff a graph that never changes — hence a
   * capability a fixture declares rather than a behavior every eval pays for.
   *
   * When present, the runner captures a snapshot before and after each scored
   * turn and passes the diff to `scoreOutcome`, whose verdict is THE score.
   * `score` still runs, and its verdict is recorded as a trajectory
   * diagnostic (see `ScenarioResult.trajectory`).
   */
  graph?: {
    /**
     * Node types to enumerate when snapshotting. Types created during the run
     * are discovered from the schema list and do not need to be listed here;
     * this is the seed set for types that may already exist.
     */
    types: string[];
    /**
     * Score one scenario from what the graph actually did. Must be pure over
     * the diff, so it is unit-testable without a daemon.
     */
    scoreOutcome(
      scenario: Scenario,
      diff: GraphDiff,
      /**
       * The scored turn, for the one property a diff cannot express: whether
       * the model ASKED instead of acting.
       *
       * A clarification writes nothing, so it is indistinguishable from doing
       * nothing by end-state alone — yet on an underdetermined prompt it is
       * the correct outcome, and scoring it as failure rewards a model that
       * guesses over one that checks.
       */
      turns?: TurnRecord[],
    ): Verdict;
  };
  /**
   * Optional: the action tools this scenario's expectation asserts on.
   *
   * Stage-2 routing scopes the tool surface per turn, so a scenario can assert
   * a tool the model was never offered — it then reds out for "not calling" a
   * tool it could not call. That is an environment fault of the same class as
   * a degenerate empty generation, and the runner excludes it on the same
   * grounds rather than scoring it (see the `toolsOffered` guard in run()).
   *
   * Return an empty array for scenarios that assert an absence (`noTools`),
   * where no tool needs to be offered for the assertion to be reachable.
   */
  requiredTools?(scenario: Scenario): string[];
  /**
   * Optional per-scenario fields to record in the results file beyond
   * pass/fail — routing signals, matched skill, and similar.
   */
  extra?(scenario: Scenario, turns: TurnRecord[]): Record<string, unknown>;
  /**
   * Optional extra summary lines (e.g. "load-bearing: 4/5"), rendered under
   * the headline pass count.
   */
  summary?(results: ScenarioResult[]): string[];
}

/**
 * Conditions under which a run was produced. Recorded in every results file so
 * a cited number carries its context and cannot outlive it.
 */
export interface Provenance {
  /** Model id the daemon actually had loaded. */
  model: string;
  /** ISO-8601 timestamp of the run. */
  recordedAt: string;
  /** Host physical RAM in GB, as reported by the daemon. */
  hostMemoryGb: number;
  /**
   * Context window the daemon granted this model. The eval is only meaningful
   * when this comfortably exceeds the agent's system prompt; see preflight.ts.
   */
  nCtx: number;
  /**
   * Set when the loaded model could only be matched to `NS_MODEL` by filename,
   * because the daemon reported a resolved path rather than a catalog id. The
   * exact build behind this number is unconfirmed.
   */
  modelMatchedByPath?: boolean;
  /** Commit of the repo the eval ran from. */
  evalCommit: string;
  /** True when the working tree had uncommitted changes at run time. */
  dirty: boolean;
  /**
   * Which seeded guidance this run actually measured.
   *
   * Seeding is content-versioned (`_seed.version`/`_seed.key` on each seeded
   * prompt/skill node) and only replaces stale content on daemon startup — a
   * long-running daemon started before a guidance edit landed keeps serving
   * the old content indefinitely, silently. This is read back from the served
   * database right before the run (see `readGuidanceProvenance` in
   * preflight.ts) so a stale-guidance run cannot be mistaken for a fresh one
   * without re-querying the database by hand.
   */
  guidance?: GuidanceProvenance;
}

/**
 * Seed keys and versions for one seeded node type, as actually served.
 *
 * A map rather than a fixed shape because the set of seeded types (currently
 * prompt, skill) is a property of the schema, not of this eval.
 */
export interface GuidanceProvenance {
  [nodeType: string]: Array<{ key: string; version: string }>;
}

/** One repetition's scored scenarios and the conditions that produced them. */
export interface RepResult {
  /** 1-based index of this repetition within the run. */
  rep: number;
  /**
   * Provenance for THIS rep, not the run.
   *
   * Recorded per rep rather than once per file because the whole point of
   * repeating is that reps are not interchangeable: one that hit a degraded
   * environment — a daemon restarted onto different guidance, a model that
   * loaded into a smaller window the second time, a working tree that went
   * dirty mid-run — must be identifiable after the fact. A single file-level
   * provenance block would silently attribute every rep to the first rep's
   * conditions.
   */
  provenance: Provenance;
  summary: RepSummary;
  results: ScenarioResult[];
}

/** One rep's scored totals. */
export interface RepSummary {
  /**
   * Scored scenarios only — excludes empty-generation and setup exclusions.
   */
  total: number;
  passed: number;
  failed: number;
  /**
   * Scenarios excluded as degenerate empty generations, not scored either
   * way. Kept separate from `failed` so the rate is visible rather than
   * silently deflating the pass rate; present (possibly 0) whenever the
   * runner supports the exclusion, so its absence marks an older results
   * file rather than a run with none.
   */
  excludedEmptyGenerations?: number;
  /**
   * Scenarios excluded as fixture setup (see `Scenario.setup`), not scored
   * either way. Reported separately from `excludedEmptyGenerations` because
   * the two mean opposite things: an empty generation is a fault whose rate
   * matters, while a setup turn is a deliberate, fixed part of the fixture.
   */
  excludedSetup?: number;
  /**
   * Scenarios excluded because the tool their assertion names was never
   * offered to the model this rep, not scored either way. Reported
   * separately from `excludedEmptyGenerations`: an empty generation is an
   * inference bug, this is a routing miss — the two need separate lines so a
   * shrunken denominator says which one caused it.
   */
  excludedToolNotOffered?: number;
  /**
   * Scenarios excluded because their group's fixture setup (`seedGroup`)
   * failed, not scored either way. Reported separately again: this names the
   * fixture's own precondition as the cause, neither an inference bug nor a
   * routing miss.
   */
  excludedSetupFailed?: number;
  /**
   * How many scored scenarios' trajectory diagnostic DISAGREED with the
   * outcome score. Not a failure count — it is the number worth looking at
   * after a rep, because each disagreement is either a scenario whose
   * expectation no longer describes the behavior or a real change in how the
   * model reaches its result.
   */
  trajectoryDisagreements?: number;
}

/**
 * One scenario's outcome across every rep of a run.
 *
 * This is the unit the multi-rep summary is built from, and the reason the
 * eval repeats at all: "fails 3/3" and "fails 1/3" are different findings that
 * a single run renders identically, and only the second one is debuggable.
 */
export interface ScenarioReliability {
  id: string;
  scenario: string;
  /** Reps in which this scenario was actually scored (exclusions removed). */
  scoredReps: number;
  /** Scored reps in which it passed. */
  passedReps: number;
  /**
   * Reps excluded as degenerate empty generations. Counted toward neither
   * `passedReps` nor the pass^k denominator — an inference bug is not evidence
   * either way about the scenario, so a rep that hit one must not make a
   * scenario look unreliable.
   */
  excludedReps: number;
  /**
   * The scenario passed in every rep where it was scored, and was scored at
   * least once. This is the pass^k predicate: an all-or-nothing verdict, which
   * is what makes the aggregate a reliability measure rather than a best-case
   * one.
   */
  passedAll: boolean;
  /** It both passed and failed across reps — the score is not reproducible. */
  flipped: boolean;
  /**
   * Every rep of this scenario was fixture setup (see `Scenario.setup`) —
   * run and recorded, excluded from the scored denominator.
   *
   * Carried up to the reliability level so baseline comparison can skip it:
   * a setup turn's verdict is not a result to regress against, and a baseline
   * predating the setup reclassification still holds a scored verdict under
   * the same id. Its failure is surfaced during the run instead, where it
   * means something — the state its successors needed.
   */
  setup?: boolean;
}

/**
 * A run's aggregate across reps.
 *
 * `passAtK` is the headline and `passAt1` sits alongside it deliberately: the
 * property that matters for a model writing to a user's graph is whether it
 * does the right thing every time, not whether it can. Two models with the
 * same pass^1 and different flip rates are not equally good, and only pass^k
 * separates them.
 */
export interface RunAggregate {
  /** Number of reps that ran. */
  reps: number;
  /**
   * Scenarios scored in at least one rep — the denominator for both metrics
   * below. A scenario excluded as an empty generation in EVERY rep was never
   * scored at all and is counted here as neither passing nor failing.
   */
  scoredScenarios: number;
  /** Scenarios that passed in every rep where they were scored. */
  passAtK: number;
  /**
   * Mean scored pass count across reps — the single-run number this project
   * has historically quoted, kept alongside pass^k so the gap between
   * best-case capability and reliability is readable rather than lost.
   */
  passAt1: number;
  /** Scenarios that both passed and failed across reps. */
  flipped: number;
  /** Per-scenario detail, in fixture order. */
  scenarios: ScenarioReliability[];
}

/**
 * The results file an eval run writes.
 *
 * Always rep-shaped, including for `--runs 1`: a single-rep file that looked
 * structurally different from a multi-rep one would mean every reader needs
 * two code paths, and the single-rep shape is exactly the one that produced
 * the confident wrong conclusions repeating exists to prevent.
 */
export interface EvalResults {
  eval: string;
  label: string;
  /**
   * Provenance of the FIRST rep, for readers that want one block without
   * walking `reps`. The per-rep blocks in `reps[].provenance` are
   * authoritative — a rep that drifted is only visible there.
   */
  provenance: Provenance;
  /** Aggregate across every rep: pass^k, pass^1, and per-scenario flips. */
  aggregate: RunAggregate;
  reps: RepResult[];
}
