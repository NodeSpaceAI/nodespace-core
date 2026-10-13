/**
 * The scenario in one Antigravity conversation: the skill and the
 * instructions block the installer writes in the user's own config (ADR-093
 * §6), and the hooks this checkout's installer writes (ADR-093 §5).
 * `ANTIGRAVITY_BASELINE=1` leaves the hooks out, which is the baseline the
 * hooks are measured against.
 *
 * Each prompt is one `agy -p` run, the later ones continuing the conversation
 * the first reported.
 *
 * Like the Codex driver, this does not move the harness's home: Antigravity
 * keeps its sign-in there. The skill and the block must already be installed in
 * the user's own Antigravity config (`bun packages/skill/src/install.ts install
 * antigravity`); the driver stops when they are not. The hooks are installed
 * by the same installer into a folder of the scenario's, and registered from
 * the checkout's `.agents/hooks.json`: the user's global hooks file is not
 * touched.
 *
 * Print mode cannot ask for a permission, so the same file carries a
 * `PreToolUse` hook that approves a short list of commands
 * (./antigravity-approve.ts) and leaves everything else to be denied.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { REPO_ROOT } from "../env.ts";
import { type ScenarioEnv, type Turn, PROMPTS, installSkill, sessionEnv } from "./scenario.ts";

const TURN_TIMEOUT_MS = 12 * 60 * 1000;
const BLOCK_BEGIN = "<!-- nodespace:instructions:begin";

type Json = Record<string, unknown>;

export async function antigravity(
  env: ScenarioEnv,
  cwd: string,
  log: { raw(text: string): void },
  play: (turn: Turn) => Promise<void>,
): Promise<void> {
  const config = join(homedir(), ".gemini", "config");
  const rules = join(config, "AGENTS.md");

  if (!existsSync(join(config, "skills", "nodespace", "SKILL.md")) || !existsSync(rules) || !readFileSync(rules, "utf8").includes(BLOCK_BEGIN)) {
    throw new Error(
      `The NodeSpace skill and instructions block are not installed in ${config}: run \`bun packages/skill/src/install.ts install antigravity\`.`,
    );
  }

  const approve = join(REPO_ROOT, "scripts/eval/harness/antigravity-approve.ts");
  const hooks: Record<string, unknown> = {
    harness: { PreToolUse: [{ matcher: "run_command", hooks: [{ type: "command", command: `bun ${JSON.stringify(approve)}` }] }] },
  };

  if (!process.env.ANTIGRAVITY_BASELINE) {
    // The installer's own output, in a home of the scenario's: its hooks file
    // holds the one `nodespace` hook, pointing at the script it just wrote.
    const home = join(env.dir, "antigravity-home");

    installSkill("antigravity", "HOME", home);

    const written = JSON.parse(readFileSync(join(home, ".gemini", "config", "hooks.json"), "utf8")) as Json;

    if (typeof written.nodespace !== "object" || written.nodespace === null) {
      throw new Error("The installer wrote no `nodespace` hook for Antigravity.");
    }

    hooks.nodespace = written.nodespace;
  }

  mkdirSync(join(cwd, ".agents"), { recursive: true });
  writeFileSync(join(cwd, ".agents", "hooks.json"), JSON.stringify(hooks, null, 2));
  // The hooks file is the scenario's, not the fixture project's. The session
  // can edit it (it is in the checkout it works in), which is why the approver
  // is a fixture rule and not a boundary.
  const exclude = join(cwd, ".git/info/exclude");

  if (existsSync(dirname(exclude)) && !readExclude(cwd).split("\n").includes(".agents")) {
    writeFileSync(exclude, `${readExclude(cwd)}.agents\n`);
  }

  // The caller may itself be inside a harness that names a write journal.
  const session = sessionEnv(env);

  delete session.NODESPACE_WRITE_JOURNAL;

  let conversation = "";

  const turn: Turn = async name => {
    log.raw(`${JSON.stringify({ type: "scenario.prompt", name })}\n`);

    // The prompt is attached to `-p`: a flag after it would be read as the prompt.
    const argv = [
      "agy", `-p=${PROMPTS[name]}`, "--output-format", "stream-json", "--mode", "accept-edits",
      ...(env.model ? ["--model", env.model] : []),
      ...(conversation ? ["--conversation", conversation] : []),
    ];
    const run = Bun.spawn(argv, { cwd, env: session, stdin: "ignore", stdout: "pipe", stderr: "inherit", timeout: TURN_TIMEOUT_MS });
    const out = await Bun.readableStreamToText(run.stdout);
    const code = await run.exited;

    log.raw(out);

    if (code !== 0) {
      throw new Error(`agy exited with ${code} on the ${name} prompt`);
    }

    let said = "";

    for (const line of out.split("\n")) {
      try {
        const event = JSON.parse(line) as Json;
        const result = (event.result ?? {}) as Json;

        if (event.event === "init" && typeof event.conversation_id === "string") {
          conversation ||= event.conversation_id;
        }

        if (event.event === "result") {
          const denied = Array.isArray(result.denied_actions) ? result.denied_actions.length : 0;

          said = String(result.response ?? "");

          if (result.status !== "SUCCESS") {
            throw new Error(`agy ended the ${name} prompt with status ${String(result.status)}`);
          }

          // The permission system refused something the session needed: the
          // answer says so, and a run with denials is not a measurement of
          // the session's behaviour.
          if (denied > 0) {
            said += `\n[the permission system denied ${denied} action(s) on this prompt]`;
          }
        }
      } catch {
        // Not JSON: kept in the log as it came.
      }
    }

    return said;
  };

  await play(turn);
}

function readExclude(cwd: string): string {
  try {
    return readFileSync(join(cwd, ".git/info/exclude"), "utf8");
  } catch {
    return "";
  }
}
