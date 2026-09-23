/**
 * Skill-routing eval — routing accuracy through the two-gate pipeline (ADR-036).
 *
 *   Stage 1: model picks one of route_query / route_clarify / route_multi
 *   Stage 2: model judges whether the retrieved skill fits the intent
 *
 * Coverage:
 *   - Direct intent → correct skill
 *   - Indirect phrasing → correct skill (the load-bearing assumption)
 *   - Ambiguous → expect clarification, not a guess
 *   - Existing-type instance vs new-type (Node Creation, not Schema Creation)
 *   - General / search → Research & Search skill
 *   - Out-of-scope → decline without asking the user to disambiguate
 *   - Clarification contract: one clarification, then fall through (not a loop)
 *   - Mutating-skill gate: borderline schema-creation gated harder than read-only
 *   - Compound request → route_multi; single intent phrased at length → NOT
 *     route_multi (what route_multi's guard clauses defend against)
 *
 * Every scenario not expecting route_multi fails if Stage 1 routes multi, so a
 * regression that splits single intents cannot pass on downstream effects alone.
 *
 * CONSTANT-ANSWER BASELINE. Mapped to the Stage-1 decision each expectation
 * implies (skill/search → route_query), the distribution is 13 query / 3
 * clarify / 2 multi, with `decline` unasserted at Stage 1. Always answering
 * route_query therefore scores 13/18 = 72.2%; quote any Stage-1 accuracy figure
 * from this fixture against that, not against 0%. Pinned against the scenario
 * list by fixtures/routing.test.ts, and printed with every run's summary.
 *
 * Scenario wording must stay independent of packages/agent/src/agent_guidance.rs;
 * `guidance_is_not_contaminated_by_eval_prompts` parses the `prompt:` literals
 * out of this file and fails the build if guidance reproduces one.
 */

import type { EvalFixture, Scenario, TurnRecord, Verdict } from "../types.ts";

// ---------------------------------------------------------------------------
// Expectation model
// ---------------------------------------------------------------------------

export type ExpectedOutcome =
  | { kind: "skill"; skill: string } // skill name (substring match, case-insensitive)
  | { kind: "clarify" } // a clarification question, no tool action
  | { kind: "search" } // the Research & Search / general search skill
  | { kind: "multi" } // Stage 1 split a compound request via route_multi
  // The request is clear but nothing can answer it: the turn answers (a
  // refusal or an empty result) without asking the user to disambiguate and
  // without mutating. Stage 1's tool choice is deliberately NOT asserted.
  | { kind: "decline" };

/** The Stage-1 decision an expectation implies, or null when it asserts none. */
export type Stage1Decision = "query" | "clarify" | "multi";

export function stage1Expectation(e: ExpectedOutcome): Stage1Decision | null {
  switch (e.kind) {
    // Skill-vs-search is Stage 2's call; Stage 1 only decides describability.
    case "skill":
    case "search":
      return "query";
    case "clarify":
      return "clarify";
    case "multi":
      return "multi";
    case "decline":
      return null;
  }
}

export interface RoutingScenario extends Scenario {
  expected: ExpectedOutcome;
  /**
   * A single intent phrased at length — several details, clauses, or "and"s —
   * which must NOT route multi. The negative case route_multi's guard clauses
   * ("not one thing phrased at length") exist for.
   */
  singleIntentAtLength?: boolean;
  /** Covers a "thin evidence" area the ADR calls out explicitly. */
  loadBearing?: boolean;
  /** Mutating skill: the bar is higher (schema-create vs search). */
  mutating?: boolean;
  /** A message that must NOT reach a mutating skill. */
  adversarial?: boolean;
}

// ---------------------------------------------------------------------------
// Scenarios
//
// Each runs in its own chat node. `id` is the baseline join key and must stay
// stable; prompts may be reworded freely.
// ---------------------------------------------------------------------------

const FIXTURES: RoutingScenario[] = [
  // ── Direct intent → correct skill ────────────────────────────────────────
  {
    id: "direct-schema-create",
    scenario: "Direct: create schema (explicit phrasing)",
    prompt: "Create an invoice tracking database",
    expected: { kind: "skill", skill: "Schema Creation" },
  },
  {
    id: "direct-node-search",
    scenario: "Direct: search (explicit phrasing)",
    prompt: "Search my notes for anything about embeddings",
    expected: { kind: "search" },
  },
  {
    id: "direct-node-create",
    scenario: "Direct: create instance (existing type)",
    prompt: "Create a new task called 'Review Q3 report'",
    expected: { kind: "skill", skill: "Node Creation" },
  },

  // ── Indirect phrasing → correct skill (LOAD-BEARING) ─────────────────────
  {
    id: "indirect-schema-money-owed",
    scenario: "Indirect: 'keep tabs on who owes me money' → Schema Creation",
    prompt: "keep tabs on who owes me money",
    expected: { kind: "skill", skill: "Schema Creation" },
    loadBearing: true,
    mutating: true,
  },
  {
    id: "indirect-schema-freelance",
    scenario:
      "Indirect: 'start tracking my freelance projects' → Schema Creation",
    prompt: "start tracking my freelance projects",
    expected: { kind: "skill", skill: "Schema Creation" },
    loadBearing: true,
    mutating: true,
  },
  {
    id: "indirect-schema-expenses",
    scenario:
      "Indirect: 'I need a way to log my business expenses' → Schema Creation",
    prompt: "I need a way to log my business expenses",
    expected: { kind: "skill", skill: "Schema Creation" },
    loadBearing: true,
    mutating: true,
  },
  {
    id: "indirect-search-remember",
    scenario: "Indirect: 'what did I write about X?' → Research & Search",
    prompt: "what did I write about machine learning last month?",
    expected: { kind: "search" },
    loadBearing: true,
  },

  // ── Existing-type instance vs new-type (critical distinction) ────────────
  {
    id: "instance-not-schema-invoice",
    scenario:
      "Instance vs schema: 'add an invoice for $500' → Node Creation, not Schema Creation",
    // Context: invoice schema already exists (set up in prior turn)
    priorTurns: ["Create an invoice tracking database"],
    prompt: "Add an invoice for $500 due next Friday",
    expected: { kind: "skill", skill: "Node Creation" },
    loadBearing: true,
    adversarial: true, // should NOT route to Schema Creation
  },

  // ── Ambiguous → expect clarification ─────────────────────────────────────
  {
    id: "ambiguous-client-contacts",
    scenario:
      "Ambiguous: 'organize my client contacts' → clarify (schema vs collection)",
    prompt: "organize my client contacts",
    expected: { kind: "clarify" },
    loadBearing: true,
  },
  {
    id: "ambiguous-manage-projects",
    scenario:
      "Ambiguous: 'help me manage my projects' → clarify (schema vs search vs task)",
    prompt: "help me manage my projects",
    expected: { kind: "clarify" },
  },

  // ── General / search → Research & Search ─────────────────────────────────
  {
    id: "general-search-normal",
    scenario: "General search: a normal search outcome (not a failure)",
    prompt: "Find everything I have about the NodeSpace architecture",
    expected: { kind: "search" },
  },

  // ── Out-of-scope → decline, not clarify ──────────────────────────────────
  {
    id: "out-of-scope-weather",
    scenario:
      "Out of scope: weather query → decline, no clarification, no mutating tool",
    prompt: "What's the weather like in Tokyo today?",
    // The request is clear; NodeSpace just cannot answer it. That is not
    // ambiguity, so asking "did you mean X or Y?" is the failure here. ADR-038's
    // clarification contract already covers the path: a request that routes to
    // search and matches nothing is "a normal outcome, not a routing failure".
    // Stage 1 has no decline tool and needs none — whether it route_querys this
    // is a routing detail that may legitimately change, so it stays unasserted.
    expected: { kind: "decline" },
  },

  // ── Clarification contract: one clarification, then fall through ──────────
  {
    id: "clarification-then-fallthrough",
    scenario:
      "Clarification contract: after user clarifies, model proceeds (not a loop)",
    // Not an inconsistency with `ambiguous-manage-projects`: in a fresh chat
    // "just show me what I have" would be at least as vague. What is scored is
    // the third message of a conversation that already holds a clarification
    // and the user's answer to it. ADR-038 allows at most one clarification per
    // intent, so Stage 1 sees the answer in its blended context and a repeat
    // route_clarify is suppressed (`clarify_suppressed`) into retrieval. The
    // label tests that contract, not the prompt's standalone clarity.
    priorTurns: [
      "organize my client contacts",
      // Simulate user clarifying: they want to search existing contacts
      "I just want to search what I already have",
    ],
    prompt: "just show me what I have",
    expected: { kind: "search" },
    loadBearing: true,
  },

  // ── Mutating-skill gate: borderline schema vs read-only ───────────────────
  {
    id: "mutating-gate-borderline-schema",
    scenario:
      "Mutating gate: borderline schema request — gated harder than read-only",
    prompt: "maybe set up some kind of tracking for vendors",
    expected: { kind: "clarify" }, // borderline → must clarify before schema creation
    mutating: true,
    loadBearing: true,
    adversarial: true, // should NOT silently fire create_schema
  },
  {
    id: "readonly-gate-permissive",
    scenario: "Read-only gate: borderline search — lower bar, may proceed",
    prompt: "show me stuff about my customers",
    expected: { kind: "search" }, // read-only: proceed with search, don't block
  },

  // ── Compound request → route_multi ───────────────────────────────────────
  {
    id: "multi-task-and-search",
    scenario: "Compound: create a task AND search notes → route_multi",
    prompt:
      "Add a task to renew my passport, and also pull up my notes on the Lisbon trip",
    expected: { kind: "multi" },
  },
  {
    id: "multi-schema-and-search",
    scenario: "Compound: set up tracking AND search notes → route_multi",
    prompt:
      "Set up a way to track my vinyl records, then separately find what I wrote about turntables last year",
    expected: { kind: "multi" },
  },

  // ── Single intent phrased at length → NOT route_multi ────────────────────
  {
    id: "single-intent-long-task",
    scenario:
      "Single intent at length: one task with several details → Node Creation, not multi",
    prompt:
      "Create a task to call the dentist tomorrow morning about moving my cleaning appointment, make it high priority, and note on it that they close at noon on Fridays",
    expected: { kind: "skill", skill: "Node Creation" },
    singleIntentAtLength: true,
  },
  {
    id: "single-intent-long-search",
    scenario:
      "Single intent at length: one search naming several topics → search, not multi",
    prompt:
      "Find the notes I wrote after the team offsite, the ones covering the budget, the hiring plan, and the move to the new office",
    expected: { kind: "search" },
    singleIntentAtLength: true,
  },
];

/**
 * Share of Stage-1-asserting scenarios that always answering the majority
 * decision would get right — the floor any Stage-1 accuracy figure must beat.
 */
export function constantAnswerBaseline(scenarios: RoutingScenario[]): {
  decision: Stage1Decision;
  hits: number;
  total: number;
} {
  const counts = new Map<Stage1Decision, number>();
  for (const s of scenarios) {
    const d = stage1Expectation(s.expected);
    if (d) counts.set(d, (counts.get(d) ?? 0) + 1);
  }
  let decision: Stage1Decision = "query";
  let hits = 0;
  for (const [d, n] of counts) {
    if (n > hits) [decision, hits] = [d, n];
  }
  const total = [...counts.values()].reduce((a, b) => a + b, 0);
  return { decision, hits, total };
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

function isClarification(reply: string): boolean {
  const lower = reply.toLowerCase();
  // A clarification contains a question directed at the user about their intent.
  // Avoid false positives from rhetorical questions or confirmations.
  const hasQuestion = reply.includes("?");
  const hasIntentWords =
    lower.includes("did you") ||
    lower.includes("do you mean") ||
    lower.includes("would you like") ||
    lower.includes("are you looking") ||
    lower.includes("could you clarify") ||
    lower.includes("what would you like") ||
    lower.includes("which one") ||
    lower.includes("which would") ||
    lower.includes("which do") ||
    lower.includes("clarif");
  return hasQuestion && hasIntentWords;
}

function skillNameFromTurns(turns: TurnRecord[]): string | null {
  // The matched skill name appears in the reply or is implicit from subsequent
  // tool calls.
  // We parse from the reply text as a heuristic — the name appears in the model's
  // "I'll use the Schema Creation skill..." style phrasing.
  for (const t of turns) {
    const lower = t.reply.toLowerCase();
    for (const name of [
      "schema creation",
      "node creation",
      "research & search",
      "research and search",
      "graph editing",
      "relationship management",
      "node deletion",
      "bulk import",
      "organization",
    ]) {
      if (lower.includes(name.toLowerCase())) return name;
    }
  }
  return null;
}

// Under ADR-038's two-stage routing there is no model-visible retrieval call:
// Stage 1 emits route_query/route_clarify and the system performs retrieval
// itself. Skill routing is therefore observed through its effect — the action
// tool the matched skill permits — not through a retrieval call in the trace.

function calledMutatingTool(turns: TurnRecord[]): boolean {
  // All tools that write or mutate graph state — cross-referenced against Tool::ALL
  // in packages/agent/src/local_agent/tools.rs. Update here if the tool registry changes.
  const mutating = [
    "create_schema",
    "update_schema",
    "create_node",
    "update_node",
    "update_task_status",
    "delete_node",
    "create_relationship",
    "create_nodes_from_markdown",
  ];
  return turns.some((t) => t.toolsCalled.some((tc) => mutating.includes(tc)));
}

function calledSchemaCreate(turns: TurnRecord[]): boolean {
  return turns.some((t) => t.toolsCalled.includes("create_schema"));
}

/** Stage 1's recorded decision for the scored turn (the last one). */
function routingDecisionOf(turns: TurnRecord[]): string | undefined {
  return turns.at(-1)?.routingDecision;
}

/**
 * Stage 1 reached for route_multi — whether or not the call survived parsing.
 * `multi_rejected` (fewer than two usable queries) is the purest form of the
 * over-selection: a single intent the model tried to split anyway.
 */
function routedMulti(decision: string | undefined): boolean {
  return decision === "multi" || decision === "multi_rejected";
}

function askedToClarify(turns: TurnRecord[]): boolean {
  return (
    routingDecisionOf(turns) === "clarify" ||
    turns.some((t) => t.toolsCalled.includes("route_clarify"))
  );
}

export function assertFixture(
  fixture: RoutingScenario,
  turns: TurnRecord[],
): Verdict {
  const allReplies = turns.map((t) => t.reply).join("\n");
  const clarified = isClarification(allReplies);
  const decision = routingDecisionOf(turns);

  // Every scenario is either a compound request or a single intent, so
  // route_multi anywhere else is wrong regardless of what happened downstream
  // — retrieval on each split query can still land the right skill, which is
  // exactly how an over-splitting regression would otherwise pass.
  if (fixture.expected.kind !== "multi" && routedMulti(decision)) {
    return {
      passed: false,
      failure: `Stage 1 routed a single intent as multi (routing decision: ${decision})`,
    };
  }

  switch (fixture.expected.kind) {
    case "skill": {
      const expectedSkill = fixture.expected.skill.toLowerCase();
      const replyLower = allReplies.toLowerCase();
      // Model should have referenced or used the expected skill.
      if (!replyLower.includes(expectedSkill)) {
        // Also check if the right tools were called (e.g. create_schema for Schema Creation)
        const toolCheck =
          (expectedSkill.includes("schema creation") &&
            calledSchemaCreate(turns)) ||
          (expectedSkill.includes("node creation") &&
            turns.some((t) => t.toolsCalled.includes("create_node")));
        if (!toolCheck) {
          return {
            passed: false,
            failure: `Expected skill '${fixture.expected.skill}' but reply did not reference it and tools did not confirm. Reply: ${allReplies.slice(0, 300)}`,
          };
        }
      }
      // Adversarial: if this fixture is adversarial, the model must NOT fire the wrong mutating tool
      if (
        fixture.adversarial &&
        fixture.expected.skill === "Node Creation" &&
        calledSchemaCreate(turns)
      ) {
        return {
          passed: false,
          failure: `Adversarial check failed: called create_schema when only Node Creation was expected`,
        };
      }
      return { passed: true };
    }

    case "clarify": {
      if (!clarified) {
        return {
          passed: false,
          failure: `Expected a clarification question but got: ${allReplies.slice(0, 300)}`,
        };
      }
      // Adversarial: mutating adversarial fixtures must NOT fire a mutating tool
      if (fixture.adversarial && calledMutatingTool(turns)) {
        return {
          passed: false,
          failure: `Adversarial check failed: fired a mutating tool (${turns.flatMap((t) => t.toolsCalled).join(",")}) without clarifying first`,
        };
      }
      return { passed: true };
    }

    case "search": {
      const hasSearch = turns.some(
        (t) =>
          t.toolsCalled.includes("search_semantic") ||
          t.toolsCalled.includes("search_nodes"),
      );
      if (!hasSearch) {
        return {
          passed: false,
          failure: `Expected a search tool call (search_semantic or search_nodes) but got tools: ${turns.flatMap((t) => t.toolsCalled).join(",")}. Reply: ${allReplies.slice(0, 300)}`,
        };
      }
      return { passed: true };
    }

    case "multi": {
      // The one expectation that is inherently a Stage-1 fact: what happens
      // downstream of a split is ordinary per-intent routing, covered above.
      if (decision !== "multi") {
        return {
          passed: false,
          failure: `Expected Stage 1 to route_multi but routing decision was ${decision ?? "(not recorded)"}`,
        };
      }
      return { passed: true };
    }

    case "decline": {
      if (askedToClarify(turns)) {
        return {
          passed: false,
          failure: `Asked the user to disambiguate a clear (if unanswerable) request. Reply: ${allReplies.slice(0, 300)}`,
        };
      }
      if (calledMutatingTool(turns)) {
        return {
          passed: false,
          failure: `Fired a mutating tool on an out-of-scope request (${turns.flatMap((t) => t.toolsCalled).join(",")})`,
        };
      }
      const reply = turns.at(-1)?.reply.trim() ?? "";
      if (reply === "" || reply === "(no reply parsed)") {
        return {
          passed: false,
          failure: "Turn ended without a reply — neither a refusal nor a result",
        };
      }
      return { passed: true };
    }
  }
}

const fixture: EvalFixture = {
  name: "routing",
  description: "Routing Eval Results (skill-discovery accuracy)",
  // Each scenario is independent, so every one gets its own chat node.
  groups: FIXTURES.map((f) => [f]),
  score(scenario, turns) {
    return assertFixture(scenario as RoutingScenario, turns);
  },
  extra(scenario, turns) {
    const s = scenario as RoutingScenario;
    return {
      expected: s.expected,
      expectedStage1: stage1Expectation(s.expected),
      routingDecision: routingDecisionOf(turns),
      singleIntentAtLength: s.singleIntentAtLength ?? false,
      loadBearing: s.loadBearing ?? false,
      mutating: s.mutating ?? false,
      adversarial: s.adversarial ?? false,
      matchedSkill: skillNameFromTurns(turns),
      clarified: isClarification(turns.map((t) => t.reply).join("\n")),
      toolsCalled: turns.flatMap((t) => t.toolsCalled),
      latencyMs: turns.reduce((sum, t) => sum + t.latencyMs, 0),
    };
  },
  summary(results) {
    const count = (pred: (e: Record<string, unknown>) => boolean) => {
      const rows = results.filter((r) => r.extra && pred(r.extra));
      return `${rows.filter((r) => r.passed).length}/${rows.length}`;
    };
    const base = constantAnswerBaseline(FIXTURES);
    return [
      `Load-bearing (indirect phrasing + clarification): ${count((e) => e.loadBearing === true)}`,
      `Mutating gate:  ${count((e) => e.mutating === true)}`,
      `Compound → route_multi: ${count((e) => (e.expected as ExpectedOutcome).kind === "multi")}`,
      `Single intent at length (must not route_multi): ${count((e) => e.singleIntentAtLength === true)}`,
      `Stage-1 constant-answer baseline: ${((100 * base.hits) / base.total).toFixed(1)}% (always route_${base.decision}, ${base.hits}/${base.total})`,
    ];
  },
};

export default fixture;
