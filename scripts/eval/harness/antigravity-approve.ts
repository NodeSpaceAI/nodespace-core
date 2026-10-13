#!/usr/bin/env bun
/**
 * The scenario's permission rule for Antigravity, run as a `PreToolUse` hook
 * from the scratch checkout's `.agents/hooks.json`.
 *
 * Antigravity in print mode cannot prompt, so a tool call that needs a
 * permission is denied. This answers for the one fixture checkout and nothing
 * else: a shell command is approved when every command in the line is one the
 * scenario needs, and anything else is left to the permission system, which
 * denies it in print mode. It is not a bypass: `--dangerously-skip-permissions`
 * approves everything, this approves a short list.
 *
 * It answers with a `decision`, which `PreToolUse` requires, and prints
 * nothing else.
 */

// A module, so the top-level `await` below is one.
export {};

/** The words a scenario run uses; each word is the first of one command. */
const ALLOWED = new Set([
  "nodespace", "git", "node", "sh", "cat", "ls", "grep", "echo", "printf", "head", "tail", "sed", "wc", "test",
  "mkdir", "touch", "awk", "sort", "uniq", "diff", "pwd", "true",
]);

/** The line with its single-quoted strings emptied: nothing inside them is read by the shell. */
function withoutLiterals(line: string): string {
  return line.replace(/'[^']*'/g, "''");
}

/** Splits a shell line at the operators that start another command. */
function commandsOf(line: string): string[] {
  return withoutLiterals(line).split(/&&|\|\||;|\||\n/).map(part => part.trim()).filter(part => part !== "");
}

function firstWord(command: string): string {
  // A leading `VAR=value` is an environment assignment, not the command.
  const words = command.split(/\s+/).filter(word => !/^[A-Za-z_][A-Za-z0-9_]*=/.test(word));

  return (words[0] ?? "").replace(/^.*\//, "");
}

const input = JSON.parse(await Bun.stdin.text()) as { toolCall?: { name?: string; args?: { CommandLine?: string } } };
const call = input.toolCall;
const line = call?.args?.CommandLine ?? "";

if (call?.name !== "run_command") {
  // Not a shell command: the session's own mode decides.
  console.log(JSON.stringify({ decision: "allow" }));
} else if (line !== "" && !/[`]|\$\(/.test(withoutLiterals(line)) && commandsOf(line).every(command => ALLOWED.has(firstWord(command)))) {
  console.log(JSON.stringify({ decision: "auto_approve" }));
} else {
  console.log(JSON.stringify({ decision: "ask", reason: "not a command the harness scenario uses" }));
}
