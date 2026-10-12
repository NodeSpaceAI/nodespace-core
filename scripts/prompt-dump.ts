#!/usr/bin/env bun
/**
 * prompt-dump.ts — read and pretty-print a prompt/response dump.
 *
 * Every inference engine can write what it put on the wire, and the raw model
 * response, to a JSONL file when `NODESPACE_PROMPT_DUMP` is set (see
 * packages/nlp-engine/src/inference/prompt_dump.rs). One writer and one
 * sequence counter serve both engines, so a turn served locally and a turn
 * served by a remote endpoint appear in one file as one numbered timeline.
 * The daemon log only records sizes and a short preview, which is
 * insufficient for diagnosing prompt/tool-call issues. This reader renders the
 * dump so you can see verbatim what reached the model and what it produced.
 *
 * Workflow:
 *   1. Run the daemon with dumping on:
 *        NODESPACE_PROMPT_DUMP=/tmp/dump.jsonl <launch nodespaced>
 *   2. Drive a turn (e.g. `bun run chat ask "Create an invoice database"`).
 *   3. Inspect:
 *        bun run prompt:dump /tmp/dump.jsonl            # full, in order
 *        bun run prompt:dump /tmp/dump.jsonl --prompt   # what was sent, verbatim
 *        bun run prompt:dump /tmp/dump.jsonl --tools    # tools offered (remote only)
 *        bun run prompt:dump /tmp/dump.jsonl --last     # only the most recent call
 *        bun run prompt:dump /tmp/dump.jsonl --raw      # raw JSONL lines
 *
 * A record has the same shape whichever engine wrote it, tagged with the
 * engine. What differs is only what was put on the wire:
 *   {"kind":"prompt","engine":"native","seq":0,"target":null,
 *    "sent":"<rendered prompt text>","sent_len":N}
 *   {"kind":"prompt","engine":"openai_compat","seq":1,"target":"<base url>",
 *    "sent":{<request body>},"sent_len":N}
 *   {"kind":"response","engine":"native"|"openai_compat","seq":N,
 *    "raw_response":"…","raw_response_len":N}
 */

import { readFileSync } from "node:fs";

export type Engine = "native" | "openai_compat";

export type RequestBody = {
  model?: string;
  messages?: Array<{ role: string; content: unknown }>;
  tools?: Array<{ function?: { name: string; description?: string } }>;
};

export type PromptRecord = {
  kind: "prompt";
  engine: Engine;
  seq: number;
  target: string | null;
  sent: string | RequestBody;
  sent_len: number;
};

export type ResponseRecord = {
  kind: "response";
  engine: Engine;
  seq: number;
  raw_response: string;
  raw_response_len: number;
};

export type DumpRecord = PromptRecord | ResponseRecord;

const RULE = "=".repeat(72);

function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max)}…` : text;
}

function contentText(content: unknown): string {
  return typeof content === "string" ? content : JSON.stringify(content);
}

/** The tool names a request body offers; empty for rendered prompt text. */
function offeredTools(sent: PromptRecord["sent"]): string[] {
  if (typeof sent === "string") return [];
  return (sent.tools ?? []).map((t) => t.function?.name ?? "?");
}

function renderPrompt(r: PromptRecord, flags: Set<string>): string[] {
  const where = r.target ? ` → ${r.target}` : "";
  if (flags.has("--tools")) {
    const tools = offeredTools(r.sent);
    const lines = [`\n${RULE}\nTOOLS OFFERED #${r.seq} [${r.engine}]${where}`];
    if (typeof r.sent === "string") {
      lines.push("  (rendered into the prompt text — see --prompt)");
    } else {
      lines.push(...tools.map((name) => `  • ${name}`));
    }
    return lines;
  }
  if (flags.has("--prompt")) {
    const verbatim =
      typeof r.sent === "string" ? r.sent : JSON.stringify(r.sent, null, 2);
    return [
      `\n${RULE}\nPROMPT #${r.seq} [${r.engine}]${where} | sent_len=${r.sent_len}\n${RULE}`,
      verbatim,
    ];
  }
  const lines = [
    `\n${RULE}\nPROMPT #${r.seq} [${r.engine}]${where} | sent_len=${r.sent_len}`,
  ];
  if (typeof r.sent === "string") {
    lines.push("--- rendered prompt text ---", `  ${JSON.stringify(clip(r.sent, 400))}`);
  } else {
    if (r.sent.model) lines.push(`model: ${r.sent.model}`);
    lines.push("--- messages sent ---");
    for (const m of r.sent.messages ?? []) {
      lines.push(`  [${m.role}] ${JSON.stringify(clip(contentText(m.content), 200))}`);
    }
    const tools = offeredTools(r.sent);
    if (tools.length) lines.push(`--- tools (${tools.length}) ---`, ...tools.map((n) => `  • ${n}`));
  }
  return lines;
}

function renderResponse(r: ResponseRecord): string[] {
  const lines = [`--- RESPONSE #${r.seq} [${r.engine}] (raw_len=${r.raw_response_len}) ---`];
  if (r.raw_response) lines.push(`  raw: ${JSON.stringify(clip(r.raw_response, 300))}`);
  return lines;
}

/**
 * Render a dump's lines, in file order, as the lines the CLI prints.
 *
 * File order is the timeline: one counter numbers every call across both
 * engines, so records from a local turn and a remote turn interleave exactly
 * as the calls happened.
 */
export function renderDump(lines: string[], flags: Set<string>): string[] {
  if (flags.has("--raw")) return lines;

  let records: DumpRecord[] = lines.map((l) => JSON.parse(l) as DumpRecord);

  // --last: keep only the most recent call — the last prompt and its response.
  if (flags.has("--last")) {
    const lastPrompt = [...records].reverse().find((r) => r.kind === "prompt");
    records = lastPrompt ? records.filter((r) => r.seq === lastPrompt.seq) : [];
  }

  const out: string[] = [];
  for (const r of records) {
    if (r.kind === "prompt") {
      out.push(...renderPrompt(r, flags));
    } else if (r.kind === "response") {
      if (flags.has("--prompt") || flags.has("--tools")) continue;
      out.push(...renderResponse(r));
    }
  }
  return out;
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const file = args.find((a) => !a.startsWith("--"));
  const flags = new Set(args.filter((a) => a.startsWith("--")));

  if (!file) {
    console.error("usage: bun run prompt:dump <dump.jsonl> [--prompt|--tools|--last|--raw]");
    process.exit(1);
  }

  let lines: string[];
  try {
    lines = readFileSync(file, "utf8").trim().split("\n").filter(Boolean);
  } catch (e) {
    console.error(`Cannot read ${file}: ${(e as Error).message}`);
    process.exit(1);
  }

  for (const line of renderDump(lines, flags)) console.log(line);
}
