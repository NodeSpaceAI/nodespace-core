/**
 * The scenario in one Codex conversation. Codex runs each hook event as a
 * command of its own, so the hooks keep the session's state in a file named
 * for the conversation: each prompt is `codex exec` (the first) or
 * `codex exec resume <thread>` (the rest), and the hooks find the same
 * session each time.
 *
 * Unlike the other drivers this one does not move the harness's home into the
 * scenario folder: Codex keeps its sign-in there, and a copy of it would be a
 * copy of a credential. The hooks under test are therefore the ones installed
 * in the user's own Codex home, already trusted through `/hooks`; the driver
 * stops with a message when they are not.
 *
 *   nodespace skill install codex     then, in Codex, /hooks to trust them
 *
 * `CODEX_SANDBOX` (default `workspace-write`) is passed as `--sandbox`.
 */

import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { type ScenarioEnv, type Turn, PROMPTS, sessionEnv } from "./scenario.ts";

const TURN_TIMEOUT_MS = 12 * 60 * 1000;

type Json = Record<string, unknown>;

export async function codex(
  env: ScenarioEnv,
  cwd: string,
  log: { raw(text: string): void },
  play: (turn: Turn) => Promise<void>,
): Promise<void> {
  const home = process.env.CODEX_HOME?.trim() || join(homedir(), ".codex");

  if (!existsSync(join(home, "nodespace", "hook.ts")) || !existsSync(join(home, "hooks.json"))) {
    throw new Error(
      `The NodeSpace hooks are not installed in ${home}: run \`nodespace skill install codex\`, then trust them with /hooks in Codex.`,
    );
  }

  if (!readFileSync(join(home, "hooks.json"), "utf8").includes("nodespace/hook.ts")) {
    throw new Error(`${join(home, "hooks.json")} registers no NodeSpace hook: run \`nodespace skill install codex\`.`);
  }

  const session = sessionEnv(env);
  const sandbox = process.env.CODEX_SANDBOX?.trim() || "workspace-write";
  let threadId = "";

  const turn: Turn = async name => {
    log.raw(`${JSON.stringify({ type: "scenario.prompt", name })}\n`);

    const common = ["--json", "--skip-git-repo-check", "--sandbox", sandbox, ...(env.model ? ["--model", env.model] : [])];
    const argv = threadId
      ? ["codex", "exec", "resume", ...common, threadId, PROMPTS[name]]
      : ["codex", "exec", ...common, PROMPTS[name]];
    const run = Bun.spawn(argv, { cwd, env: session, stdout: "pipe", stderr: "inherit", timeout: TURN_TIMEOUT_MS });
    const out = await Bun.readableStreamToText(run.stdout);
    const code = await run.exited;

    log.raw(out);

    if (code !== 0) {
      throw new Error(`codex exec exited with ${code} on the ${name} prompt`);
    }

    let said = "";

    for (const line of out.split("\n")) {
      try {
        const event = JSON.parse(line) as Json;
        const item = (event.item ?? {}) as Json;

        if (event.type === "thread.started" && typeof event.thread_id === "string") {
          threadId ||= event.thread_id;
        }

        if (event.type === "item.completed" && item.type === "agent_message") {
          said += String(item.text ?? "");
        }
      } catch {
        // Not JSON: kept in the log as it came.
      }
    }

    return said;
  };

  await play(turn);
}
