#!/usr/bin/env bun
/**
 * Prints a run's log as one line per thing that happened: each prompt, each
 * tool call with the start of its result, and what the agent said. Reads both
 * the Claude Code and the OpenCode event shapes.
 *
 *   bun scripts/eval/harness/trace.ts <log.jsonl> [--full]
 */

import { readFileSync } from "node:fs";

type Json = Record<string, unknown>;

const file = process.argv[2];

if (!file) {
  console.error("Usage: bun scripts/eval/harness/trace.ts <log.jsonl> [--full]");
  process.exit(64);
}

const resultChars = process.argv.includes("--full") ? 4000 : 300;
const cut = (text: unknown, max: number) => String(text ?? "").replace(/\s+/g, " ").slice(0, max);
const parts = (content: unknown): Json[] => (Array.isArray(content) ? (content as Json[]) : []);

for (const line of readFileSync(file, "utf8").split("\n").filter(Boolean)) {
  let event: Json;

  try {
    event = JSON.parse(line) as Json;
  } catch {
    continue;
  }

  const message = (event.message ?? {}) as Json;
  const part = (event.part ?? {}) as Json;

  if (event.type === "scenario.prompt") {
    console.log(`\n=== PROMPT ${String(event.name)}`);
  } else if (event.type === "system" && event.subtype === "init") {
    console.log(`INIT model=${String(event.model)} plugins=${JSON.stringify(event.plugins)} errors=${JSON.stringify(event.plugin_errors)}`);
  } else if (event.type === "system" && event.subtype === "ui_status") {
    console.log(`STATUS ${String(event.text)}`);
  } else if (event.type === "assistant") {
    for (const said of parts(message.content)) {
      const input = (said.input ?? {}) as Json;

      if (said.type === "text") {
        console.log(`SAYS ${cut(said.text, 1400)}`);
      } else if (said.type === "tool_use") {
        console.log(`TOOL ${String(said.name)} ${cut(input.command ?? input.file_path ?? JSON.stringify(input), 500)}`);
      }
    }
  } else if (event.type === "user") {
    for (const given of parts(message.content)) {
      if (given.type === "tool_result") {
        const text = Array.isArray(given.content) ? parts(given.content).map(c => String(c.text ?? "")).join(" ") : given.content;

        console.log(`  -> ${given.is_error ? "ERROR " : ""}${cut(text, resultChars)}`);
      } else if (given.type === "text") {
        console.log(`GIVEN ${cut(given.text, 600)}`);
      }
    }
  } else if (event.type === "tool_use") {
    const state = (part.state ?? {}) as Json;
    const input = (state.input ?? {}) as Json;

    console.log(`TOOL ${String(part.tool)} [${String(state.status)}] ${cut(input.command ?? input.filePath ?? JSON.stringify(input), 500)}`);
    console.log(`  -> ${cut(state.output ?? state.error, resultChars)}`);
  } else if (event.type === "text") {
    console.log(`SAYS ${cut(part.text, 1400)}`);
  }
}
