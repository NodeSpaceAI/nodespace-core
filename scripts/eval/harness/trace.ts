#!/usr/bin/env bun
/**
 * Prints a run's log as one line per thing that happened: each prompt, each
 * tool call with the start of its result, and what the agent said. Reads the
 * Claude Code, OpenCode, Codex, Antigravity and Pi (RPC) event shapes.
 *
 *   bun scripts/eval/harness/trace.ts <log.jsonl> [--full]
 */

import { readFileSync } from "node:fs";
import { EXIT_USAGE } from "../preflight.ts";

type Json = Record<string, unknown>;

const file = process.argv[2];

if (!file) {
  console.error("Usage: bun scripts/eval/harness/trace.ts <log.jsonl> [--full]");
  process.exit(EXIT_USAGE);
}

const resultChars = process.argv.includes("--full") ? 4000 : 300;
/** The text of each agent response step so far, by step index: it arrives in pieces. */
const spoken = new Map<number, string>();
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
  } else if (event.event === "step_update") {
    // Antigravity: the agent's text arrives as deltas of one step, and a tool
    // call is a step that goes ACTIVE (with its parameters) and then DONE or ERROR.
    const step = (event.step_update ?? {}) as Json;
    const index = Number(step.step_index);
    const info = (step.tool_info ?? {}) as Json;
    const parameters = (info.parameters ?? {}) as Json;

    if (step.step_type === "agent_response") {
      spoken.set(index, `${spoken.get(index) ?? ""}${String(step.text_delta ?? "")}`);

      if (step.state === "DONE" && (spoken.get(index) ?? "").trim() !== "") {
        console.log(`SAYS ${cut(spoken.get(index), 1400)}`);
      }
    } else if (step.step_type === "tool" && step.state === "ACTIVE") {
      console.log(`TOOL ${String(step.tool_name)} ${cut(parameters.CommandLine ?? JSON.stringify(parameters), 500)}`);
    } else if (step.step_type === "tool" && (step.state === "DONE" || step.state === "ERROR")) {
      const error = (info.error ?? {}) as Json;

      console.log(`  -> ${step.state === "ERROR" ? "ERROR " : ""}${cut(error.message ?? info.output, resultChars)}`);
    }
  } else if (event.type === "tool_execution_start") {
    // Pi (RPC): a tool call is a start and an end, correlated by id.
    const args = (event.args ?? {}) as Json;

    console.log(`TOOL ${String(event.toolName)} ${cut(args.command ?? args.path ?? JSON.stringify(args), 500)}`);
  } else if (event.type === "tool_execution_end") {
    const result = (event.result ?? {}) as Json;

    console.log(`  -> ${event.isError ? "ERROR " : ""}${cut(parts(result.content).map(c => String(c.text ?? "")).join(" "), resultChars)}`);
  } else if (event.type === "message_end" && (message.role === "assistant" || message.role === "custom")) {
    const text = typeof message.content === "string" ? message.content : parts(message.content).map(c => (c.type === "text" ? String(c.text ?? "") : "")).join("");

    if (text.trim() !== "") {
      console.log(`${message.role === "assistant" ? "SAYS" : `GIVEN (${String(message.customType)})`} ${cut(text, 1400)}`);
    }
  } else if (event.type === "extension_ui_request") {
    console.log(`UI ${String(event.method)} ${cut(event.statusText ?? event.message ?? event.title, 300)}`);
  } else if (event.type === "extension_error") {
    console.log(`EXTENSION ERROR ${String(event.event)} ${cut(event.error, 500)}`);
  } else if (event.type === "item.started" || event.type === "item.completed") {
    // Codex: a shell command is an item, and so is each thing the agent says.
    const item = (event.item ?? {}) as Json;

    if (item.type === "agent_message" && event.type === "item.completed") {
      console.log(`SAYS ${cut(item.text, 1400)}`);
    } else if (item.type === "command_execution" && event.type === "item.started") {
      console.log(`TOOL shell ${cut(item.command, 500)}`);
    } else if (item.type === "command_execution") {
      console.log(`  -> exit ${String(item.exit_code)} ${cut(item.aggregated_output, resultChars)}`);
    }
  }
}
