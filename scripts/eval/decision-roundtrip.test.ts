/**
 * The cross-language half of the `[decision ...]` marker contract.
 *
 * `packages/agent/tests/decision_marker_golden.rs` emits real
 * `DecisionRecord`s through the real tracing layer and commits the captured
 * log lines to the golden file this test reads. Here those same lines go
 * through the real `formatTurnLogLines` scrape and the real `parseTurnOutput`
 * parser, and the recovered records are asserted against what Rust encoded.
 *
 * ## Why this test is not "more parser tests"
 *
 * `aichat.test.ts` and `runner.test.ts` each assert their own site against
 * their own hand-written fixture strings. Both can be green while the three
 * sites disagree, because no test compares a fixture to what the emitter
 * actually emits. That is not hypothetical — it has happened twice in this
 * format family:
 *
 * - a scrape looked for a `scoped tool list` line carrying `selected_tools=`
 *   that has never existed in the daemon, so `[tools offered]` was never
 *   emitted and every scored turn recorded an empty tool list;
 * - `decision_selected` was matched as a non-whitespace run, truncating
 *   `"Schema Creation"` to `"Schema"` — invisible for tool names and type ids,
 *   wrong for every skill, and caught by eye rather than by a test.
 *
 * This file is the mechanical replacement for "a human notices". Change the
 * Rust emission and this test fails, because its input is the Rust output.
 *
 * If it fails after a deliberate emission change: regenerate the golden with
 * `UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test decision_marker_golden`,
 * then update the expectations below to match the new contract.
 */

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { formatTurnLogLines } from "../aichat";
import { parseTurnOutput } from "./runner";

/** The golden emitted by the Rust side. */
const GOLDEN = join(
  import.meta.dir,
  "../../packages/agent/tests/golden/decision_markers/decision_markers.golden",
);

/**
 * Drive the golden through the real production path: scrape, then parse.
 *
 * `formatTurnLogLines` returns marker lines; `parseTurnOutput` consumes the
 * joined stdout `aichat.ts` would have printed. Joining them here reproduces
 * that hand-off exactly rather than calling the parser on a constructed
 * string.
 */
function roundTrip() {
  const slice = readFileSync(GOLDEN, "utf8");
  const markers = formatTurnLogLines(slice);
  return {
    markers,
    decisions: parseTurnOutput(markers.join("\n"), 0).decisions,
  };
}

describe("decision marker round trip (Rust emit → scrape → parse)", () => {
  test("recovers every decision the Rust side emitted", () => {
    const { decisions } = roundTrip();
    // Nine records, matching the golden's nine emitted lines. Asserted as a
    // count first so a silently dropped record fails here with a readable
    // number rather than as a confusing mismatch inside the table below.
    expect(decisions).toHaveLength(9);
  });

  test("a candidate name containing the old delimiter stays one candidate", () => {
    // THE bug this issue was filed for. The previous format joined candidates
    // with ", " and split on ",", so this name became two candidates and the
    // selection read as off-menu — a corrupted record that looks valid to any
    // scorer counting candidates or testing membership.
    const { decisions } = roundTrip();
    const rec = decisions?.[3];

    expect(rec?.kind).toBe("schema");
    expect(rec?.candidates).toEqual(["Company, Sold To", "invoice"]);
    expect(rec?.selected).toBe("Company, Sold To");
    expect(rec?.offMenu).toBe(false);
  });

  test("a candidate name containing a quote survives intact", () => {
    // tracing quotes a string field only when it must — which for the old
    // `decision_selected` meant only when the value contained a space — so the
    // scrape carried a quoted pattern plus a bare fallback and neither
    // survived an embedded quote.
    const { decisions } = roundTrip();
    const rec = decisions?.[4];

    expect(rec?.kind).toBe("skill");
    expect(rec?.candidates).toEqual(['a " quote', "plain"]);
    expect(rec?.selected).toBe('a " quote');
  });

  test("a candidate name containing a newline survives intact", () => {
    // The scrape's very first action on a log slice is split("\n"). A verbatim
    // newline would truncate this record at the break and drop the rest, so
    // the JSON encoding keeping it on one line is what makes this parseable at
    // all.
    const { decisions } = roundTrip();
    const rec = decisions?.[5];

    expect(rec?.kind).toBe("schema");
    expect(rec?.candidates).toEqual(["a \n newline", "invoice"]);
    expect(rec?.selected).toBe("a \n newline");
  });

  test("an empty selection stays distinct from no selection", () => {
    // Two different outcomes that the encoding must not collapse: `null` is
    // the model declining to pick (ADR-056's Scenario 6 shape, the failure
    // class that motivated recording decisions at all), while `""` is a name
    // it produced that happens to be blank.
    const { decisions } = roundTrip();

    expect(decisions?.[6].selected).toBeNull();
    expect(decisions?.[7].selected).toBe("");
    expect(decisions?.[7].selected).not.toBeNull();

    // The blank selection is also off-menu, and deliberately so rather than
    // incidentally: `""` is a selection that the candidate set does not offer,
    // so it is reported like any other unoffered name. Asserted because the
    // eval both fails a scenario on this flag and counts it into a reported
    // "off-menu type named" figure, so a change here is a change to a score.
    expect(decisions?.[7].offMenu).toBe(true);
    // The genuine no-selection record is NOT off-menu — declining to pick is
    // not naming something unoffered, and conflating them would put a null in
    // the numerator of that same figure.
    expect(decisions?.[6].offMenu).toBe(false);
  });

  test("an off-menu selection round-trips its flag", () => {
    const { decisions } = roundTrip();
    const rec = decisions?.[2];

    expect(rec?.selected).toBe("album");
    expect(rec?.candidates).toEqual(["invoice", "customer"]);
    expect(rec?.offMenu).toBe(true);
  });

  test("an empty candidate surface round-trips as an empty list", () => {
    const { decisions } = roundTrip();
    const rec = decisions?.[8];

    expect(rec?.candidates).toEqual([]);
    expect(rec?.selected).toBeNull();
  });

  test("the ordinary shapes are carried alongside the adversarial ones", () => {
    // A regression in the common path should surface in this same file rather
    // than only in the per-site tests.
    const { decisions } = roundTrip();

    expect(decisions?.[0]).toEqual({
      kind: "skill",
      candidates: ["Schema Creation", "Node Creation"],
      selected: "Schema Creation",
      offMenu: false,
    });
    expect(decisions?.[1]).toEqual({
      kind: "operation",
      candidates: ["search_nodes", "create_node"],
      selected: "create_node",
      offMenu: false,
    });
  });

  test("every emitted line produced exactly one marker", () => {
    // Binds the scrape to the emitter by count: a line the scrape silently
    // failed to match would show up here rather than as a missing record in
    // some later assertion. This is the shape of the `scoped tool list`
    // failure, where a scrape matched nothing at all and stayed invisible.
    const { markers } = roundTrip();
    const emitted = readFileSync(GOLDEN, "utf8")
      .split("\n")
      .filter((l) => l.includes("Agent decision:")).length;

    expect(markers.filter((m) => m.startsWith("[decision "))).toHaveLength(emitted);
  });
});
