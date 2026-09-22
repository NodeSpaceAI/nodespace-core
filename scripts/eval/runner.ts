/**
 * Shared eval harness.
 *
 * Owns everything an eval does NOT need to decide for itself: the environment
 * contract, the preflight gate, argv, chat-node lifecycle, results assembly,
 * the summary table, baseline diffing, and exit codes. An eval supplies only
 * its scenarios and how to score a turn (see ./types.ts, ./fixtures/).
 *
 * Adding an eval is therefore a fixture module, not another copy of this
 * plumbing — which is what let the two original harnesses drift into having
 * separately-implemented, separately-broken versions of the same five concerns.
 */

import { readEnv, ENV_USAGE, REPO_ROOT, type EvalEnv } from "./env.ts";
import { captureSnapshot, diffSnapshots } from "./graph.ts";
import {
  abortOnEnvironment,
  awaitSkillIndex,
  preflight,
  readDaemonStatus,
  readGuidanceProvenance,
  type DaemonStatus,
  EnvironmentError,
  EXIT_FAILED,
  EXIT_USAGE,
} from "./preflight.ts";
import type {
  DecisionRecord,
  EvalFixture,
  EvalResults,
  GuidanceProvenance,
  Provenance,
  RepResult,
  RunAggregate,
  ScenarioReliability,
  ScenarioResult,
  ToolCallRecord,
  TurnRecord,
} from "./types.ts";

// ---------------------------------------------------------------------------
// aichat.ts driver
// ---------------------------------------------------------------------------

/**
 * Create a fresh chat node; return its id.
 *
 * Failing here means the daemon went away mid-run — preflight already proved it
 * was reachable. That is an environment failure, not a scenario failure, and
 * must not be reported (or exit) as one: the scenarios scored before the daemon
 * died would otherwise look like a partial result rather than a void run.
 */
function newChat(env: EvalEnv): string {
  const r = Bun.spawnSync(["bun", "run", env.aichat, "new"], {
    stdout: "pipe",
    stderr: "pipe",
    env: { ...process.env },
  });
  if (r.exitCode !== 0) {
    throw new EnvironmentError(
      `The daemon stopped responding partway through the run: could not create a ` +
        `chat node.\n  ${r.stderr.toString().trim()}`,
      `Check that the daemon on ${env.socket} is still alive (and did not run out ` +
        `of memory or get killed).\n  Partial scores from an aborted run are not ` +
        `comparable to a complete one, so none were written.`,
    );
  }
  return r.stdout.toString().trim();
}

/**
 * Pause before a turn, to stay under a served endpoint's per-minute cap.
 *
 * Synchronous on purpose: `runTurn` is sync, the harness is serial, and making
 * the call chain async purely to await a sleep would be a large diff for no
 * behavioural gain. `Atomics.wait` on a throwaway buffer is the standard way
 * to block a worker-free main thread without a spin loop.
 *
 * Deliberately OUTSIDE the timed region below — folding it into `latencyMs`
 * would make a paced arm's latency incomparable to an unpaced one.
 */
function pauseBeforeTurn(): void {
  const ms = Number(process.env.NS_TURN_DELAY_MS ?? 0);
  if (!Number.isFinite(ms) || ms <= 0) return;
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

/**
 * Run one turn and scrape its outcome.
 *
 * A failed send is recorded rather than thrown, so one flaky turn does not
 * abandon a run that costs minutes of inference. It is flagged `sendFailed` so
 * the caller can tell it apart from a turn that genuinely called no tools —
 * scoring the two alike is what lets a dead daemon pass a "no tools" assertion.
 * The run loop aborts once sends fail consecutively.
 */
function runTurn(env: EvalEnv, chatId: string, message: string): TurnRecord {
  pauseBeforeTurn();
  const start = performance.now();
  const r = Bun.spawnSync(["bun", "run", env.aichat, "send", chatId, message], {
    stdout: "pipe",
    stderr: "pipe",
    env: { ...process.env },
  });
  const latencyMs = Math.round(performance.now() - start);

  if (r.exitCode !== 0) {
    const err = r.stderr.toString().trim();
    return {
      toolsOffered: `(error: ${err})`,
      toolsCalled: [],
      reply: `(send failed: ${err})`,
      latencyMs,
      sendFailed: true,
    };
  }

  return parseTurnOutput(r.stdout.toString(), latencyMs);
}

/**
 * Parse the `[decision ...]` markers for one turn, in round order.
 *
 * Returns `undefined` rather than `[]` when no marker is present, so a results
 * file from a build predating the markers stays distinguishable from a turn that
 * genuinely made no decision — the same "absent is not false" rule
 * `stage2CandidatesInjected` follows.
 *
 * `candidates=` runs to end of line because the value is a comma-separated list
 * of names that tracing emits unquoted; `selected=` is matched as a
 * non-whitespace run for the same reason it can be, being a single identifier.
 * Anchored to line start like every other marker here: `out` carries arbitrary
 * raw model text via `[raw]` lines, and the model narrating a marker-shaped
 * string must not be read as the harness's own signal.
 */
function parseDecisions(out: string): DecisionRecord[] | undefined {
  const rows = [
    ...out.matchAll(
      /^\[decision (schema|operation)\] selected=(\S*?)( \[off-menu\])? candidates=(.*)$/gm,
    ),
  ];
  if (rows.length === 0) return undefined;
  return rows.map((m) => ({
    kind: m[1] as "schema" | "operation",
    // `none` is the marker's rendering of "offered options, picked nothing".
    // It round-trips back to null rather than the literal string, so a scorer
    // never has to know the wire spelling.
    selected: m[2] === "none" ? null : m[2],
    offMenu: m[3] !== undefined,
    candidates: m[4]
      .split(",")
      .map((c) => c.trim())
      .filter((c) => c.length > 0),
  }));
}

/**
 * Parse aichat.ts's stdout for one turn into a `TurnRecord`.
 *
 * Split out from `runTurn` so this — the actual marker-parsing logic, where
 * both regressions caught in review lived — is unit-testable directly against
 * a fixed string, without spawning `aichat.ts` or a daemon (see runner.test.ts).
 */
export function parseTurnOutput(out: string, latencyMs: number): TurnRecord {
  // One pass over the [tool] lines feeds both shapes, so they cannot disagree
  // about how many calls a turn made. aichat.ts emits:
  //   [tool] <name>[ERROR][ [fields=N]] <args>
  // The args are free-form and truncated, so every structured marker precedes
  // them and nothing here parses them.
  //
  // Anchored to the START of a line (`^` with the `m` flag) — without this, a
  // `[raw]` line's JSON-encoded payload containing the literal substring
  // "[tool] create_node" (the model narrating a tool call in its own text,
  // rather than actually invoking one) matched here too and was counted as a
  // real tool call. Caught in review as a direct consequence of introducing
  // arbitrary model text into this same stdout stream via the [raw] marker.
  const toolCalls: ToolCallRecord[] = [
    ...out.matchAll(
      /^\[tool\] ([a-z_]+)( \[ERROR\])?( \[fields=(\d+)\])?( \[content-only\])?/gm,
    ),
  ].map((m) => ({
    name: m[1],
    isError: m[2] !== undefined,
    ...(m[4] === undefined ? {} : { fieldCount: Number(m[4]) }),
    ...(m[5] === undefined ? {} : { contentOnly: true }),
  }));

  // Raw generations, one per ReAct iteration: `[raw] iteration=N <json-string>`.
  // aichat.ts JSON-encodes the payload before emitting it specifically so this
  // is always exactly one line — a plain `\n`-delimited match here would be
  // unsafe against a lookahead terminator (`\n[tool]`, a following `\n[raw]`)
  // that the model's own raw text could contain literally, which is exactly
  // what an earlier version of this regex got wrong (caught in review before
  // it shipped — see the multiline/embedded-marker tests in runner.test.ts).
  // Iteration order is preserved by matchAll's left-to-right scan, so a
  // multi-round turn's trace reads as one transcript without needing to
  // re-sort.
  const rawLines = [...out.matchAll(/^\[raw\] iteration=(\d+) (.*)$/gm)];
  const rawOutput =
    rawLines.length > 0
      ? rawLines
          .map((m) => `[iteration ${m[1]}] ${JSON.parse(m[2])}`)
          .join("\n")
      : undefined;

  // Every marker below is anchored to the START of a line (`^` with the `m`
  // flag), for the same reason the tool-call regex above is: `out` now
  // contains arbitrary raw model text via `[raw]` lines, and an unanchored
  // match against a marker-shaped substring inside that text — e.g. the model
  // narrating "[routing] query" or "[empty-generation]" in its own words —
  // would be silently counted as the harness's own signal rather than the
  // model's. `assistant> ` is deliberately NOT anchored/multiline here: it is
  // aichat.ts's own final-reply marker, printed exactly once as the last
  // thing on stdout, and `[\s\S]*$` intentionally captures everything after
  // it (a real assistant reply can itself be multiline).
  return {
    toolsOffered: out.match(/^\[tools offered\] (.*)/m)?.[1]?.trim() ?? "",
    routedSkills: out.match(/^\[routed skills\] (.*)/m)?.[1]?.trim(),
    toolsCalled: toolCalls.map((t) => t.name),
    toolCalls,
    reply:
      out.match(/assistant> ([\s\S]*)$/)?.[1]?.trim() ?? "(no reply parsed)",
    latencyMs,
    routingDecision: out.match(/^\[routing\] ([a-z_]+)/m)?.[1],
    stage2CandidatesInjected: (() => {
      const m = out.match(/^\[stage2 injected\] (true|false)/m);
      return m ? m[1] === "true" : undefined;
    })(),
    rawOutput,
    emptyGeneration:
      out.match(/^\[empty-generation\]$/m) !== null || undefined,
    decisions: parseDecisions(out),
  };
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

function gitCommit(): { commit: string; dirty: boolean } {
  const rev = Bun.spawnSync(
    ["git", "-C", REPO_ROOT, "rev-parse", "--short", "HEAD"],
    {
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  const status = Bun.spawnSync(
    ["git", "-C", REPO_ROOT, "status", "--porcelain"],
    {
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  return {
    commit: rev.exitCode === 0 ? rev.stdout.toString().trim() : "(unknown)",
    dirty: status.exitCode === 0 && status.stdout.toString().trim().length > 0,
  };
}

// ---------------------------------------------------------------------------
// Baseline comparison
// ---------------------------------------------------------------------------

/**
 * Read a baseline results file into per-scenario reliability.
 *
 * Baselines are compared on pass^k, not on a single verdict, for the same
 * reason the runner repeats at all: a single-draw baseline diffed against a
 * single-draw run reports the distribution's spread as regressions and fixes.
 * A one-rep baseline still works — its pass^k is just its only verdict — but
 * it inherits that noise, which is why the summary says how many reps each
 * side carried.
 */
export function readBaselineReliability(
  parsed: unknown,
): { reps: number; byId: Map<string, ScenarioReliability> } | null {
  // A baseline that parsed to a non-object (a file containing `null`, a bare
  // number, an array) must warn like any other unreadable baseline rather than
  // throwing — a bad --baseline path cannot be allowed to discard a run that
  // took minutes of inference to produce.
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return null;
  }
  const b = parsed as Partial<EvalResults> & {
    // Pre-`--runs` files carried scenarios at the top level with no reps.
    results?: ScenarioResult[];
  };
  if (Array.isArray(b.reps) && b.reps.length > 0) {
    const agg = aggregateReps(b.reps.map((r) => r.results ?? []));
    return { reps: b.reps.length, byId: new Map(agg.scenarios.map((s) => [s.id, s])) };
  }
  if (Array.isArray(b.results)) {
    const agg = aggregateReps([b.results]);
    return { reps: 1, byId: new Map(agg.scenarios.map((s) => [s.id, s])) };
  }
  return null;
}

/**
 * Compare against a recorded baseline. Returns the regression count.
 *
 * Joins on scenario `id`, not the prompt text: prompts get reworded (the
 * decontamination pass rewrote every one of them) and joining on text would
 * report the whole suite as removed-and-added.
 */
async function compareToBaseline(
  evalName: string,
  aggregate: RunAggregate,
  baselinePath: string,
): Promise<number> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(await Bun.file(baselinePath).text());
  } catch (e) {
    // A baseline that cannot be read is an operator error worth surfacing, but
    // it must not discard a run that took minutes of inference to produce.
    console.error(
      `[${evalName}] Warning: could not read baseline at ${baselinePath}: ${e}`,
    );
    return 0;
  }

  const base = readBaselineReliability(parsed);
  if (!base) {
    console.error(
      `[${evalName}] Warning: baseline at ${baselinePath} carries neither ` +
        `\`reps\` nor \`results\` — nothing to compare against.`,
    );
    return 0;
  }

  const p = (parsed as Partial<EvalResults>).provenance;
  console.log(`── Baseline comparison (vs ${baselinePath}) ──`);
  if (p) {
    console.log(
      `   Recorded: ${p.recordedAt} · model ${p.model} · n_ctx ${p.nCtx}`,
    );
  }
  console.log(
    `   Comparing pass^k: baseline ${base.reps} rep(s) vs this run ${aggregate.reps} rep(s)`,
  );
  if (base.reps === 1 && aggregate.reps === 1) {
    console.log(
      `   Note: both sides are single draws — a difference here is within the ` +
        `run-to-run spread this suite is known to have.`,
    );
  }

  let regressions = 0;

  for (const cur of aggregate.scenarios) {
    const b = base.byId.get(cur.id);
    if (!b) {
      console.log(
        `   NEW         ${cur.id} → ${cur.passedAll ? "pass^k" : "fail"} ` +
          `(${cur.passedReps}/${cur.scoredReps} reps)`,
      );
      continue;
    }
    // Setup scenarios are not scored, so their verdict is not a result to
    // regress against — but a baseline predating the setup reclassification
    // still carries a scored verdict for the same id, and comparing the two
    // would report a REGRESSION the moment a setup turn takes a different
    // (still perfectly valid) path. Its failure is surfaced during the run
    // instead, where it means something: the state its successors needed.
    if (cur.setup) {
      console.log(
        `   SETUP       ${cur.id}: fixture setup, not scored — not compared`,
      );
      continue;
    }
    // A scenario excluded in every rep was never scored this run — an
    // inference bug, not a scoring outcome. Comparing it against a baseline
    // verdict would report a spurious REGRESSION on a scenario the model was
    // never actually measured against this time.
    if (cur.scoredReps === 0) {
      console.log(
        `   EXCLUDED    ${cur.id}: degenerate empty generation in all ` +
          `${cur.excludedReps} rep(s) — not compared`,
      );
      continue;
    }
    if (b.passedAll && !cur.passedAll) {
      console.log(
        `   REGRESSION  ${cur.id}: passed all ${b.scoredReps} baseline rep(s), ` +
          `now ${cur.passedReps}/${cur.scoredReps}`,
      );
      regressions++;
    } else if (!b.passedAll && cur.passedAll) {
      console.log(
        `   FIXED       ${cur.id}: was ${b.passedReps}/${b.scoredReps} in ` +
          `baseline, now passes all ${cur.scoredReps} rep(s)`,
      );
    }
  }

  for (const [id] of base.byId) {
    if (!aggregate.scenarios.some((s) => s.id === id)) {
      console.log(`   REMOVED     ${id}: in baseline, not in this run`);
    }
  }

  console.log(
    regressions > 0
      ? `\n[${evalName}] ✗ ${regressions} regression(s) vs baseline`
      : `\n[${evalName}] ✓ No regressions vs baseline`,
  );
  return regressions;
}

// ---------------------------------------------------------------------------
// Scoring/reporting helpers — pure, so they are unit-testable without a
// daemon (see scripts/eval/runner.test.ts).
// ---------------------------------------------------------------------------

/**
 * The status marker for one result: ⊘ excluded, ⊙ setup, ✓ pass, ✗ fail.
 *
 * Shared by the per-turn line and the summary list so the two cannot drift
 * apart, which they had already started to do by spelling the precedence out
 * twice. The ordering here is defensive rather than observable: a result can
 * only be flagged `excludedAsSetup` after the empty-generation branch has
 * already `continue`d, so nothing carries both flags today. It is pinned
 * anyway, because the day one does, the two call sites disagreeing is exactly
 * the kind of silent divergence this file exists to prevent — and the choice
 * matches `partitionExcluded`, which counts such a scenario in the exclusion
 * bucket whose rate is itself a result.
 */
export function markerFor(r: {
  excludedAsEmptyGeneration?: boolean;
  excludedAsSetup?: boolean;
  excludedAsToolNotOffered?: boolean;
  passed: boolean;
}): string {
  if (r.excludedAsEmptyGeneration) return "⊘";
  // Distinct glyph from ⊘: both are exclusions, but one is an inference bug
  // and this one is a ROUTING miss. Collapsing them would hide which of the
  // two a run is actually suffering from.
  if (r.excludedAsToolNotOffered) return "⊗";
  if (r.excludedAsSetup) return "⊙";
  return r.passed ? "✓" : "✗";
}

/**
 * Split a run's results into scenarios that were actually scored and those
 * excluded — as degenerate empty generations (see `TurnRecord.emptyGeneration`)
 * or as fixture setup (see `Scenario.setup`).
 *
 * The two exclusions are counted separately because they mean opposite things:
 * an empty generation is a fault whose RATE is a result in itself, while a
 * setup turn is a deliberate, fixed part of the fixture. Collapsing them would
 * make a run with a rising empty-generation rate look unchanged.
 *
 * Note that a snapshot that could not be captured is NOT a third bucket: it
 * scores `passed: false` and lands in `failed`. That is deliberate — an
 * environment fault should be loud rather than quietly shrinking the
 * denominator — but it does mean a run against a dying daemon reads as a
 * failing run, not a short one.
 *
 * Kept out of `runEval`'s body so uniformity/totals math is independently
 * testable against a fixed `ScenarioResult[]`, without spawning a daemon.
 */
export function partitionExcluded(results: ScenarioResult[]): {
  scored: ScenarioResult[];
  excludedCount: number;
  setupCount: number;
  /**
   * Turns excluded because Stage-2 never offered the asserted tool.
   *
   * Counted separately from `excludedCount`: both leave the scored set, but an
   * empty generation is an inference bug and this is a routing miss. Reporting
   * them as one number would hide which failure a run is actually suffering,
   * and leaving it uncounted made the totals stop reconciling.
   */
  toolNotOfferedCount: number;
  /**
   * Turns excluded because a setup turn in their group failed.
   *
   * Counted separately for the same reason as `toolNotOfferedCount`: it names a
   * distinct cause (the fixture's own precondition), and folding it into the
   * others would hide which failure a run is suffering.
   */
  setupFailedCount: number;
} {
  const scored = results.filter(
    (r) =>
      !r.excludedAsEmptyGeneration &&
      !r.excludedAsSetup &&
      !r.excludedAsToolNotOffered &&
      !r.excludedAsSetupFailed,
  );
  return {
    scored,
    excludedCount: results.filter((r) => r.excludedAsEmptyGeneration).length,
    setupCount: results.filter(
      (r) => r.excludedAsSetup && !r.excludedAsEmptyGeneration,
    ).length,
    toolNotOfferedCount: results.filter((r) => r.excludedAsToolNotOffered).length,
    setupFailedCount: results.filter((r) => r.excludedAsSetupFailed).length,
  };
}

/**
 * Fold every rep's results into per-scenario reliability plus run totals.
 *
 * The headline it produces is pass^k — a scenario counts only if it passed in
 * every rep that scored it — because that is the property the eval is for. A
 * model that writes to a user's graph correctly two times in three is not a
 * model that works; pass^1 records what it can do, pass^k what it does.
 *
 * Excluded reps are removed from a scenario's denominator rather than counted
 * as failures, so a stray empty generation cannot make a reliable scenario
 * read as a flipping one. A scenario excluded in EVERY rep drops out of
 * `scoredScenarios` entirely — it was never measured, and counting it either
 * way would be inventing a result.
 *
 * Pure, and takes rep results rather than a `RepResult[]`, so the aggregation
 * math is unit-testable against fixed arrays with no daemon and no provenance
 * scaffolding (see runner.test.ts).
 */
export function aggregateReps(reps: ScenarioResult[][]): RunAggregate {
  // Fixture order, taken from the first rep that saw each scenario. Reps run
  // the same fixture, so this is stable; a rep that aborted early simply
  // contributes fewer scenarios rather than reordering them.
  const order: string[] = [];
  const byId = new Map<string, { scenario: string; results: ScenarioResult[] }>();
  for (const rep of reps) {
    for (const r of rep) {
      let entry = byId.get(r.id);
      if (!entry) {
        entry = { scenario: r.scenario, results: [] };
        byId.set(r.id, entry);
        order.push(r.id);
      }
      entry.results.push(r);
    }
  }

  const scenarios: ScenarioReliability[] = order.map((id) => {
    const { scenario, results } = byId.get(id)!;
    // Setup scenarios are excluded from pass^k for the same reason they are
    // excluded from a single run's denominator: they are not observations.
    // Counting them here would put the cascade back one level up — a setup
    // turn that flipped would be reported as an unreliable SCENARIO, when what
    // it actually means is that its successors' reps are not comparable.
    const setup = results.length > 0 && results.every((r) => r.excludedAsSetup);
    const excludedReps = results.filter(
      (r) => r.excludedAsEmptyGeneration,
    ).length;
    const scored = setup
      ? []
      : results.filter((r) => !r.excludedAsEmptyGeneration && !r.excludedAsSetup && !r.excludedAsToolNotOffered);
    const passedReps = scored.filter((r) => r.passed).length;
    return {
      id,
      scenario,
      scoredReps: scored.length,
      passedReps,
      excludedReps,
      passedAll: scored.length > 0 && passedReps === scored.length,
      flipped: passedReps > 0 && passedReps < scored.length,
      ...(setup ? { setup: true } : {}),
    };
  });

  const measured = scenarios.filter((s) => s.scoredReps > 0);
  // pass^1 is the mean of the per-rep scored pass counts — the number a single
  // run would have quoted — not a per-scenario rate, so it stays directly
  // comparable to every score this project has cited from a one-rep run.
  const perRepPassed = reps.map(
    (rep) => partitionExcluded(rep).scored.filter((r) => r.passed).length,
  );
  const passAt1 =
    perRepPassed.length > 0
      ? perRepPassed.reduce((a, b) => a + b, 0) / perRepPassed.length
      : 0;

  return {
    reps: reps.length,
    scoredScenarios: measured.length,
    passAtK: measured.filter((s) => s.passedAll).length,
    passAt1,
    flipped: scenarios.filter((s) => s.flipped).length,
    scenarios,
  };
}

/**
 * How many distinct tools a full-pass run must have called to be believed.
 *
 * Set well below the suite's real diversity (a complete matrix run exercises
 * create/update/search/schema/relationship tools) but above what a degenerate
 * environment can produce: a run whose every turn dies calls zero tools, and one
 * stuck on a single code path calls one. Three is comfortably clear of both
 * without pinning the guard to this fixture's exact tool set.
 */
const MIN_DISTINCT_TOOLS_FOR_REAL_PASS = 3;

/**
 * Decide whether a scored run's pass rate is uniform enough to be a harness
 * signature rather than a result. Returns `null` when the run is fine, or an
 * `EnvironmentError` when it should abort.
 *
 * A pure decision function (as opposed to inlining the check + throw in
 * `runEval`) so the threshold and both boundary cases — one scenario under
 * `minScenarios`, and a genuine partial result — are covered by a unit test
 * that never spawns a daemon.
 */
export function checkUniformity(
  passed: number,
  total: number,
  minScenarios = 4,
  // Number of DISTINCT tools called across the scored scenarios. The signature
  // this guard exists to catch produces identical turns - every send failing the
  // same way, so every turn calls nothing (or the same nothing). A run where the
  // model actually exercised the suite calls many different tools, and that is
  // observable without a model in the loop.
  //
  // Defaulted to 0 so an omitted argument keeps the pre-existing behaviour
  // (every uniform run is suspicious); callers that can measure diversity pass
  // it and get the narrower check.
  distinctToolsCalled = 0,
): EnvironmentError | null {
  if (total < minScenarios) return null;
  if (passed !== 0 && passed !== total) return null;
  // A full pass with a varied tool surface is a RESULT, not a signature.
  //
  // The original premise here - "real runs on this suite have never been
  // perfectly uniform" - held only while outcome scoring was mis-reading
  // type-keyed properties and suppressing passes on correct writes. With that
  // fixed, a capable model legitimately passes everything: DeepSeek V4 Pro
  // scored 21/21 and this guard discarded the run and wrote no results file.
  //
  // A uniform ZERO is left suspicious regardless of diversity: the known false
  // results were all-fail, and a model that calls varied tools and still fails
  // every scenario is exactly the "same unhandled code path" case.
  if (passed === total && distinctToolsCalled >= MIN_DISTINCT_TOOLS_FOR_REAL_PASS) {
    return null;
  }
  return new EnvironmentError(
    `Every scored scenario ${passed === 0 ? "FAILED" : "PASSED"} (${passed}/${total}). ` +
      `A rate this uniform across an entire run is a harness signature, not a result — ` +
      `real runs on this suite have never been perfectly uniform, and the two known false ` +
      `results on record (contaminated 11/12, and the "no tools called" scenarios passing ` +
      `while every turn died on context overflow) both looked like plausible numbers until ` +
      `someone read the raw output.`,
    `Read a raw generation before trusting this number: re-run with the daemon at ` +
      `RUST_LOG=debug and inspect the [raw] lines this eval now captures (see rawOutput on ` +
      `each turn), or check the daemon log directly for an error common to every turn.\n` +
      `  No results file was written for this run.`,
  );
}

/**
 * Decide whether guidance drifted between reps. Returns `null` when it did
 * not, or an `EnvironmentError` naming the drift.
 *
 * Reps are only comparable if they measured the same guidance. Seeding runs at
 * daemon startup and is content-versioned, so any between-rep purge/restart —
 * exactly what this harness asks the operator to do — can silently land a rep
 * on different content: a rebuilt daemon, a purge that did not take, a
 * different checkout. Folding two guidance versions into one pass^k number
 * produces a reliability figure for a system that never existed, so this
 * aborts rather than averaging across it.
 *
 * Pure, so both the stable and drifted cases are testable with no daemon.
 */
export function checkGuidanceDrift(
  first: GuidanceProvenance | undefined,
  current: GuidanceProvenance | undefined,
  rep: number,
): EnvironmentError | null {
  const fingerprint = (g: GuidanceProvenance | undefined): string =>
    JSON.stringify(
      Object.entries(g ?? {})
        .map(([type, entries]) => [
          type,
          [...entries]
            .map((e) => `${e.key}@${e.version}`)
            .sort((a, b) => a.localeCompare(b)),
        ])
        .sort((a, b) => (a[0] as string).localeCompare(b[0] as string)),
    );

  const a = fingerprint(first);
  const b = fingerprint(current);
  if (a === b) return null;

  return new EnvironmentError(
    `Seeded guidance changed between rep 1 and rep ${rep} — the reps did not measure ` +
      `the same system, so their scores cannot be pooled.\n` +
      `  rep 1:   ${a}\n` +
      `  rep ${rep}: ${b}`,
    `Reps must run against identical guidance. If a between-run hook rebuilds or ` +
      `reseeds the daemon, make sure it restores the SAME commit's content every ` +
      `time.\n  No results file was written for this run.`,
  );
}

/** One line of the raw-output JSONL trace. */
export interface TraceLine {
  /** 1-based rep this turn came from. */
  rep: number;
  scenarioId: string;
  turnIndex: number;
  isPriorContext: boolean;
  rawOutput: string;
  routingDecision?: string;
  stage2CandidatesInjected?: boolean;
  toolsCalled: string[];
}

/**
 * Build the raw-output trace lines for one rep's results.
 *
 * Only turns that actually captured `rawOutput` produce a line — a turn run
 * against a daemon without `RUST_LOG=debug` simply contributes nothing,
 * rather than a null placeholder. Each line carries its `rep` so a multi-rep
 * trace answers "what did the model say the time it failed", which is the
 * whole point of keeping the reps that disagree.
 */
export function buildTraceLines(
  results: ScenarioResult[],
  rep = 1,
): TraceLine[] {
  const lines: TraceLine[] = [];
  for (const r of results) {
    for (const [idx, t] of r.turns.entries()) {
      if (t.rawOutput === undefined) continue;
      lines.push({
        rep,
        scenarioId: r.id,
        turnIndex: idx,
        isPriorContext: idx < r.turns.length - 1,
        rawOutput: t.rawOutput,
        routingDecision: t.routingDecision,
        stage2CandidatesInjected: t.stage2CandidatesInjected,
        toolsCalled: t.toolsCalled,
      });
    }
  }
  return lines;
}

/**
 * Render the per-scenario reliability table.
 *
 * Split out as a pure string[] builder so the rendering that carries the
 * finding — "fails 3/3" versus "fails 1/3", the distinction a single run
 * cannot make — is asserted directly in a unit test rather than only by
 * eyeballing terminal output.
 */
export function formatReliabilityTable(aggregate: RunAggregate): string[] {
  const single = aggregate.reps === 1;
  return aggregate.scenarios.map((s) => {
    // Setup before the exclusion branch: a setup scenario also has
    // `scoredReps === 0`, and reporting it as a degenerate empty generation
    // would name an inference bug that did not happen.
    if (s.setup) {
      return `  ⊙ ${s.id}  fixture setup — not scored`;
    }
    if (s.scoredReps === 0) {
      return single
        ? `  ⊘ ${s.id}  excluded (degenerate empty generation) — never scored`
        : `  ⊘ ${s.id}  excluded in all ${s.excludedReps} rep(s) — never scored`;
    }
    const marker = s.passedAll ? "✓" : s.flipped ? "~" : "✗";
    const tally = single
      ? s.passedAll
        ? "pass"
        : "fail"
      : `${s.passedReps}/${s.scoredReps} reps`;
    const excluded =
      s.excludedReps > 0 ? ` · ${s.excludedReps} excluded` : "";
    const flag = s.flipped ? "  ← FLIPPED" : "";
    return `  ${marker} ${s.id}  ${tally}${excluded}${flag}`;
  });
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

function usage(fixture: EvalFixture): string {
  return (
    `usage: bun run scripts/eval/${fixture.name}.ts <label> [out.json] [--runs N]\n` +
    `       [--between-runs <cmd>] [--baseline <path>]\n\n` +
    `  ${fixture.description}\n\n` +
    `  label          tag recorded in the results (e.g. 'e4b')\n` +
    `  out.json       where to write results (default: /tmp/${fixture.name}-<label>-<ts>.json)\n` +
    `  --runs         repetitions of the whole fixture (default 1). The headline\n` +
    `                 becomes pass^k: a scenario counts only if it passed in every\n` +
    `                 rep. A rep costs a full run's wall clock.\n` +
    `  --between-runs shell command run between reps (not before the first, not\n` +
    `                 after the last) — purge the database and restart the daemon\n` +
    `                 here. Reps run against the same daemon otherwise, and the\n` +
    `                 runner aborts if guidance drifts between them.\n` +
    `  --baseline     compare against a recorded run and fail on regression\n\n` +
    ENV_USAGE +
    `\n\nExit codes: 0 all passed · 1 scenario failure/regression · 2 environment unusable · 64 usage`
  );
}

/** Pull `--flag <value>` out of argv, or `undefined`. Exits on a missing value. */
function takeFlag(
  argv: string[],
  flag: string,
  fixture: EvalFixture,
): string | undefined {
  const i = argv.indexOf(flag);
  if (i === -1) return undefined;
  const value = argv[i + 1];
  if (value === undefined) {
    console.error(`${flag} needs a value\n\n${usage(fixture)}`);
    process.exit(EXIT_USAGE);
  }
  argv.splice(i, 2);
  return value;
}

/** Read the daemon status and run preflight, aborting on an environment failure. */
function gate(fixture: EvalFixture, env: EvalEnv): DaemonStatus {
  try {
    const s = readDaemonStatus(env);
    preflight(env, s);
    // Runs after the checks above and before any scenario: embeddings are
    // generated on a ~30s debounce, so a daemon freshly started against a
    // purged database — which is exactly what the documented `--between-runs`
    // command produces before every rep — serves its first turns with an EMPTY
    // skill index. Those turns fail open to the full tool surface with no skill
    // guidance, and the resulting malformed write cascades through the rest of
    // its group. Waiting here costs seconds once; not waiting silently reds out
    // whichever group happens to run first.
    awaitSkillIndex(env);
    return s;
  } catch (e) {
    if (e instanceof EnvironmentError) abortOnEnvironment(fixture.name, e);
    throw e;
  }
}

/**
 * Run every scenario in the fixture once.
 *
 * Extracted from `runEval` so a rep is a callable unit rather than the body of
 * the entry point — which is what lets `--runs` repeat the identical code path
 * instead of a second, separately-drifting copy of the scenario loop.
 * Throws `EnvironmentError` if the daemon dies mid-rep; the caller decides
 * whether that voids the whole run (it does — see `runEval`).
 */
function runRep(fixture: EvalFixture, env: EvalEnv): ScenarioResult[] {
  const results: ScenarioResult[] = [];

  /**
   * Consecutive failed sends tolerated before the run is declared void.
   *
   * Preflight proves the environment was usable at t=0; it cannot prove it
   * stayed usable across the minutes a run takes. If the daemon dies mid-run
   * every subsequent turn calls no tools, and every negative assertion scores
   * as a PASS — the exact false result this harness exists to prevent, just
   * relocated past the gate. One failure can be a flaky turn and is a real
   * scenario failure; two in a row is the environment.
   */
  const MAX_CONSECUTIVE_SEND_FAILURES = 2;
  let consecutiveSendFailures = 0;

  /**
   * Run a turn and count it against the consecutive-failure budget.
   *
   * Every turn goes through here, prior-context turns included. A guard on only
   * the scored turn would leave the identical hole one call site over: a prior
   * turn that never reached the model leaves its scenario running against a
   * chat that never received its setup, and the resulting verdict gets filed as
   * the model's behavior rather than as an environment artifact.
   */
  const turn = (chatId: string, message: string): TurnRecord => {
    const t = runTurn(env, chatId, message);
    if (!t.sendFailed) {
      consecutiveSendFailures = 0;
      return t;
    }
    consecutiveSendFailures++;
    if (consecutiveSendFailures >= MAX_CONSECUTIVE_SEND_FAILURES) {
      throw new EnvironmentError(
        `${consecutiveSendFailures} turns in a row failed to send — the daemon ` +
          `went away partway through the run.\n  Last error: ${t.reply}`,
        `Check whether the daemon on ${env.socket} is still alive (it may have ` +
          `run out of memory or been killed).\n  ${results.length} scenario(s) had ` +
          `been scored; they are NOT written, because scores from a run that died ` +
          `midway are not comparable to a complete one — and turns that never ` +
          `reached the model would score "no tools called" assertions as passes.`,
      );
    }
    return t;
  };

  for (const group of fixture.groups) {
    // Set when a `setup: true` turn fails its own assertion. Every later
    // scenario in the group then depends on state that was never established,
    // so scoring them measures the fixture, not the model — see the exclusion
    // where this is read.
    let setupFailed: string | null = null;
    const chatId = newChat(env);
    console.error(
      `[${fixture.name}] chat ${chatId} for: ${group.map((s) => s.id).join(", ")}`,
    );

    // Establish any graph state this group's scenarios refer to but do not
    // create. Runs AFTER the chat node exists (so a seed may reference it) and
    // BEFORE the first turn, so the seeded state is already in place when the
    // pre-turn snapshot is taken — seeded nodes therefore appear in `before`
    // and never register as this turn's `createdNode`, which is what lets a
    // scenario assert `updatedNode` against them.
    //
    // A failure here is an ENVIRONMENT failure, not a scenario failure, and is
    // rethrown as one: a seed that did not run leaves every scenario in the
    // group referring to something absent, which would score as a string of
    // model failures rather than the setup fault it is.
    if (fixture.seedGroup) {
      try {
        fixture.seedGroup(env, group);
      } catch (e) {
        throw new EnvironmentError(
          `Group seeding failed for: ${group.map((s) => s.id).join(", ")}\n  ` +
            `${e instanceof Error ? e.message : String(e)}`,
          `Check whether the daemon on ${env.socket} is reachable and the fixture's ` +
            `seedGroup logic matches the current schema — a failed seed leaves every ` +
            `scenario in this group referring to nodes that were never created.`,
        );
      }
    }

    /**
     * Nodes this group has created so far — the only nodes whose edges are
     * worth walking when snapshotting (see the bracketing comment below).
     *
     * Per group rather than per run: a scenario can only link nodes its own
     * chat established, and letting this accumulate across groups would
     * restore the whole-database walk this exists to avoid.
     */
    const groupNodeIds = new Set<string>();

    for (const scenario of group) {
      // A setup turn earlier in this group failed to establish its state, so
      // this scenario's assertion is unreachable through no fault of the model.
      // Excluded on the same grounds as an unrouted turn: the harness has
      // already declared the precondition missing, and scoring it anyway
      // produces a red that reads as model behaviour. Setup turns themselves
      // still run — they are already unscored, and a later one may re-establish
      // enough state to make the failure legible in the log.
      if (setupFailed !== null && scenario.setup !== true) {
        results.push({
          id: scenario.id,
          scenario: scenario.scenario,
          prompt: scenario.prompt,
          passed: false,
          turns: [],
          excludedAsSetupFailed: true,
        });
        console.error(
          `[${fixture.name}] → ${scenario.scenario}\n` +
            `[${fixture.name}]   ⊗ excluded (setup ${setupFailed} did not establish its state)`,
        );
        continue;
      }
      console.error(`[${fixture.name}] → ${scenario.scenario}`);

      // Prior turns establish context and are never scored.
      const priorTurns: TurnRecord[] = [];
      for (const prior of scenario.priorTurns ?? []) {
        console.error(`[${fixture.name}]   [context] ${prior}`);
        priorTurns.push(turn(chatId, prior));
      }

      // Snapshot bracketing the scored turn. Captured only when the fixture
      // opts into graph grading, and taken as late/early as possible around
      // the turn so the diff attributes to THIS turn rather than to anything
      // the harness did between scenarios.
      //
      // Edge discovery is the expensive half: the daemon exposes
      // relationships per (node, relation) pair rather than as a whole-graph
      // dump, so walking every node costs a round-trip per node per relation
      // — ~11s on a 120-node database, and it would be paid twice per turn.
      //
      // Both snapshots therefore walk only the nodes THIS GROUP created,
      // which is where every edge a scenario can assert must land: a turn
      // can only link nodes its own chat established (11c links what 11a and
      // 11b created). The restriction cannot instead be "walk what changed",
      // because `create_relationship` leaves both endpoints byte-identical —
      // verified against the daemon, same version and same modified_at — so
      // that rule would find no edges at all and score 11c, the one scenario
      // that measures linking, as a false pass.
      const before = fixture.graph
        ? captureSnapshot(env, fixture.graph.types, {
            edgesFor: (n) => groupNodeIds.has(n.id),
          })
        : undefined;

      const scored = turn(chatId, scenario.prompt);

      // Nodes created by THIS turn join the candidate set before its edges
      // are walked: a turn that creates a node and links it in the same
      // breath is legitimate, and walking only the pre-turn set would miss
      // that edge. Node enumeration is cheap (one query per type); it is the
      // per-node relationship walk that is not, so the "after" pass runs in
      // two steps rather than snapshotting a third time.
      //
      // This makes the walk ASYMMETRIC — the "after" set is a strict superset
      // — and the consequence is worth stating: a node new since `before` had
      // no edges walked there, so every edge on it appears in `addedEdges`
      // whether this turn recorded it or the daemon materialized it at
      // creation time. `EdgeExpectation` therefore requires a named relation,
      // enforced by a fixture invariant; an unpinned one would pass on any
      // turn that merely created a node. Restricting the diff instead would
      // be the alternative, but it would lose the create-and-link-in-one-turn
      // case above, which is a legitimate shape.
      const after = fixture.graph
        ? captureSnapshot(env, fixture.graph.types, {
            edgesFor: (n) =>
              groupNodeIds.has(n.id) ||
              !before?.nodes.some((p) => p.id === n.id),
          })
        : undefined;
      for (const n of after?.nodes ?? []) {
        if (!before?.nodes.some((p) => p.id === n.id)) groupNodeIds.add(n.id);
      }
      const diff = before && after ? diffSnapshots(before, after) : undefined;

      // The degenerate-empty-generation failure mode (agent_loop.rs: the
      // model opens a turn and emits neither text nor a tool call) is an
      // inference bug, not a scenario outcome — scoring it as a failure
      // silently deflates every cell it lands in, exactly the harness-vs-model
      // confusion this eval exists to prevent. Excluded from the denominator
      // rather than scored, but still recorded with its turn data so the rate
      // of empty generations stays visible rather than vanishing silently.
      if (scored.emptyGeneration) {
        results.push({
          id: scenario.id,
          scenario: scenario.scenario,
          prompt: scenario.prompt,
          passed: false,
          failure: "excluded: degenerate empty generation (no text, no tool call)",
          turns: [...priorTurns, scored],
          extra: fixture.extra?.(scenario, [scored]),
          excludedAsEmptyGeneration: true,
        });
        console.error(
          `[${fixture.name}]   ⊘ excluded (empty generation) ${scored.latencyMs}ms`,
        );
        continue;
      }

      // Stage-2 scopes each turn's tool surface to the retrieved skills'
      // whitelists, so a retrieval miss can remove the tool a scenario needs.
      // The turn then cannot make the graph change either, so BOTH the
      // trajectory and outcome verdicts red out — for a turn the model had no
      // way to complete. Observed live: "Put down that we went with
      // event-based cache clearing, Priya's call" retrieved Node Deletion, so
      // the surface carried delete/search tools and no create_node.
      //
      // Excluded on the same grounds as a degenerate empty generation: the
      // assertion was unreachable, so neither verdict is a statement about the
      // model. Still recorded, so the rate stays visible.
      const required = fixture.requiredTools?.(scenario) ?? [];
      const offeredTools = scored.toolsOffered
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean);
      const missingTools = required.filter((t) => !offeredTools.includes(t));
      if (missingTools.length > 0 && offeredTools.length > 0) {
        results.push({
          id: scenario.id,
          scenario: scenario.scenario,
          prompt: scenario.prompt,
          passed: false,
          failure:
            `excluded: asserted tool(s) ${missingTools.join(", ")} were never ` +
            `offered this turn (Stage-2 routing scoped to: ${offeredTools.join(", ")})`,
          turns: [...priorTurns, scored],
          extra: fixture.extra?.(scenario, [scored]),
          excludedAsToolNotOffered: true,
        });
        console.error(
          `[${fixture.name}]   ⊗ excluded (tool not offered: ${missingTools.join(", ")}) ${scored.latencyMs}ms`,
        );
        continue;
      }

      // The trajectory assertions still run, but they no longer decide the
      // score when the fixture grades on outcome — they are recorded as a
      // diagnostic. Trajectory answers "how did the model get there", which
      // is what a debugging session needs and what an outcome score cannot
      // say; it just is not the thing being graded.
      const trajectory = fixture.score(scenario, [scored]);
      const verdict =
        fixture.graph && diff
          ? fixture.graph.scoreOutcome(scenario, diff, [scored])
          : trajectory;

      results.push({
        id: scenario.id,
        scenario: scenario.scenario,
        prompt: scenario.prompt,
        passed: verdict.passed,
        failure: verdict.failure,
        turns: [...priorTurns, scored],
        extra: fixture.extra?.(scenario, [scored]),
        graphDiff: diff,
        trajectory: fixture.graph ? trajectory : undefined,
        excludedAsSetup: scenario.setup === true ? true : undefined,
      });

      const marker = markerFor({
        excludedAsSetup: scenario.setup === true,
        passed: verdict.passed,
      });

      console.error(
        `[${fixture.name}]   ${marker} ` +
          `tools=[${scored.toolsCalled.join(",")}] ${scored.latencyMs}ms` +
          (scenario.setup ? " (setup — not scored)" : ""),
      );
      if (!verdict.passed)
        console.error(`[${fixture.name}]     ↳ ${verdict.failure}`);
      // A setup turn that failed to establish its state makes every scenario
      // after it unwinnable. It is not scored, but it must not pass silently.
      if (scenario.setup && !verdict.passed) {
        console.error(
          `[${fixture.name}]     ⚠ setup did not establish its state — ` +
            `later scenarios in this group are excluded, not scored`,
        );
        setupFailed ??= scenario.id;
      }
    }
  }

  return results;
}

/**
 * Run the operator's between-rep command.
 *
 * The harness deliberately does NOT purge the database or restart the daemon
 * itself: the caller owns the daemon, socket, and database everywhere else in
 * this harness, and a runner that starts killing processes it did not start
 * would own a lifecycle it cannot see (a daemon under a launch agent, a
 * remote socket, a model that takes a minute to load). Instead the operator
 * supplies the command, and the runner verifies afterwards — via the guidance
 * readback that already exists — that whatever it did left the reps
 * comparable. Assert, do not own.
 */
function runBetween(fixture: EvalFixture, cmd: string, rep: number): void {
  console.error(`[${fixture.name}] between reps ${rep - 1}→${rep}: ${cmd}`);
  const r = Bun.spawnSync(["sh", "-c", cmd], {
    stdout: "inherit",
    stderr: "inherit",
    env: { ...process.env },
  });
  if (r.exitCode !== 0) {
    abortOnEnvironment(
      fixture.name,
      new EnvironmentError(
        `The --between-runs command exited ${r.exitCode} before rep ${rep}.`,
        `Reps that run against an environment the hook failed to reset are not ` +
          `comparable to the ones before it, so the run stops here rather than ` +
          `pooling them.\n  No results file was written for this run.`,
      ),
    );
  }
}

/**
 * Run an eval end to end. Call this from a fixture's CLI wrapper; it owns the
 * process lifetime and exits rather than returning.
 */
export async function runEval(fixture: EvalFixture): Promise<never> {
  const argv = process.argv.slice(2);

  const baselinePath = takeFlag(argv, "--baseline", fixture);
  const betweenRuns = takeFlag(argv, "--between-runs", fixture);
  const runsArg = takeFlag(argv, "--runs", fixture);

  let runs = 1;
  if (runsArg !== undefined) {
    runs = Number(runsArg);
    if (!Number.isInteger(runs) || runs < 1) {
      console.error(
        `--runs needs a positive integer (got ${JSON.stringify(runsArg)})\n\n${usage(fixture)}`,
      );
      process.exit(EXIT_USAGE);
    }
  }
  if (betweenRuns !== undefined && runs === 1) {
    console.error(
      `--between-runs has no effect with a single rep — it runs BETWEEN reps.\n\n${usage(fixture)}`,
    );
    process.exit(EXIT_USAGE);
  }

  const [label, outPathArg] = argv;
  if (!label) {
    console.error(usage(fixture));
    process.exit(EXIT_USAGE);
  }

  const env = readEnv();

  // Preflight BEFORE any scenario runs. An environment failure must never be
  // reported as scenario results, so this precedes both the run and the file.
  // Re-run before every rep: a between-runs hook restarts the daemon, and rep
  // 2 onward would otherwise score against whatever came back up — including
  // a different model, or a smaller granted window.
  const status = gate(fixture, env);

  const ts = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const outPath = outPathArg ?? `/tmp/${fixture.name}-${label}-${ts}.json`;

  const provenanceFor = (s: DaemonStatus): Provenance => {
    const { commit, dirty } = gitCommit();
    return {
      model: s.modelId,
      recordedAt: new Date().toISOString(),
      hostMemoryGb: Number((s.hostRamBytes / 1e9).toFixed(1)),
      nCtx: s.grantedNCtx,
      ...(s.modelMatchedByPath ? { modelMatchedByPath: true } : {}),
      evalCommit: commit,
      dirty,
      guidance: readGuidanceProvenance(env),
    };
  };

  const firstProvenance = provenanceFor(status);

  console.error(
    `[${fixture.name}] label=${label} model=${status.modelId} n_ctx=${status.grantedNCtx}` +
      (firstProvenance.dirty ? " (working tree dirty)" : "") +
      (runs > 1 ? ` runs=${runs}` : ""),
  );
  for (const [nodeType, entries] of Object.entries(
    firstProvenance.guidance ?? {},
  )) {
    console.error(
      `[${fixture.name}] guidance seeded (${nodeType}): ${
        entries.length === 0
          ? "none found — cannot confirm what this run measured"
          : entries.map((e) => `${e.key}@${e.version}`).join(", ")
      }`,
    );
  }
  if (runs > 1 && betweenRuns === undefined) {
    console.error(
      `[${fixture.name}] note: no --between-runs command — reps share one daemon and ` +
        `one database, so each rep starts from the state the previous one left. Pass ` +
        `--between-runs to purge and restart between them.`,
    );
  }

  // -------------------------------------------------------------------------
  // Run
  // -------------------------------------------------------------------------

  const reps: RepResult[] = [];

  try {
    for (let rep = 1; rep <= runs; rep++) {
      // Rep 1 reuses the provenance already read for the header; every later
      // rep re-reads it, because a between-runs hook that restarted the daemon
      // can bring back a different model or a smaller context window, and
      // pooling that rep's scores with the earlier ones would file them all
      // under rep 1's conditions.
      let provenance = firstProvenance;
      if (rep > 1) {
        if (betweenRuns !== undefined) runBetween(fixture, betweenRuns, rep);
        provenance = provenanceFor(gate(fixture, env));
        const drift = checkGuidanceDrift(
          firstProvenance.guidance,
          provenance.guidance,
          rep,
        );
        if (drift) abortOnEnvironment(fixture.name, drift);
      }
      if (runs > 1) console.error(`[${fixture.name}] ── rep ${rep}/${runs} ──`);

      const results = runRep(fixture, env);
      const {
        scored,
        excludedCount,
        setupCount,
        toolNotOfferedCount,
        setupFailedCount,
      } = partitionExcluded(results);
      const passed = scored.filter((r) => r.passed).length;

      // Where the retired trajectory assertions disagree with the outcome
      // score. Not a failure count — each disagreement is either a scenario
      // whose expectation no longer describes the behavior, or a real change
      // in how the model reaches its result. Both are worth a look; neither is
      // a verdict.
      const trajectoryDisagreements = scored.filter(
        (r) => r.trajectory !== undefined && r.trajectory.passed !== r.passed,
      ).length;

      // Uniform 0% or 100% across every SCORED scenario is a harness signature —
      // an environment that preflight could not catch (e.g. every send silently
      // routing to a dead model behind a load balancer, or every turn hitting the
      // same unhandled code path) rather than a real result. Checked per rep
      // rather than on the pooled scores: a single impossible rep is exactly as
      // much a harness signature as a single impossible run, and averaging it
      // into the others is how it would stop being visible.
      const distinctToolsCalled = new Set(
        scored.flatMap((r) => (r.turns ?? []).flatMap((t) => t.toolsCalled ?? [])),
      ).size;
      const uniformityError = checkUniformity(
        passed,
        scored.length,
        undefined,
        distinctToolsCalled,
      );
      if (uniformityError) abortOnEnvironment(fixture.name, uniformityError);

      reps.push({
        rep,
        provenance,
        summary: {
          total: scored.length,
          passed,
          failed: scored.length - passed,
          excludedEmptyGenerations: excludedCount,
          excludedSetup: setupCount,
          excludedToolNotOffered: toolNotOfferedCount,
          excludedSetupFailed: setupFailedCount,
          trajectoryDisagreements,
        },
        results,
      });
    }
  } catch (e) {
    if (e instanceof EnvironmentError) abortOnEnvironment(fixture.name, e);
    throw e;
  }

  // -------------------------------------------------------------------------
  // Report
  // -------------------------------------------------------------------------

  const aggregate = aggregateReps(reps.map((r) => r.results));

  const evalResults: EvalResults = {
    eval: fixture.name,
    label,
    provenance: firstProvenance,
    aggregate,
    reps,
  };

  await Bun.write(outPath, JSON.stringify(evalResults, null, 2));
  console.error(
    `[${fixture.name}] wrote ${aggregate.scoredScenarios} scenario(s) × ${runs} rep(s) to ${outPath}`,
  );

  // Raw-output JSONL trace: one line per scored turn that captured raw
  // generation text, alongside the results JSON so a scenario that needs
  // investigating never requires a re-run to see what the model actually
  // said. Absent turns (no RUST_LOG=debug on the daemon) are simply skipped
  // rather than padded with nulls.
  const tracePath = outPath.replace(/\.json$/, ".trace.jsonl");
  const traceLines = reps
    .flatMap((r) => buildTraceLines(r.results, r.rep))
    .map((l) => JSON.stringify(l));
  if (traceLines.length > 0) {
    await Bun.write(tracePath, traceLines.join("\n") + "\n");
    console.error(
      `[${fixture.name}] wrote ${traceLines.length} raw-output trace line(s) to ${tracePath}`,
    );
  } else {
    console.error(
      `[${fixture.name}] no raw-output trace written — daemon was not run with RUST_LOG=debug ` +
        `(or an equivalent filter including "nodespace_agent" at debug level)`,
    );
  }

  console.log(`\n── ${fixture.description} ─────────────────────────────────`);
  console.log(`   Label:    ${label}`);
  console.log(`   Model:    ${firstProvenance.model}`);
  console.log(
    `   Context:  n_ctx ${firstProvenance.nCtx} · host RAM ${firstProvenance.hostMemoryGb} GB`,
  );
  console.log(
    `   Commit:   ${firstProvenance.evalCommit}${firstProvenance.dirty ? " (dirty)" : ""}`,
  );
  console.log(`   Reps:     ${aggregate.reps}`);
  // pass^k first, deliberately: it is the number to cite. pass^1 is printed
  // right under it because it is the number every earlier run quoted, and the
  // distance between them is the finding. At k=1 the two are the same number
  // by definition, so only one line is printed — and it is labelled pass^1,
  // since a lone "pass^1" is exactly the single draw this flag exists to stop
  // people citing as a reliability figure.
  if (aggregate.reps === 1) {
    console.log(
      `   pass^1:   ${aggregate.passAtK}/${aggregate.scoredScenarios}` +
        `  (single rep — pass a --runs N above 1 for a reliability figure)`,
    );
  } else {
    console.log(
      `   pass^${aggregate.reps}:   ${aggregate.passAtK}/${aggregate.scoredScenarios}` +
        `  (passed in every rep)`,
    );
    // pass^1's denominator is the mean number of scenarios SCORED per rep,
    // which is below `scoredScenarios` whenever exclusions were uneven across
    // reps. Printing it over `scoredScenarios` would render e.g. "2.50/3"
    // beside "pass^3: 3/3" — both correct, but inviting the reader to take the
    // pair as a rate over one population when the two are computed over
    // different ones.
    const meanScored =
      reps.reduce((n, r) => n + r.summary.total, 0) / reps.length;
    console.log(
      `   pass^1:   ${aggregate.passAt1.toFixed(2)}/${
        Number.isInteger(meanScored) ? meanScored : meanScored.toFixed(2)
      }  (mean of per-rep scores)`,
    );
  }
  if (aggregate.reps > 1) {
    console.log(
      `   Flipped:  ${aggregate.flipped}/${aggregate.scoredScenarios}` +
        `  (passed in some reps, failed in others)`,
    );
  }
  const excluded = reps.reduce(
    (n, r) => n + (r.summary.excludedEmptyGenerations ?? 0),
    0,
  );
  if (excluded > 0) {
    console.log(
      `   Excluded: ${excluded} scenario-rep(s) (degenerate empty generation — not scored either way)`,
    );
  }
  // Summed across reps for the same reason `excluded` is: these are per-rep
  // counts, and a run's headline should not silently report only its first.
  const setupExcluded = reps.reduce(
    (n, r) => n + (r.summary.excludedSetup ?? 0),
    0,
  );
  if (setupExcluded > 0) {
    console.log(
      `   Setup:    ${setupExcluded} scenario-rep(s) (fixture setup — not scored)`,
    );
  }
  // Reported separately from `excluded`: both leave the scored set, but an
  // empty generation is an inference bug and this is a ROUTING miss. A reader
  // seeing a shrunken denominator needs to know which one they are looking at,
  // and #2240/#2254 are the reason the distinction is worth a line.
  const toolNotOffered = reps.reduce(
    (n, r) => n + (r.summary.excludedToolNotOffered ?? 0),
    0,
  );
  if (toolNotOffered > 0) {
    console.log(
      `   Unrouted: ${toolNotOffered} scenario-rep(s) (asserted tool never offered — not scored)`,
    );
  }
  // Reported separately again, for the same reason: this names the FIXTURE's
  // own precondition as the cause, which is neither an inference bug nor a
  // routing miss. Without its own line the denominator shrinks with nothing on
  // screen to explain it.
  const setupFailed = reps.reduce(
    (n, r) => n + (r.summary.excludedSetupFailed ?? 0),
    0,
  );
  if (setupFailed > 0) {
    console.log(
      `   Blocked:  ${setupFailed} scenario-rep(s) (group setup failed — not scored)`,
    );
  }
  const disagreements = reps.reduce(
    (n, r) => n + (r.summary.trajectoryDisagreements ?? 0),
    0,
  );
  if (disagreements > 0) {
    console.log(
      `   Trajectory disagrees on ${disagreements} scenario-rep(s) — ` +
        `diagnostic only, see 'trajectory' in the results file`,
    );
  }
  if (aggregate.reps > 1) {
    console.log(
      `   Per rep:  ${reps.map((r) => `${r.summary.passed}/${r.summary.total}`).join("  ")}`,
    );
  }
  // Fixture summaries are per-rep constructs (they read a ScenarioResult[]),
  // so they are rendered per rep rather than against a pooled list that would
  // double-count every scenario.
  for (const r of reps) {
    const lines = fixture.summary?.(r.results) ?? [];
    for (const line of lines) {
      console.log(`   ${aggregate.reps > 1 ? `[rep ${r.rep}] ` : ""}${line}`);
    }
  }
  console.log(
    `────────────────────────────────────────────────────────────────────`,
  );
  for (const line of formatReliabilityTable(aggregate)) {
    console.log(line);
  }
  console.log(
    `────────────────────────────────────────────────────────────────────\n`,
  );

  let regressions = 0;
  if (baselinePath) {
    regressions = await compareToBaseline(fixture.name, aggregate, baselinePath);
  }

  // The exit code follows pass^k, not the last rep: a suite that passes
  // everything twice and fails one scenario the third time has not passed.
  const notPassing = aggregate.scoredScenarios - aggregate.passAtK;
  if (notPassing > 0 || regressions > 0) {
    console.error(
      `\n[${fixture.name}] ✗ ${notPassing}/${aggregate.scoredScenarios} scenarios did not pass ` +
        `every rep` +
        (aggregate.flipped > 0 ? ` (${aggregate.flipped} flipped)` : ""),
    );
    process.exit(EXIT_FAILED);
  }
  console.error(
    `\n[${fixture.name}] ✓ All ${aggregate.scoredScenarios} scenarios passed all ${aggregate.reps} rep(s)`,
  );
  process.exit(0);
}
