#!/usr/bin/env bun
/**
 * aichat.ts — drive a native AI chat (`ai-chat-native`) end-to-end through the CLI, no UI.
 *
 * Used to iterate on agent prompting. Talks to a freshly-built nodespaced over a
 * dedicated test socket/DB so it never touches the user's real ~/.nodespace data.
 *
 * Mechanism: there is no "send message" RPC. A chat's messages are its
 * `ai-chat-message` child nodes, and the daemon's event watcher runs an
 * inference turn when an ai-chat-native node has turn_status:"processing" AND
 * its last message is the user's. On completion it appends the assistant reply
 * as another message node and sets turn_status:"idle". So a turn is: create the
 * user's message under the chat, set turn_status:processing → poll until idle.
 *
 * Commands:
 *   bun run scripts/aichat.ts new                  Create a native chat; prints its ID.
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

import { closeSync, fstatSync, openSync, readSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { providerOf } from "./eval/env.ts";

const WORKTREE = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const NS_BIN = process.env.NS_BIN ?? join(WORKTREE, "target/release/nodespace");
const SOCKET =
  process.env.NODESPACED_SOCKET ?? "/tmp/nodespaced-test/daemon.sock";
const NS_LOG = process.env.NS_LOG ?? "/tmp/nodespaced-test/daemon.log";
const NS_MODEL = process.env.NS_MODEL ?? "gemma-4-e4b-q4km";
const TIMEOUT_MS = Number(process.env.NS_TIMEOUT_MS ?? 180_000);

/** The type of the chat this harness drives: one NodeSpace's own agent loop runs. */
export const CHAT_NODE_TYPE = "ai-chat-native";

/** The type of a chat's messages: its children, in conversation order. */
export const MESSAGE_NODE_TYPE = "ai-chat-message";

interface AiChat {
  agent: string;
  provider: string;
  model: string;
  turn_status: string;
}

interface ChatMessage {
  role: string;
  content: string;
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
  // The CLI's `--json` output is flat — the chat's fields (turn_status,
  // model, ...), inherited ones included, sit directly on `properties`.
  // Writes are flat too: the daemon places each key in the bucket of the
  // schema that declares it.
  properties: Partial<AiChat>;
}

function getNode(id: string): NodeJson {
  return ns(["node", "get", id]) as NodeJson;
}

function defaultAiChat(): AiChat {
  return {
    agent: "nodespace",
    provider: providerOf(NS_MODEL),
    model: NS_MODEL,
    turn_status: "idle",
  };
}

/**
 * A chat's messages, in order, out of a `node children` payload. A chat may
 * hold other children; only its message nodes are the conversation.
 */
export function readMessages(payload: unknown): ChatMessage[] {
  if (typeof payload !== "object" || payload === null) return [];
  const nodes = (payload as { nodes?: unknown }).nodes;
  if (!Array.isArray(nodes)) return [];
  const messages: ChatMessage[] = [];
  for (const raw of nodes) {
    if (typeof raw !== "object" || raw === null) continue;
    const node = raw as {
      node_type?: unknown;
      content?: unknown;
      properties?: { role?: unknown };
    };
    if (node.node_type !== MESSAGE_NODE_TYPE) continue;
    messages.push({
      role: typeof node.properties?.role === "string" ? node.properties.role : "user",
      content: typeof node.content === "string" ? node.content : "",
    });
  }
  return messages;
}

function getMessages(id: string): ChatMessage[] {
  return readMessages(ns(["node", "children", id]));
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
    CHAT_NODE_TYPE,
    "--content",
    "CLI test chat",
    // `agent` is required on create: it says who runs the conversation.
    "--property",
    "agent=nodespace",
  ]) as {
    id: string;
  };
  if (!created?.id) throw new Error("create returned no id");
  batchUpdateProps(created.id, null, { ...defaultAiChat() });
  return created.id;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));



/** Strip ANSI colour codes that tracing writes to the log. */
function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

/**
 * Read one complete JSON object out of `line` starting at `start`, ignoring
 * whatever follows it.
 *
 * `JSON.parse` on the rest of the line would throw the moment tracing emitted
 * another field after this one, so parsing the remainder wholesale would
 * reintroduce exactly the "this field must be last on the line" constraint the
 * JSON payload exists to delete. Scanning to the matching brace instead makes
 * the read independent of field order.
 *
 * String state is tracked because a value may legitimately contain a brace —
 * a schema id or skill name is not a controlled input — and a naive depth
 * count would stop at the wrong character. Escapes are honoured for the same
 * reason: a `\"` inside a value must not be read as closing the string.
 *
 * Returns `undefined` for a line whose payload is absent, truncated, or not an
 * object, so the caller can skip a marker it cannot trust rather than emit a
 * malformed one.
 */
function extractJsonObject(line: string, start: number): unknown | undefined {
  if (line[start] !== "{") return undefined;
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let i = start; i < line.length; i++) {
    const c = line[i];
    if (inString) {
      if (escaped) escaped = false;
      else if (c === "\\") escaped = true;
      else if (c === '"') inString = false;
      continue;
    }
    if (c === '"') inString = true;
    else if (c === "{") depth++;
    else if (c === "}") {
      depth--;
      if (depth === 0) {
        try {
          return JSON.parse(line.slice(start, i + 1));
        } catch {
          return undefined;
        }
      }
    }
  }
  return undefined;
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
  // routing overhead" (query/lookup/multi/multi_rejected/clarify_suppressed/none).
  // Take the last, in case a prior context turn in the same slice also routed.
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
    // Wall-clock spent on Stage 1 alone: one generative pass, or two for a
    // lookup-shaped message that was asked about by itself first and turned
    // out not to be a lookup. Its entire output is a structural choice among
    // four routing tools. Captured
    // separately from the turn's total because it is the cost of *deciding*
    // rather than of answering, and the two are the terms of any
    // decision-model comparison — a replacement that is more accurate but no
    // cheaper, or cheaper but less accurate, are different propositions and a
    // single turn-level number cannot tell them apart.
    const routingMs = routingLine.match(/routing_latency_ms=(\d+)/)?.[1];
    if (routingMs) out.push(`[routing ms] ${routingMs}`);
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
  // The three named decisions per ReAct iteration (agent_loop.rs's "Agent
  // decision: ..." lines, via local_agent::decisions). Each carries the
  // candidate set alongside the outcome, because an outcome alone is not
  // scoreable: whether calling `search_nodes` was right depends on what else
  // was on offer that turn.
  //
  // The payload arrives as one JSON object and is forwarded as one, rather
  // than being unpacked into a delimited marker and re-split downstream. The
  // delimited form it replaces could not represent a name containing a comma
  // (the list was joined and split on one) or a quote (tracing quotes a field
  // only when it must, so the scrape needed a quoted pattern plus a bare
  // fallback, and neither survived an embedded quote). Both are reachable:
  // `create_schema` derives type ids from the model's own phrasing.
  //
  // Forwarding verbatim also means this scrape asserts nothing about the
  // payload's internal shape — it reads `decision` for the marker name and
  // hands the rest to the parser, so the two cannot disagree about a field
  // only one of them knows.
  for (const l of lines.filter((l) => l.includes("Agent decision:"))) {
    const kind = l.match(/decision="?(skill|schema|operation)"?/)?.[1];
    if (!kind) continue;
    const payloadIdx = l.indexOf("decision_payload=");
    if (payloadIdx === -1) continue;
    const payload = extractJsonObject(l, payloadIdx + "decision_payload=".length);
    if (payload === undefined) continue;
    // Re-stringified from the parsed value rather than forwarded as the raw
    // slice, so the marker carries exactly the object and nothing that
    // happened to follow it on the log line.
    out.push(`[decision ${kind}] ${JSON.stringify(payload)}`);
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
    // A call dispatch refused because it named a type outside the turn's
    // offered set: the log line's own `type_refused` field, which dispatch
    // sets from the result the call got back. A count of these is what
    // dispatch did rather than what a decision record implies. Read from the
    // fields ahead of `args_preview`, so text the model put in its arguments
    // cannot be mistaken for it.
    const head = l.split(" args_preview=")[0];
    const typeRefused = /\btype_refused=true\b/.test(head)
      ? " [type-refused]"
      : "";
    // The other side of the same check: a call naming a type outside the
    // offered set that reached the executor. Dispatch works it out from the
    // call's arguments and from whether the call was dispatched, so it is an
    // observation of the event, not something inferred from the refusals.
    const offMenuRan = /\boff_menu_ran=true\b/.test(head)
      ? " [off-menu-ran]"
      : "";
    out.push(
      `[tool] ${tool}${err}${fieldPart}${contentOnly}${typeRefused}${offMenuRan} ${args}`,
    );
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

/**
 * A file's bytes from `fromByte` on, or nothing when it can't be read. Reads
 * only that tail: the log is megabytes long and this runs once a turn.
 */
function readLogFrom(path: string, fromByte: number): string {
  let fd: number | undefined;
  try {
    fd = openSync(path, "r");
    const length = Math.max(0, fstatSync(fd).size - fromByte);
    const tail = Buffer.alloc(length);
    const read = readSync(fd, tail, 0, length, fromByte);
    return tail.subarray(0, read).toString("utf8");
  } catch {
    return "";
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
}

/**
 * What the daemon logged since its log was `sinceByte` long.
 *
 * The daemon rotates its own log once it passes a size threshold: `<log>`
 * becomes `<log>.1` and a new `<log>` starts empty. A log now shorter than
 * it was when the turn began was rotated during the turn, so the turn's lines
 * are the rest of `<log>.1` followed by all of the new file. Reading the new
 * file from the old offset would find nothing, and the turn would be scored
 * as one that made no decision and called no tool.
 *
 * One rotation per turn is all this follows: a turn does not log a whole
 * threshold's worth.
 */
export function readTurnLog(logPath: string, sinceByte: number): string {
  let size = 0;
  try {
    size = statSync(logPath).size;
  } catch {
    return "";
  }
  if (size >= sinceByte) return readLogFrom(logPath, sinceByte);
  return readLogFrom(`${logPath}.1`, sinceByte) + readLogFrom(logPath, 0);
}

/** Pull this turn's internal decisions out of the daemon log slice. */
function reportTurnLog(sinceByte: number): void {
  const slice = stripAnsi(readTurnLog(NS_LOG, sinceByte));
  for (const line of formatTurnLogLines(slice)) console.log(line);
}

async function cmdSend(id: string, message: string): Promise<void> {
  const assistantCount = (messages: ChatMessage[]) =>
    messages.filter((m) => m.role === "assistant").length;
  const beforeAssistant = assistantCount(getMessages(id));

  const logSize = (() => {
    try {
      return Bun.file(NS_LOG).size;
    } catch {
      return 0;
    }
  })();

  // The user's message is a node under the chat; asking for the turn is the
  // chat's `processing` status. The daemon starts the turn once it has both.
  ns([
    "node",
    "create",
    "--type",
    MESSAGE_NODE_TYPE,
    "--parent",
    id,
    "--content",
    message,
    "--property",
    "role=user",
    "--property",
    `timestamp=${new Date().toISOString()}`,
  ]);
  batchUpdateProps(id, getNode(id).version, { turn_status: "processing" });

  const deadline = Date.now() + TIMEOUT_MS;
  let turnStatus = "processing";
  let messages: ChatMessage[] = [];
  while (Date.now() < deadline) {
    await sleep(1000);
    turnStatus = getNode(id).properties.turn_status ?? turnStatus;
    messages = getMessages(id);
    if (turnStatus === "idle" && assistantCount(messages) > beforeAssistant) break;
  }
  if (turnStatus !== "idle") {
    console.error(`(timeout after ${TIMEOUT_MS}ms; turn_status=${turnStatus})`);
  }

  if (logSize > 0) reportTurnLog(logSize);

  const reply = [...messages].reverse().find((m) => m.role === "assistant");
  console.log(`assistant> ${reply?.content ?? "(no assistant reply)"}`);
}

function cmdShow(id: string): void {
  for (const m of getMessages(id)) {
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
