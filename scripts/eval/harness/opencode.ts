/**
 * The scenario in one OpenCode session. One `opencode serve` process holds
 * the plugin for the whole session, as the terminal UI does, and each prompt
 * is sent to it with `opencode run --attach`: a `run` of its own would start
 * a new process, and with it a plugin that has read nothing yet.
 *
 * OpenCode's configuration folder is moved into the scenario folder, so the
 * plugin under test is the one this checkout installs.
 */

import { join } from "node:path";
import { type ScenarioEnv, type Turn, PROMPTS, installSkill, sessionEnv } from "./scenario.ts";

const PORT = 4733;

export async function opencode(
  env: ScenarioEnv,
  cwd: string,
  log: { raw(text: string): void },
  play: (turn: Turn) => Promise<void>,
): Promise<void> {
  const config = join(env.dir, "xdg-config");

  installSkill("opencode", "XDG_CONFIG_HOME", config);

  const session = sessionEnv(env, { XDG_CONFIG_HOME: config });
  const server = Bun.spawn(["opencode", "serve", "--port", String(PORT)], {
    cwd,
    env: session,
    stdout: "ignore",
    stderr: "ignore",
  });
  let sessionId = "";

  const turn: Turn = async name => {
    log.raw(`${JSON.stringify({ type: "scenario.prompt", name })}\n`);

    const run = Bun.spawn(
      [
        "opencode", "run", "--attach", `http://127.0.0.1:${PORT}`, "--dir", cwd, "--format", "json", "--auto",
        ...(env.model ? ["--model", env.model] : []),
        ...(sessionId ? ["--session", sessionId] : []),
        PROMPTS[name],
      ],
      { cwd, env: session, stdout: "pipe", stderr: "inherit" },
    );
    const out = await Bun.readableStreamToText(run.stdout);

    await run.exited;
    log.raw(out);

    let said = "";

    for (const line of out.split("\n")) {
      try {
        const event = JSON.parse(line) as { type?: string; sessionID?: string; part?: { text?: string } };

        sessionId ||= event.sessionID ?? "";

        if (event.type === "text") {
          said += event.part?.text ?? "";
        }
      } catch {
        // Not JSON.
      }
    }

    return said;
  };

  try {
    // The server takes a few seconds to listen.
    await Bun.sleep(8000);
    await play(turn);
  } finally {
    server.kill();
  }
}
