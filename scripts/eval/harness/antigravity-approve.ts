#!/usr/bin/env bun
/**
 * The scenario's permission rule for Antigravity, run as a `PreToolUse` hook
 * from the scratch checkout's `.agents/hooks.json`.
 *
 * Antigravity in print mode cannot prompt, so a tool call that needs a
 * permission is denied. This answers for the one fixture checkout: a shell
 * line is approved only when it is made of simple commands joined by `&&`,
 * `||`, `;`, `|` or a newline, every word quoted or plain, whose first words
 * are all in a short list. Anything it does not understand is left to the
 * permission system (`ask`), which denies it in print mode: the rule fails
 * closed, on a line and on input it cannot read.
 *
 * It is NOT a sandbox. `sh <file>` and `node <file>` run whatever the file
 * holds, and the agent can write files, so an approved line can run arbitrary
 * code; `touch` and `mkdir` can create paths outside the checkout, and `cat`,
 * `head` and `grep` can read any path the user can. What the rule does is keep
 * an agent from reaching for a program the scenario does not need, in a
 * fixture checkout and an isolated daemon. It is not
 * `--dangerously-skip-permissions`, and it is not to be pointed at a checkout
 * that matters.
 *
 * A `PreToolUse` hook must answer with a `decision`, so it prints one and
 * nothing else.
 */

export type Decision = { decision: "allow" | "auto_approve" | "ask"; reason?: string };

/** Commands that run as they are, whatever their arguments. */
const PLAIN = new Set(["cat", "ls", "head", "tail", "grep", "wc", "echo", "printf", "pwd", "true", "test", "mkdir", "touch"]);

/**
 * What `nodespace` may be asked, by its first word after `--json`: the
 * commands that read the graph and work a task. Not `session launch` (it
 * starts an agent that runs outside this rule), `skill install`, `database`
 * or `mcp`.
 */
const NODESPACE_SUBCOMMANDS = new Set(["node", "query", "skill", "diagnostics"]);

/** `nodespace skill <word>`: reading a skill, not installing one. */
const NODESPACE_SKILL_WORDS = new Set(["guidance", "get"]);

/** What `git` may be asked, by its first word: no `-c`, no `--exec-path`, nothing that runs a program named on the line. */
const GIT_SUBCOMMANDS = new Set(["status", "diff", "log", "add", "commit"]);

/** Arguments of an allowed git command that read a file named on the line, write one or run a program. */
const GIT_REFUSED =
  /^(-F|--file|--template|--git-dir|--work-tree|--no-index|--ext-diff|--textconv|--amend|--output|--exec|--open-files-in-pager|--upload-pack|--receive-pack)/;

/** Redirections that only discard or merge output, removed before the line is read. */
const HARMLESS_REDIRECTION = /(^|\s)(?:2>&1|&?>\s*\/dev\/null|2>\s*\/dev\/null)(?=\s|;|&|\||$)/g;

/** Characters that start something this reader does not follow, outside quotes. */
const UNFOLLOWED = /[`$\\<>(){}!]/;

/**
 * The simple commands of a shell line, each as its words, or `null` when the
 * line has anything but plain words, quotes and the joining operators.
 */
export function words(line: string): string[][] | null {
  const text = line.replace(HARMLESS_REDIRECTION, "$1");
  const commands: string[][] = [];
  let current: string[] = [];
  let word = "";
  let isWord = false;

  const endWord = () => {
    if (isWord) {
      current.push(word);
    }

    word = "";
    isWord = false;
  };
  const endCommand = () => {
    endWord();

    if (current.length > 0) {
      commands.push(current);
    }

    current = [];
  };

  for (let i = 0; i < text.length; i += 1) {
    const char = text[i] ?? "";

    if (char === "'") {
      const end = text.indexOf("'", i + 1);

      if (end < 0) {
        return null;
      }

      word += text.slice(i + 1, end);
      isWord = true;
      i = end;
    } else if (char === '"') {
      let end = i + 1;
      let quoted = "";

      // Inside double quotes the shell still reads `$` and a backtick, and `\`
      // before one of `$` `` ` `` `"` `\`. Those are followed here: an escaped
      // character is a literal, and a `$` that cannot start an expansion is one.
      // An unescaped backtick, or a `$` that could start one, is not understood.
      for (; end < text.length && text[end] !== '"'; end += 1) {
        const inside = text[end] ?? "";

        if (inside === "\\" && "$`\"\\".includes(text[end + 1] ?? "\u0000")) {
          end += 1;
          quoted += text[end] ?? "";
        } else if (inside === "`" || (inside === "$" && /[A-Za-z0-9_{([@*#?!$-]/.test(text[end + 1] ?? ""))) {
          return null;
        } else {
          quoted += inside;
        }
      }

      if (end >= text.length) {
        return null;
      }

      word += quoted;
      isWord = true;
      i = end;
    } else if (char === " " || char === "\t") {
      endWord();
    } else if (char === "\n" || char === ";") {
      endCommand();
    } else if (char === "&") {
      // `&&` joins commands; a lone `&` would run one in the background.
      if (text[i + 1] !== "&") {
        return null;
      }

      endCommand();
      i += 1;
    } else if (char === "|") {
      endCommand();

      if (text[i + 1] === "|") {
        i += 1;
      }
    } else if (UNFOLLOWED.test(char)) {
      return null;
    } else {
      word += char;
      isWord = true;
    }
  }

  endCommand();

  return commands.length > 0 ? commands : null;
}

/** Whether one simple command is one the scenario uses. */
function isAllowed(command: readonly string[]): boolean {
  const [name = "", second = "", ...rest] = command;

  // The name is looked up as written: a path, or `VAR=value` before it, is not on the list.
  // `printf -v` assigns a variable, which a later `$[...]` could evaluate.
  if (name === "printf") {
    return ![second, ...rest].some(word => /^-[a-zA-Z]*v/.test(word));
  }

  if (PLAIN.has(name)) {
    return true;
  }

  if (name === "nodespace") {
    // `--json` is a global flag, and may come first.
    const [subcommand = "", next = ""] = command.slice(1).filter((word, index) => !(index === 0 && word === "--json"));

    return NODESPACE_SUBCOMMANDS.has(subcommand) && (subcommand !== "skill" || NODESPACE_SKILL_WORDS.has(next));
  }

  if (name === "git") {
    return GIT_SUBCOMMANDS.has(second) && ![second, ...rest].some(word => GIT_REFUSED.test(word));
  }

  // A script file: `-c`, `-e` and the like would run code written on the line.
  if (name === "node" || name === "sh") {
    return second !== "" && !second.startsWith("-");
  }

  return false;
}

/** What to answer for one hook payload. Never throws. */
export function decide(input: unknown): Decision {
  const call = typeof input === "object" && input !== null ? (input as { toolCall?: unknown }).toolCall : undefined;
  const { name, args } = (typeof call === "object" && call !== null ? call : {}) as { name?: unknown; args?: unknown };

  if (typeof name !== "string" || name === "") {
    return { decision: "ask", reason: "no tool named" };
  }

  if (name !== "run_command") {
    // Not a shell command: the session's own mode decides.
    return { decision: "allow" };
  }

  const line = typeof args === "object" && args !== null ? (args as { CommandLine?: unknown }).CommandLine : undefined;
  const commands = typeof line === "string" ? words(line) : null;

  return commands?.every(isAllowed)
    ? { decision: "auto_approve" }
    : { decision: "ask", reason: "not a command line the harness scenario uses" };
}

if (import.meta.main) {
  let answer: Decision;

  try {
    answer = decide(JSON.parse(await Bun.stdin.text()));
  } catch {
    answer = { decision: "ask", reason: "unreadable hook input" };
  }

  console.log(JSON.stringify(answer));
}
