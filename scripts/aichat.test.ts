/**
 * Unit tests for aichat.ts's log-scraping logic.
 *
 * Runs via `bun run test:scripts` (and so under `bun run test:all`). Deliberate
 * exception to the project-wide "never use `bun test`" rule: this file touches
 * no DOM, and cannot run under Vitest anyway (imports `bun:test`, and scripts/
 * is outside every Vitest project glob).
 *
 * These specifically regression-test two bugs a code review caught before
 * merge: `formatTurnLogLines` splitting the log slice on `\n` before scanning
 * for the raw-generation marker silently truncated any multiline model
 * output at its first newline, and (before the JSON-encoding fix on the Rust
 * side) a lookahead-based parse of that marker broke if the model's own text
 * happened to contain a literal `[tool]` substring.
 *
 * FIXTURES MUST BE VERBATIM DAEMON OUTPUT. `tracing` quotes a field value only
 * when it needs to, so `%`-formatted fields arrive bare —
 * `tool_names=create_node, search_nodes`, not `tool_names="..."`. Fixtures here
 * once used the quoted form throughout, which meant they asserted a shape the
 * daemon never emits: the tests passed while the real scrape matched nothing,
 * and an always-empty `toolsOffered` reached every committed trace. When adding
 * a marker, copy its line from a real log rather than writing it by hand.
 */

import { describe, expect, test } from "bun:test";
import { formatTurnLogLines } from "./aichat.ts";

/** Build a daemon-log-shaped raw-generation line the way agent_loop.rs emits it. */
function rawGenerationLine(iteration: number, text: string): string {
  return `2026-07-30T22:27:39Z DEBUG nodespace_agent: Agent loop: raw generation iteration=${iteration} raw_response=${JSON.stringify(text)}`;
}

describe("formatTurnLogLines", () => {
  test("extracts a single-line raw generation", () => {
    const slice = rawGenerationLine(0, "Hi there!");
    const lines = formatTurnLogLines(slice);
    expect(lines).toEqual([`[raw] iteration=0 ${JSON.stringify("Hi there!")}`]);
  });

  test("preserves a multiline raw generation intact (regression)", () => {
    const text = "line one\nline two\nline three";
    const slice = rawGenerationLine(0, text);
    const lines = formatTurnLogLines(slice);
    expect(lines).toHaveLength(1);
    expect(JSON.parse(lines[0].replace(/^\[raw\] iteration=0 /, ""))).toBe(text);
  });

  test("does not truncate raw text containing a literal [tool] marker (regression)", () => {
    const text = "I'll use [tool] create_node to do that.";
    const slice = rawGenerationLine(0, text);
    const lines = formatTurnLogLines(slice);
    expect(lines).toHaveLength(1);
    expect(JSON.parse(lines[0].replace(/^\[raw\] iteration=0 /, ""))).toBe(text);
  });

  test("handles multiple iterations plus a following real log line without cross-contamination", () => {
    const slice = [
      rawGenerationLine(0, "first\nmultiline\nresponse"),
      rawGenerationLine(1, "second response, mentions [tool] search_nodes"),
      `2026-07-30T22:27:40Z  INFO nodespace_agent: Tool executed tool=create_node is_error=false result_field_count=3 args_preview={} result_preview={}`,
    ].join("\n");

    const lines = formatTurnLogLines(slice);
    const rawLines = lines.filter((l) => l.startsWith("[raw]"));
    expect(rawLines).toHaveLength(2);
    expect(
      JSON.parse(rawLines[0].replace(/^\[raw\] iteration=0 /, "")),
    ).toBe("first\nmultiline\nresponse");
    expect(
      JSON.parse(rawLines[1].replace(/^\[raw\] iteration=1 /, "")),
    ).toBe("second response, mentions [tool] search_nodes");

    const toolLines = lines.filter((l) => l.startsWith("[tool]"));
    expect(toolLines).toHaveLength(1);
    expect(toolLines[0]).toContain("create_node");
    expect(toolLines[0]).toContain("[fields=3]");
  });

  test("skips a raw-generation line whose payload is not valid JSON rather than emitting garbage", () => {
    // Simulates an older daemon build that logged the pre-fix verbatim form.
    const slice =
      "2026-07-30T22:27:39Z DEBUG nodespace_agent: Agent loop: raw generation iteration=0 raw_response=not json here";
    const lines = formatTurnLogLines(slice);
    expect(lines.filter((l) => l.startsWith("[raw]"))).toEqual([]);
  });

  test("extracts stage2 injected and routing markers", () => {
    const slice = [
      `2026-07-30T22:27:39Z  INFO nodespace_agent: Agent turn: system prompt and tools prepared tools_count=5 tool_names=create_node, search_nodes system_prompt_len=1234 stage2_candidates_injected=true`,
      // routed_skills is UNQUOTED here, as tracing actually emits it — a
      // quoted fixture passed while the real scrape matched nothing.
      `2026-07-30T22:27:39Z  INFO nodespace_agent: two-stage routing overhead routing_decision="query" routing_latency_ms=120 candidates=2 routed_skills=Node Creation`,
    ].join("\n");
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain("[stage2 injected] true");
    expect(lines).toContain("[routing] query");
    // Asserted here because its absence is what let the dead `scoped tool
    // list` scrape survive: this test covered the neighbouring markers on the
    // same log line and never checked that the tool list itself came through,
    // so `toolsOffered` was empty on every turn of every committed trace.
    expect(lines).toContain("[tools offered] create_node, search_nodes");
    expect(lines).toContain("[routed skills] Node Creation");
  });

  test("captures routed skill names containing spaces and commas", () => {
    // The real-world shape: tracing leaves the value bare and skill names carry
    // both separators, so the scrape must run to end of line. Verbatim from a
    // live daemon log.
    const slice = `2026-07-30T22:27:39Z  INFO nodespace_agent: two-stage routing overhead routing_decision="query" routing_latency_ms=120 candidates=3 routed_skills=Organization, Research & Search, Node Creation`;
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain(
      "[routed skills] Organization, Research & Search, Node Creation",
    );
  });

  test("emits no routed-skills marker when nothing cleared the score gate", () => {
    // A turn that routed but matched nothing above the bar. An empty marker
    // would be indistinguishable from a scrape that failed to parse, so the
    // marker is omitted entirely and `[stage2 injected] false` carries the
    // signal instead.
    const slice = [
      `2026-07-30T22:27:39Z  INFO nodespace_agent: Agent turn: system prompt and tools prepared tools_count=5 tool_names=create_node system_prompt_len=1234 stage2_candidates_injected=false`,
      `2026-07-30T22:27:39Z  INFO nodespace_agent: two-stage routing overhead routing_decision="none" routing_latency_ms=120 candidates=0 routed_skills=""`,
    ].join("\n");
    const lines = formatTurnLogLines(slice);
    expect(lines.filter((l) => l.startsWith("[routed skills]"))).toEqual([]);
    expect(lines).toContain("[stage2 injected] false");
  });

  test("takes the routed skills of the last routing line in a multi-turn slice", () => {
    // Mirrors the existing `[routing]` behaviour: a slice can contain a prior
    // context turn's routing line, and the marker must describe this turn.
    const slice = [
      `2026-07-30T22:27:39Z  INFO nodespace_agent: two-stage routing overhead routing_decision="query" routing_latency_ms=120 candidates=1 routed_skills=Stale Earlier Turn`,
      `2026-07-30T22:27:41Z  INFO nodespace_agent: two-stage routing overhead routing_decision="query" routing_latency_ms=118 candidates=2 routed_skills=Node Creation, Graph Editing`,
    ].join("\n");
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain("[routed skills] Node Creation, Graph Editing");
    expect(lines.filter((l) => l.startsWith("[routed skills]"))).toHaveLength(1);
  });

  // The decision payload arrives as one JSON object. These cover this
  // scrape's own behaviour against hand-written lines; the emitter → scrape →
  // parse contract itself is pinned mechanically by
  // scripts/eval/decision-roundtrip.test.ts, which reads log lines the real
  // Rust tracing layer emitted rather than lines written here by hand.
  test("captures both named decisions with their candidate sets", () => {
    const slice = [
      `2026-09-22T10:00:00Z  INFO nodespace_agent: Agent decision: operation selected iteration=0 decision="operation" decision_payload={"candidates":["create_node","search_nodes","get_node"],"off_menu":false,"selected":"create_node"}`,
      `2026-09-22T10:00:00Z  INFO nodespace_agent: Agent decision: schema selected iteration=0 decision="schema" decision_payload={"candidates":["invoice","customer"],"off_menu":false,"selected":"invoice"}`,
    ].join("\n");
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain(
      '[decision operation] {"candidates":["create_node","search_nodes","get_node"],"off_menu":false,"selected":"create_node"}',
    );
    expect(lines).toContain(
      '[decision schema] {"candidates":["invoice","customer"],"off_menu":false,"selected":"invoice"}',
    );
  });

  test("keeps a multi-word skill name intact (regression)", () => {
    // Under the delimited format `decision_selected` was matched as a
    // non-whitespace run, truncating this to "Schema" — invisible for tool
    // names and type ids (never spaced) and wrong for every skill. The JSON
    // payload removes the class of bug rather than the one instance.
    const slice = `2026-09-22T13:45:00Z  INFO nodespace_agent: Agent decision: skill selected iteration=0 decision="skill" decision_payload={"candidates":["Schema Creation","Conflict Resolution"],"off_menu":false,"selected":"Schema Creation"}`;
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain(
      '[decision skill] {"candidates":["Schema Creation","Conflict Resolution"],"off_menu":false,"selected":"Schema Creation"}',
    );
  });

  test("a candidate name containing a comma stays one candidate", () => {
    // The bug the JSON payload was introduced for. The delimited form joined
    // candidates with ", " and was split on ",", so this name became two.
    const slice = `2026-09-22T10:00:00Z  INFO nodespace_agent: Agent decision: schema selected iteration=0 decision="schema" decision_payload={"candidates":["Company, Sold To","invoice"],"off_menu":false,"selected":"Company, Sold To"}`;
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain(
      '[decision schema] {"candidates":["Company, Sold To","invoice"],"off_menu":false,"selected":"Company, Sold To"}',
    );
  });

  test("reads the payload even when another field follows it on the line", () => {
    // Field order carries no meaning now. Under the delimited format the
    // candidate list had to be last on the line, an invariant documented in
    // three files and enforced in none — so a reordering broke the scrape
    // silently. The payload is read to its matching brace instead.
    const slice = `2026-09-22T10:00:00Z  INFO nodespace_agent: Agent decision: operation selected decision="operation" decision_payload={"candidates":["get_node"],"off_menu":false,"selected":"get_node"} iteration=0`;
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain(
      '[decision operation] {"candidates":["get_node"],"off_menu":false,"selected":"get_node"}',
    );
  });

  test("a brace inside a candidate name does not truncate the payload", () => {
    // The payload is read by scanning to the matching brace, so the scan
    // tracks string state: a brace inside a value must not be counted as
    // structure. Names are not a controlled input.
    const slice = `2026-09-22T10:00:00Z  INFO nodespace_agent: Agent decision: schema selected decision="schema" decision_payload={"candidates":["a } brace","invoice"],"off_menu":false,"selected":"a } brace"}`;
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain(
      '[decision schema] {"candidates":["a } brace","invoice"],"off_menu":false,"selected":"a } brace"}',
    );
  });

  test("skips a decision line whose payload is truncated", () => {
    // A half-written line is dropped rather than forwarded as a marker the
    // parser would have to guess at.
    const slice = `2026-09-22T10:00:00Z  INFO nodespace_agent: Agent decision: schema selected decision="schema" decision_payload={"candidates":["inv`;
    expect(formatTurnLogLines(slice).filter((l) => l.startsWith("[decision"))).toHaveLength(0);
  });

  test("records an empty selection without dropping the marker", () => {
    // ADR-056's Scenario 6 shape: tools were offered and none was called.
    // Dropping the marker would make that failure indistinguishable from a
    // line this scrape could not parse.
    const slice = `2026-09-22T10:00:00Z  INFO nodespace_agent: Agent decision: operation selected iteration=1 decision="operation" decision_payload={"candidates":["search_nodes","update_node"],"off_menu":false,"selected":null}`;
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain(
      '[decision operation] {"candidates":["search_nodes","update_node"],"off_menu":false,"selected":null}',
    );
  });

  test("keeps one decision marker per iteration in a multi-round turn", () => {
    // Unlike `[routed skills]`, which describes the turn and is taken from the
    // last line, decisions are per-round: a ReAct turn that searched and then
    // wrote made two operation decisions and both are scoreable.
    const slice = [
      `2026-09-22T10:00:00Z  INFO nodespace_agent: Agent decision: operation selected iteration=0 decision="operation" decision_payload={"candidates":["search_nodes","update_node"],"off_menu":false,"selected":"search_nodes"}`,
      `2026-09-22T10:00:02Z  INFO nodespace_agent: Agent decision: operation selected iteration=1 decision="operation" decision_payload={"candidates":["search_nodes","update_node"],"off_menu":false,"selected":"update_node"}`,
    ].join("\n");
    const lines = formatTurnLogLines(slice);
    expect(lines.filter((l) => l.startsWith("[decision operation]"))).toHaveLength(2);
  });

  test("extracts the empty-generation marker only on the documented error text", () => {
    const slice =
      '2026-07-30T22:27:39Z  WARN nodespace_daemon: inference turn failed error="model produced empty response with no tool calls"';
    const lines = formatTurnLogLines(slice);
    expect(lines).toContain("[empty-generation]");
  });

  test("does not emit the empty-generation marker for an unrelated inference failure", () => {
    const slice =
      '2026-07-30T22:27:39Z  WARN nodespace_daemon: inference turn failed error="connection reset"';
    const lines = formatTurnLogLines(slice);
    expect(lines.filter((l) => l === "[empty-generation]")).toEqual([]);
  });
});
