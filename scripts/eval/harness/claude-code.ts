/**
 * The scenario in one Claude Code session: one process, the prompts on its
 * input stream, so the plugin's state lasts the session as it does in a
 * terminal.
 *
 * `plugin` loads the installed plugin. `block` loads none and puts the
 * instructions block (ADR-093 §6) in the checkout's instruction file, with
 * the skill beside it: what a harness without a plugin is given, on the same
 * model.
 */

import { cpSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { type ScenarioEnv, type Turn, PROMPTS, installSkill, sessionEnv } from "./scenario.ts";

const TURN_TIMEOUT_MS = 12 * 60 * 1000;

export async function claudeCode(
  env: ScenarioEnv,
  variant: "plugin" | "block",
  cwd: string,
  log: { raw(text: string): void },
  play: (turn: Turn) => Promise<void>,
): Promise<void> {
  const extra: string[] = [];

  if (variant === "plugin") {
    const config = join(env.dir, "claude-config");

    installSkill("claude-code", "CLAUDE_CONFIG_DIR", config);
    extra.push("--plugin-dir", join(config, "skills/nodespace"));
  } else {
    const home = join(env.dir, "codex-home");

    installSkill("codex", "CODEX_HOME", home);
    mkdirSync(join(cwd, ".claude/skills"), { recursive: true });
    cpSync(join(home, "skills/nodespace"), join(cwd, ".claude/skills/nodespace"), { recursive: true });
    cpSync(join(home, "AGENTS.md"), join(cwd, "CLAUDE.md"));
    writeFileSync(join(cwd, ".git/info/exclude"), ".claude\nCLAUDE.md\n");
  }

  const child = Bun.spawn(
    [
      "claude", "-p",
      "--input-format", "stream-json",
      "--output-format", "stream-json",
      "--verbose",
      "--include-hook-events",
      // The user's own plugins, hooks and instruction files stay out of the session.
      "--setting-sources", "project,local",
      "--permission-mode", "acceptEdits",
      "--allowedTools", "Bash,Read,Edit,Write,Grep,Glob,Skill",
      ...(env.model ? ["--model", env.model] : []),
      ...extra,
    ],
    { cwd, env: sessionEnv(env), stdin: "pipe", stdout: "pipe", stderr: "inherit" },
  );
  /** What the prompt in flight is answered through, or failed through. */
  const pending: { answer: ((text: string) => void) | null; fail: ((err: Error) => void) | null } = {
    answer: null,
    fail: null,
  };

  // A session that ends (not signed in, a flag it does not know, a crash)
  // will never answer the prompt in flight: say so now and not when that
  // prompt's timer runs out.
  void child.exited.then(code => {
    const fail = pending.fail;

    pending.answer = null;
    pending.fail = null;
    fail?.(new Error(`claude exited with ${code} before answering`));
  });

  const reading = (async () => {
    const decoder = new TextDecoder();
    let buffer = "";

    for await (const chunk of child.stdout) {
      buffer += decoder.decode(chunk, { stream: true });

      for (let end = buffer.indexOf("\n"); end >= 0; end = buffer.indexOf("\n")) {
        const line = buffer.slice(0, end);

        buffer = buffer.slice(end + 1);

        if (line.trim() === "") {
          continue;
        }

        log.raw(`${line}\n`);

        try {
          const event = JSON.parse(line) as { type?: string; result?: unknown };

          if (event.type === "result" && pending.answer) {
            const done = pending.answer;

            pending.answer = null;
            pending.fail = null;
            done(String(event.result ?? ""));
          }
        } catch {
          // Not JSON: kept in the log as it came.
        }
      }
    }
  })();

  const turn: Turn = name => {
    log.raw(`${JSON.stringify({ type: "scenario.prompt", name })}\n`);

    const said = new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`the ${name} prompt was not answered in time`)), TURN_TIMEOUT_MS);

      pending.answer = text => {
        clearTimeout(timer);
        resolve(text);
      };
      pending.fail = err => {
        clearTimeout(timer);
        reject(err);
      };
    });

    child.stdin.write(`${JSON.stringify({ type: "user", message: { role: "user", content: PROMPTS[name] } })}\n`);
    child.stdin.flush();

    return said;
  };

  try {
    await play(turn);
  } finally {
    child.stdin.end();
    await Promise.race([reading, Bun.sleep(15_000)]);
    child.kill();
  }
}
