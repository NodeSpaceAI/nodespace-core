/**
 * The four places `runRep` marks a group's setup as failed.
 *
 * `withSetupFailure` is tested in isolation in runner.test.ts; these run
 * `runRep` itself, so deleting a call site (a setup turn that never reached the
 * model, came back empty, never had its tool offered, or failed its own
 * assertion) makes a test here fail. Every turn goes through a stand-in for
 * `aichat.ts`, a script that answers from a table, so no daemon and no model is
 * involved: the runner is the thing under test, and the daemon's output is the
 * only input it reads.
 */

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runRep } from "./runner.ts";
import type { EvalEnv } from "./env.ts";
import type { EvalFixture, Scenario, ScenarioResult } from "./types.ts";

let dir: string;
let aichat: string;

/** Answers `new` with a chat id and `send <chat> <message>` from the table in STUB_TURNS. */
const STUB = `
if (process.argv[2] === "new") { console.log("chat-1"); process.exit(0); }
const table = JSON.parse(process.env.STUB_TURNS ?? "{}");
const out = table[process.argv[4]];
if (out === undefined) { console.error("no stub for: " + process.argv[4]); process.exit(1); }
if (out === "FAIL") { console.error("daemon unreachable"); process.exit(1); }
console.log(out);
`;

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "runner-call-sites-"));
  aichat = join(dir, "aichat-stub.ts");
  writeFileSync(aichat, STUB);
});

afterAll(() => rmSync(dir, { recursive: true, force: true }));

const env = (): EvalEnv => ({
  nsBin: "/nonexistent/nodespace",
  socket: "/nonexistent.sock",
  log: "/nonexistent.log",
  model: "stub",
  timeoutMs: 1000,
  aichat,
});

const SETUP: Scenario = { id: "11a", scenario: "setup", prompt: "set it up", setup: true };
const NEXT: Scenario = { id: "11b", scenario: "depends on setup", prompt: "use it" };

const GOOD = "[tools offered] create_node\n[tool] create_node\nassistant> done";

function fixture(opts: { setupPasses: boolean }): EvalFixture {
  return {
    name: "call-sites",
    description: "runRep call sites",
    groups: [[SETUP, NEXT]],
    score: (scenario) =>
      scenario.id === "11a" && !opts.setupPasses
        ? { passed: false, failure: "the setup assertion failed" }
        : { passed: true },
    requiredTools: (scenario) => (scenario.id === "11a" ? ["create_node"] : []),
  };
}

function run(
  setupTurn: string,
  opts: { setupPasses?: boolean } = {},
): { setup: ScenarioResult; next: ScenarioResult } {
  process.env.STUB_TURNS = JSON.stringify({
    [SETUP.prompt]: setupTurn,
    [NEXT.prompt]: GOOD,
  });
  const results = runRep(fixture({ setupPasses: opts.setupPasses ?? true }), env());
  expect(results.map((r) => r.id)).toEqual(["11a", "11b"]);
  return { setup: results[0]!, next: results[1]! };
}

describe("runRep: a setup turn that established nothing excludes the scenarios after it", () => {
  test("control: a setup turn that passes leaves the next scenario scored", () => {
    const { setup, next } = run(GOOD);
    expect(setup.passed).toBe(true);
    expect(next.excludedAsSetupFailed).toBeUndefined();
    expect(next.passed).toBe(true);
  });

  test("a setup turn that never reached the model", () => {
    const { setup, next } = run("FAIL");
    expect(setup.excludedAsSendFailed).toBe(true);
    expect(next.excludedAsSetupFailed).toBe(true);
    expect(next.passed).toBe(false);
  });

  test("a setup turn with an empty generation", () => {
    const { setup, next } = run("[tools offered] create_node\n[empty-generation]\nassistant> ");
    expect(setup.excludedAsEmptyGeneration).toBe(true);
    expect(next.excludedAsSetupFailed).toBe(true);
  });

  test("a setup turn whose asserted tool was never offered", () => {
    const { setup, next } = run("[tools offered] search_nodes\nassistant> looked");
    expect(setup.excludedAsToolNotOffered).toBe(true);
    expect(next.excludedAsSetupFailed).toBe(true);
  });

  test("a setup turn that fails its own assertion", () => {
    const { setup, next } = run(GOOD, { setupPasses: false });
    expect(setup.passed).toBe(false);
    expect(setup.excludedAsSetup).toBe(true);
    expect(next.excludedAsSetupFailed).toBe(true);
  });
});
