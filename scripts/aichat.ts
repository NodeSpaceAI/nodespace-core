#!/usr/bin/env bun
/**
 * aichat.ts — drive an ai-chat node end-to-end through the CLI, no UI.
 *
 * Used to iterate on agent prompting. Talks to a freshly-built nodespaced over a
 * dedicated test socket/DB so it never touches the user's real ~/.nodespace data.
 *
 * Mechanism: there is no "send message" RPC. The daemon's event watcher runs an
 * inference turn when an ai-chat node's properties['ai-chat'] has
 * turn_status:"processing" AND a trailing role:"user" message. On completion
 * it appends the assistant reply and sets turn_status:"idle" — a separate,
 * PTY-owned session_status axis is never touched by this flow. So a turn is:
 * batch-update (append user msg + turn_status:processing) → poll get until idle.
 *
 * Commands:
 *   bun run scripts/aichat.ts new                  Create an ai-chat node; prints its ID.
 *   bun run scripts/aichat.ts send <id> "message"  Run one turn; prints reply + tool calls.
 *   bun run scripts/aichat.ts ask "message"        Shorthand: new + send.
 *   bun run scripts/aichat.ts show <id>            Dump the full message history.
 *
 * Env:
 *   NS_BIN             Path to the `nodespace` CLI (default: worktree release build).
 *   NODESPACED_SOCKET  Socket the CLI/daemon share (default: test socket).
 *   NS_LOG             Daemon log scraped for tool calls (default: test log).
 *   NS_MODEL           Model id recorded on the node (default: gemma-4-e4b-q4km).
 *   NS_TIMEOUT_MS      Turn timeout in ms (default: 180000).
 */

import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WORKTREE = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const NS_BIN = process.env.NS_BIN ?? join(WORKTREE, "target/release/nodespace");
const SOCKET =
  process.env.NODESPACED_SOCKET ?? "/tmp/nodespaced-test/daemon.sock";
const NS_LOG = process.env.NS_LOG ?? "/tmp/nodespaced-test/daemon.log";
const NS_MODEL = process.env.NS_MODEL ?? "gemma-4-e4b-q4km";
const TIMEOUT_MS = Number(process.env.NS_TIMEOUT_MS ?? 180_000);

interface AiChat {
  provider: string;
  model: string;
  turn_status: string;
  session_status: string;
  messages: Array<{ role: string; content: string; timestamp?: string }>;
}

/** Run the nodespace CLI with --json and parse stdout. Throws on non-zero exit. */
function ns(args: string[]): unknown {
  const result = Bun.spawnSync(
    [NS_BIN, "--socket", SOCKET, "--json", ...args],
    {
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  const stdout = result.stdout.toString();
  if (result.exitCode !== 0) {
    throw new Error(
      `nodespace ${args.join(" ")} failed (exit ${result.exitCode}):\n${result.stderr.toString()}`,
    );
  }
  return stdout.trim() ? JSON.parse(stdout) : null;
}

/** Run the CLI without expecting JSON (for batch-update which prints a summary). */
function nsRaw(args: string[]): void {
  const result = Bun.spawnSync([NS_BIN, "--socket", SOCKET, ...args], {
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) {
    throw new Error(
      `nodespace ${args.join(" ")} failed (exit ${result.exitCode}):\n${result.stderr.toString()}`,
    );
  }
}

interface NodeJson {
  id: string;
  version: number;
  // The CLI's `--json` output is flat — an ai-chat node's own fields
  // (turn_status, messages, ...) sit directly on `properties`, not nested
  // under an "ai-chat" key. Writes still go through batchUpdateProps nested
  // under "ai-chat", which the daemon's `AiChatNode::from_node` accepts
  // (nested-or-flat) regardless of how the CLI echoes it back.
  properties: Partial<AiChat>;
}

function getNode(id: string): NodeJson {
  return ns(["node", "get", id]) as NodeJson;
}

function defaultAiChat(): AiChat {
  return {
    provider: "native",
    model: NS_MODEL,
    turn_status: "idle",
    session_status: "active",
    messages: [],
  };
}

function batchUpdateProps(
  id: string,
  version: number | null,
  props: Record<string, unknown>,
): void {
  const item: Record<string, unknown> = { node_id: id, properties: props };
  if (version !== null) item.version = version;
  nsRaw(["node", "batch-update", "--updates", JSON.stringify([item])]);
}

function cmdNew(): string {
  const created = ns([
    "node",
    "create",
    "--type",
    "ai-chat",
    "--content",
    "CLI test chat",
  ]) as {
    id: string;
  };
  if (!created?.id) throw new Error("create returned no id");
  batchUpdateProps(created.id, null, { "ai-chat": defaultAiChat() });
  return created.id;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));



/** Strip ANSI colour codes that tracing writes to the log. */
function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

/**
 * Turn a daemon log slice (ANSI already stripped) into the `[marker] ...`
 * lines `cmdSend` prints to stdout for the eval runner to scrape.
 *
 * Pure — no file or process I/O — specifically so the marker-parsing logic
 * (where two regressions were caught in review: a multiline raw generation
 * silently truncated by `split("\n")`, and a lookahead terminator that broke
 * if the model's own text contained a literal `[tool]`) is unit-testable
 * directly against a fixed string. See aichat.test.ts.
 */
export function formatTurnLogLines(slice: string): string[] {
  const out: string[] = [];
  const lines = slice.split("\n");
  // The offered tool list, scraped from agent_loop.rs's "Agent turn: system
  // prompt and tools prepared" line. `tool_names` there is the output of
  // routing::stage2_tools — i.e. already scoped to what the routed candidates
  // permit, which is precisely the surface the model saw.
  //
  // An earlier version of this scrape looked for a `scoped tool list` line
  // carrying `selected_tools=`. No such line has ever existed in the daemon,
  // so `[tools offered]` was never emitted and every scored turn in the trace
  // recorded an empty tool list — an availability claim about any scenario
  // could not be checked from results at all. The real line was already being
  // scraped two lines below under a second marker name; the two are now one.
  const prepared = lines
    .filter((l) => l.includes("system prompt and tools prepared"))
    .pop();
  if (prepared) {
    const m = prepared.match(/tool_names="?([^"]*?)"? system_prompt_len/);
    if (m) out.push(`[tools offered] ${m[1]}`);
    // Whether Stage 2's prompt actually carried a candidate block — distinct
    // from whether routing ran at all. A turn that routed but matched nothing
    // looks identical to one that never routed unless this is captured
    // separately (see agent_loop.rs's "Agent turn: system prompt and tools
    // prepared" line).
    const injected = prepared.match(/stage2_candidates_injected=(true|false)/)?.[1];
    if (injected) out.push(`[stage2 injected] ${injected}`);
  }
  // Stage 1's routing decision. Emitted on one of four lines depending which
  // path a turn took (see agent_loop.rs::route): "routing unavailable for
  // this turn", "stage-1 routing failed", "stage-1 routing decision" (the
  // clarify path, which returns before the line below), or "two-stage
  // routing overhead" (query/clarify_suppressed/none). Take the last, in case
  // a prior context turn in the same slice also routed.
  const routingLine = lines
    .filter(
      (l) =>
        l.includes("routing unavailable for this turn") ||
        l.includes("stage-1 routing failed") ||
        l.includes("stage-1 routing decision") ||
        l.includes("two-stage routing overhead"),
    )
    .pop();
  if (routingLine) {
    const m = routingLine.match(/routing_decision="?([a-z_]+)"?/);
    if (m) out.push(`[routing] ${m[1]}`);
    // Which skills were routed to, not just how many candidates retrieval
    // returned. Only the "two-stage routing overhead" line carries this (the
    // other three routing paths never reach retrieval), and it names the
    // candidates clearing the score gate — the ones actually rendered into
    // Stage 2's prompt. Emitted only when non-empty: a turn that routed but
    // matched nothing above the bar is already reported by `[stage2
    // injected] false`, and a marker with an empty value would be
    // indistinguishable from one this scrape failed to parse.
    //
    // Matched to END OF LINE rather than to a closing quote. tracing quotes a
    // string field only when it needs to, and it does not here, so the value
    // arrives bare: `routed_skills=Organization, Research & Search, Node
    // Creation`. Skill names contain both spaces and commas, so no delimiter
    // short of the line end is safe — and agent_loop.rs emits this field last
    // on the line for exactly that reason. A quoted-only pattern silently
    // matched nothing and put this marker right back in the state the dead
    // `scoped tool list` scrape was in.
    const skills = routingLine.match(/routed_skills="?(.*?)"?$/)?.[1]?.trim();
    if (skills) out.push(`[routed skills] ${skills}`);
  }
  // Raw generation per ReAct iteration — only present when the daemon was
  // launched with RUST_LOG=debug (or a filter including this target at
  // debug), since agent_loop.rs logs it at debug level specifically so
  // production's default `info` verbosity is unaffected.
  //
  // `raw_response` is free-form model text, so agent_loop.rs JSON-encodes it
  // before logging rather than writing it verbatim (tracing's `%` Display
  // formatter would otherwise pass embedded newlines straight through,
  // breaking the one-line-per-record assumption every marker here relies on
  // — a multiline generation would get silently truncated at its first
  // newline by the `slice.split("\n")` above). The line below is therefore
  // guaranteed to be exactly one line, and `raw_response=` is followed by a
  // JSON string literal we can parse back into the original text, quotes,
  // newlines, and all.
  for (const l of lines.filter((l) => l.includes("Agent loop: raw generation"))) {
    const iterMatch = l.match(/iteration=(\d+)/);
    const respIdx = l.indexOf("raw_response=");
    if (iterMatch && respIdx !== -1) {
      const jsonStr = l.slice(respIdx + "raw_response=".length);
      try {
        const raw = JSON.parse(jsonStr);
        out.push(`[raw] iteration=${iterMatch[1]} ${JSON.stringify(raw)}`);
      } catch {
        // Older daemon build logging the pre-fix verbatim form, or a
        // truncated line — skip rather than emit a marker downstream can't
        // trust.
      }
    }
  }
  for (const l of lines.filter((l) => l.includes("Tool executed"))) {
    const tool = l.match(/tool="?([a-z_]+)"?/)?.[1] ?? "?";
    const args = l.match(/args_preview="?([^"]*?)"? result_preview/)?.[1] ?? "";
    const err = /is_error=true/.test(l) ? " [ERROR]" : "";
    // Field count of the persisted result, emitted by any tool whose result
    // carries a top-level `fields` array. tracing omits the field entirely when
    // it is None, so "absent" (the result reports no fields) stays
    // distinguishable from "=0" (a schema persisted with no properties) — the
    // latter is a real failure that looks identical to success by tool name
    // alone. Emitted before the args, which are free-form and truncated at the
    // source and so must stay last on the line.
    const fields = l.match(/result_field_count=(\d+)/)?.[1];
    const fieldPart = fields === undefined ? "" : ` [fields=${fields}]`;
    // A write that had no properties to persist in the first place — a plain
    // text note, or an update that only changed content. Both legitimately
    // report zero, so `result_field_count` is omitted for them; without this
    // marker that absence is indistinguishable from a stale baseline recorded
    // before the field existed, and an assertion keyed on the count silently
    // passes either way.
    const contentOnly = /"(?:updated_content_only|content_only)":true/.test(l)
      ? " [content-only]"
      : "";
    out.push(`[tool] ${tool}${err}${fieldPart}${contentOnly} ${args}`);
  }
  // The documented degenerate-empty-generation failure mode: the model opens a
  // turn and emits neither text nor a tool call. local_agent_service.rs then
  // logs "inference turn failed" and resets turn_status to idle with NO assistant
  // message appended — from cmdSend's point of view this is indistinguishable
  // from a hung turn that timed out, unless this specific log line is
  // scraped. Matched on the literal error text agent_loop.rs raises so a
  // different inference error (a real bug) is not swallowed the same way.
  const emptyGen = lines.find(
    (l) =>
      l.includes("inference turn failed") &&
      l.includes("model produced empty response with no tool calls"),
  );
  if (emptyGen) out.push(`[empty-generation]`);
  return out;
}

/** Pull this turn's internal decisions out of the daemon log slice. */
function reportTurnLog(sinceByte: number): void {
  let slice = "";
  try {
    const buf = Bun.file(NS_LOG);
    // Read only the bytes appended during this turn.
    slice = stripAnsi(
      Bun.spawnSync([
        "tail",
        "-c",
        `+${sinceByte + 1}`,
        NS_LOG,
      ]).stdout.toString(),
    );
    if (!slice && buf) slice = "";
  } catch {
    return;
  }
  for (const line of formatTurnLogLines(slice)) console.log(line);
}

async function cmdSend(id: string, message: string): Promise<void> {
  const node = getNode(id);
  const aichat: AiChat = { ...defaultAiChat(), ...node.properties };
  const beforeAssistant = aichat.messages.filter(
    (m) => m.role === "assistant",
  ).length;

  const logSize = (() => {
    try {
      return Bun.file(NS_LOG).size;
    } catch {
      return 0;
    }
  })();

  aichat.messages.push({
    role: "user",
    content: message,
    timestamp: new Date().toISOString(),
  });
  aichat.turn_status = "processing";
  batchUpdateProps(id, node.version, { "ai-chat": aichat });

  const deadline = Date.now() + TIMEOUT_MS;
  let latest = aichat;
  while (Date.now() < deadline) {
    await sleep(1000);
    const cur = getNode(id);
    latest = { ...latest, ...cur.properties };
    const afterAssistant = latest.messages.filter(
      (m) => m.role === "assistant",
    ).length;
    if (latest.turn_status === "idle" && afterAssistant > beforeAssistant) break;
  }
  if (latest.turn_status !== "idle") {
    console.error(`(timeout after ${TIMEOUT_MS}ms; turn_status=${latest.turn_status})`);
  }

  if (logSize > 0) reportTurnLog(logSize);

  const reply = [...latest.messages]
    .reverse()
    .find((m) => m.role === "assistant");
  console.log(`assistant> ${reply?.content ?? "(no assistant reply)"}`);
}

function cmdShow(id: string): void {
  const node = getNode(id);
  for (const m of node.properties.messages ?? []) {
    console.log(`${m.role}> ${m.content}`);
  }
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  switch (cmd) {
    case "new":
      console.log(cmdNew());
      break;
    case "send": {
      const [id, ...msg] = rest;
      if (!id || msg.length === 0)
        throw new Error("usage: send <id> <message>");
      await cmdSend(id, msg.join(" "));
      break;
    }
    case "ask": {
      if (rest.length === 0) throw new Error("usage: ask <message>");
      const id = cmdNew();
      console.error(`chat: ${id}`);
      await cmdSend(id, rest.join(" "));
      break;
    }
    case "show": {
      const [id] = rest;
      if (!id) throw new Error("usage: show <id>");
      cmdShow(id);
      break;
    }
    default:
      console.error(
        "usage: aichat.ts {new | send <id> <msg> | ask <msg> | show <id>}",
      );
      process.exit(1);
  }
}

// Guarded so aichat.test.ts can import formatTurnLogLines without triggering
// a CLI invocation — `main()` parses process.argv and exits(1) on no match,
// which is exactly what happened before this guard existed: the test runner's
// own argv doesn't match any subcommand.
if (import.meta.main) {
  main().catch((e) => {
    console.error(e instanceof Error ? e.message : String(e));
    process.exit(1);
  });
}
