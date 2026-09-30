// Core releases ship no extensions (ADR-082). A build injects extensions
// through the NODESPACE_EXTENSIONS variable, and Tauri's `beforeBuildCommand`
// (`bun run build`) inherits the job's environment, so a stray value would
// bundle an out-of-tree module into a release. Every job in release.yml that
// runs tauri-apps/tauri-action therefore carries a step that fails the job
// when the variable is present, and nothing in the workflow may set it.
//
// The guard is checked structurally (it exists, runs before the build, has no
// `if:` that could skip it) and behaviourally: its script is extracted from
// the workflow and run under bash with the variable set, empty and unset.
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const RELEASE_WORKFLOW = join(REPO, ".github", "workflows", "release.yml");
const VARIABLE = "NODESPACE_EXTENSIONS";
const TAURI_ACTION = "tauri-apps/tauri-action";

interface Step {
  name?: string;
  uses?: string;
  run?: string;
  shell?: string;
  if?: string;
  env?: Record<string, unknown>;
}
interface Job {
  steps?: Step[];
  env?: Record<string, unknown>;
}
interface Workflow {
  env?: Record<string, unknown>;
  jobs: Record<string, Job>;
}

const workflow = Bun.YAML.parse(readFileSync(RELEASE_WORKFLOW, "utf8")) as Workflow;

/** Whether a step is the guard: it tests for the variable's presence and fails. */
function isGuard(step: Step): boolean {
  const run = step.run ?? "";
  return run.includes(`printenv ${VARIABLE}`) && /\bexit 1\b/.test(run);
}

function tauriBuildJobs(): [string, Job][] {
  return Object.entries(workflow.jobs).filter(([, job]) =>
    (job.steps ?? []).some((step) => (step.uses ?? "").startsWith(`${TAURI_ACTION}@`)),
  );
}

function runGuard(script: string, env: Record<string, string>): { exitCode: number | null; stdout: string } {
  const result = Bun.spawnSync(["bash", "-c", `set -eo pipefail\n${script}`], {
    env: { PATH: process.env.PATH ?? "", ...env },
    stdout: "pipe",
    stderr: "pipe",
  });
  return { exitCode: result.exitCode, stdout: result.stdout.toString() };
}

describe("release workflow extensions guard", () => {
  test("release.yml has the two Tauri build jobs this guard covers", () => {
    expect(tauriBuildJobs().map(([name]) => name)).toEqual(["build-tauri-macos-arm", "build-tauri"]);
  });

  for (const [jobName, job] of tauriBuildJobs()) {
    describe(jobName, () => {
      const steps = job.steps ?? [];
      const buildIndex = steps.findIndex((step) => (step.uses ?? "").startsWith(`${TAURI_ACTION}@`));
      const guardIndex = steps.findIndex(isGuard);

      test("has a guard step before the Tauri build", () => {
        expect(guardIndex).toBeGreaterThanOrEqual(0);
        expect(guardIndex).toBeLessThan(buildIndex);
      });

      test("the guard runs under bash and is never skipped", () => {
        const guard = steps[guardIndex];
        // Windows runners default to PowerShell, which has no printenv.
        expect(guard?.shell).toBe("bash");
        expect(guard?.if).toBeUndefined();
      });

      test("no step other than the guard mentions the variable in a script", () => {
        // A step between the guard and the build could export it through GITHUB_ENV.
        const mentions = steps.filter((step, i) => i !== guardIndex && (step.run ?? "").includes(VARIABLE));
        expect(mentions).toEqual([]);
      });

      test("the guard script fails when the variable is set, including empty, and passes when unset", () => {
        const script = steps[guardIndex]?.run ?? "";

        const set = runGuard(script, { [VARIABLE]: "src/extensions/index.ts" });
        expect(set.exitCode).toBe(1);
        expect(set.stdout).toContain(`::error::${VARIABLE} is set`);

        expect(runGuard(script, { [VARIABLE]: "" }).exitCode).toBe(1);
        expect(runGuard(script, {}).exitCode).toBe(0);
      });
    });
  }

  test("no workflow, job or step sets the variable in an env block", () => {
    const setters: string[] = [];
    if (workflow.env && VARIABLE in workflow.env) setters.push("workflow");
    for (const [jobName, job] of Object.entries(workflow.jobs)) {
      if (job.env && VARIABLE in job.env) setters.push(`job ${jobName}`);
      for (const step of job.steps ?? []) {
        if (step.env && VARIABLE in step.env) setters.push(`step ${jobName}/${step.name ?? "(unnamed)"}`);
      }
    }
    expect(setters).toEqual([]);
  });
});
