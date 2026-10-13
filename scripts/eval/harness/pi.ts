/**
 * The scenario in one Pi session. The extension keeps its watch state in the
 * process, and Pi's reload loses it, so every prompt goes to one `pi --mode
 * rpc` process as a `prompt` command and the driver waits for the run to
 * settle before the next.
 *
 * The extension and the skill are the ones this checkout's installer writes,
 * into a folder under the scenario's; Pi loads them by path (`-e`, `--skill`)
 * with its own discovery off. Pi's own agent directory is not moved: it holds
 * the sign-in, and a copy of that would be a copy of a credential.
 *
 * Pi has no default model that is signed in on every machine, so the model is
 * named: `HARNESS_MODEL=<provider>/<model>`, split at the first slash.
 */

import { join } from "node:path";
import { type ScenarioEnv, type Turn, PROMPTS, installSkill, sessionEnv } from "./scenario.ts";

const TURN_TIMEOUT_MS = 12 * 60 * 1000;
const START_TIMEOUT_MS = 60 * 1000;

type Json = Record<string, unknown>;

/** Records from the child's stdout, split on LF only, as Pi's framing requires. */
async function* records(stream: AsyncIterable<Uint8Array>): AsyncGenerator<string> {
  const decoder = new TextDecoder();
  let pending = "";

  for await (const chunk of stream) {
    pending += decoder.decode(chunk, { stream: true });

    let cut = pending.indexOf("\n");

    while (cut >= 0) {
      yield pending.slice(0, cut).replace(/\r$/, "");
      pending = pending.slice(cut + 1);
      cut = pending.indexOf("\n");
    }
  }

  if (pending !== "") {
    yield pending;
  }
}

export async function pi(
  env: ScenarioEnv,
  cwd: string,
  log: { raw(text: string): void },
  play: (turn: Turn) => Promise<void>,
): Promise<void> {
  const slash = env.model?.indexOf("/") ?? -1;

  if (!env.model || slash <= 0) {
    throw new Error("Name the model Pi uses: HARNESS_MODEL=<provider>/<model>, one `pi --list-models` shows and `pi auth check --provider <provider>` reports ready.");
  }

  const agentDir = join(env.dir, "pi-agent");

  installSkill("pi", "PI_CODING_AGENT_DIR", agentDir);

  // The caller may itself be inside a harness that names a write journal: the
  // extension names this session's own for each command it runs.
  const session = sessionEnv(env);

  delete session.NODESPACE_WRITE_JOURNAL;
  delete session.PI_CODING_AGENT_DIR;

  const child = Bun.spawn(
    [
      "pi", "--mode", "rpc", "--no-session", "--no-extensions", "--no-skills",
      "-e", join(agentDir, "extensions", "nodespace", "index.ts"),
      "--skill", join(agentDir, "skills", "nodespace"),
      "--provider", env.model.slice(0, slash), "--model", env.model.slice(slash + 1),
    ],
    { cwd, env: session, stdin: "pipe", stdout: "pipe", stderr: "inherit" },
  );
  const lines = records(child.stdout);
  let counter = 0;

  /** Reads records into the log until `done` accepts one, or the deadline passes. */
  const until = async (done: (record: Json) => boolean, ms: number, what: string): Promise<Json[]> => {
    const seen: Json[] = [];
    const deadline = Date.now() + ms;

    for (;;) {
      const left = deadline - Date.now();
      const next = await Promise.race([
        lines.next(),
        Bun.sleep(Math.max(left, 0)).then(() => "timeout" as const),
      ]);

      if (next === "timeout") {
        throw new Error(`Pi did not ${what} within ${Math.round(ms / 1000)}s`);
      }

      if (next.done) {
        throw new Error(`pi exited (${String(child.exitCode)}) before it ${what}`);
      }

      log.raw(`${next.value}\n`);

      try {
        const record = JSON.parse(next.value) as Json;

        seen.push(record);

        if (done(record)) {
          return seen;
        }
      } catch {
        // Not JSON: kept in the log as it came.
      }
    }
  };

  const send = (command: Json): string => {
    const id = `ns-${++counter}`;

    child.stdin.write(`${JSON.stringify({ id, ...command })}\n`);
    child.stdin.flush();

    return id;
  };

  const turn: Turn = async name => {
    log.raw(`${JSON.stringify({ type: "scenario.prompt", name })}\n`);

    const id = send({ type: "prompt", message: PROMPTS[name] });
    const accepted = await until(r => r.type === "response" && r.id === id, START_TIMEOUT_MS, "accept the prompt");
    const response = accepted[accepted.length - 1] as Json;

    if (response.success !== true) {
      throw new Error(`Pi refused the ${name} prompt: ${String(response.error)}`);
    }

    // A prompt Pi handled itself starts no run, so no run settles.
    const disposition = String(((response.data ?? {}) as Json).disposition ?? "");
    const events =
      disposition === "handled"
        ? []
        : await until(r => r.type === "agent_settled", TURN_TIMEOUT_MS, `settle on the ${name} prompt`);
    let said = "";

    for (const record of [...accepted, ...events]) {
      const message = (record.message ?? {}) as Json;

      if (record.type === "message_end" && message.role === "assistant" && Array.isArray(message.content)) {
        for (const part of message.content as Json[]) {
          if (part.type === "text") {
            said += String(part.text ?? "");
          }
        }
      }
    }

    return said;
  };

  try {
    await play(turn);
  } finally {
    // Closing stdin is Pi's orderly shutdown: the extension's session ends.
    child.stdin.end();
    await Promise.race([child.exited, Bun.sleep(10_000)]);
    child.kill();
  }
}
