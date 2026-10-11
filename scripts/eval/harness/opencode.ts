/**
 * The scenario in one OpenCode session. One `opencode serve` process holds
 * the plugin for the whole session, as the terminal UI does, and each prompt
 * is sent to it with `opencode run --attach`: a `run` of its own would start
 * a new process, and with it a plugin that has read nothing yet.
 *
 * OpenCode's configuration folder is moved into the scenario folder, so the
 * plugin under test is the one this checkout installs.
 */

import { connect } from "node:net";
import { join } from "node:path";
import { type ScenarioEnv, type Turn, PROMPTS, installSkill, sessionEnv } from "./scenario.ts";

const PORT = 4733;
const SERVER_START_MS = 30_000;
const TURN_TIMEOUT_MS = 12 * 60 * 1000;

function isListening(): Promise<boolean> {
  return new Promise(resolve => {
    const socket = connect(PORT, "127.0.0.1");

    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
  });
}

export async function opencode(
  env: ScenarioEnv,
  cwd: string,
  log: { raw(text: string): void },
  play: (turn: Turn) => Promise<void>,
): Promise<void> {
  const config = join(env.dir, "xdg-config");

  installSkill("opencode", "XDG_CONFIG_HOME", config);

  // A server an interrupted run left behind holds another checkout and a
  // plugin that has already read the graph: attaching to it would measure that.
  if (await isListening()) {
    throw new Error(`Something is already listening on port ${PORT}: stop the earlier \`opencode serve\` first.`);
  }

  const session = sessionEnv(env, { XDG_CONFIG_HOME: config });
  const server = Bun.spawn(["opencode", "serve", "--port", String(PORT)], {
    cwd,
    env: session,
    stdout: "ignore",
    stderr: "inherit",
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
      { cwd, env: session, stdout: "pipe", stderr: "inherit", timeout: TURN_TIMEOUT_MS },
    );
    const out = await Bun.readableStreamToText(run.stdout);
    const code = await run.exited;

    log.raw(out);

    if (code !== 0) {
      throw new Error(`opencode run exited with ${code} on the ${name} prompt`);
    }

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
    const deadline = Date.now() + SERVER_START_MS;

    while (!(await isListening())) {
      if (server.exitCode !== null) {
        throw new Error(`opencode serve exited with ${server.exitCode} before it listened`);
      }

      if (Date.now() > deadline) {
        throw new Error("opencode serve did not start listening in time");
      }

      await Bun.sleep(500);
    }

    await play(turn);
  } finally {
    server.kill();
  }
}
