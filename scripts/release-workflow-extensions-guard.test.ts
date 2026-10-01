// Core releases ship no extensions (ADR-082). A build injects extensions
// through two variables. NODESPACE_EXTENSIONS names a frontend module, and
// Tauri's `beforeBuildCommand` (`bun run build`) inherits the job's
// environment, so a stray value would bundle an out-of-tree module into a
// release. NODESPACE_SKILL_EXTENSIONS names a directory of agent guidance that
// `bun run build:skill` stages into the skill. Every job in release.yml that
// runs `bun run build:skill` or tauri-apps/tauri-action therefore carries a
// step that fails the job when either variable is present, before the skill is
// staged, and nothing in the workflow may set them.
//
// The guard is checked structurally (it exists, runs before the build, has no
// `if:` that could skip it) and behaviourally: its script is extracted from
// the workflow and run under bash with each variable set, empty and unset.
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const RELEASE_WORKFLOW = join(REPO, ".github", "workflows", "release.yml");
const VARIABLES = ["NODESPACE_EXTENSIONS", "NODESPACE_SKILL_EXTENSIONS"];
const TAURI_ACTION = "tauri-apps/tauri-action";
// A step that stages the skill:
// - `build:skill`, or `tauri:build`, which runs it in every package that has
//   it, from any `--cwd`;
// - a root script that runs one of them (`build`, `build:macos`,
//   `build:windows`, `build:windows:quick`). With a `--cwd`, `build` is another
//   package's build, which does not stage the skill;
// - the script `build:skill` runs.
// Not `bun build`, Bun's bundler, and not `bunx tauri build`, whose
// beforeBuildCommand is the frontend's `build`.
const SKILL_BUILD =
  /\bbun run (?:--cwd \S+ )?(?:build:skill|tauri:build)(?=\s|$)|\bbun run (?:build|build:macos|build:windows|build:windows:quick)(?=\s|$)|\bbun (?:run )?scripts\/build-skill\.ts(?=\s|$)/m;

interface Step {
  name?: string;
  uses?: string;
  run?: string;
  shell?: string;
  if?: string;
  "continue-on-error"?: unknown;
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

/** Whether a step is the guard: it tests for each variable's presence and fails. */
function isGuard(step: Step): boolean {
  const run = step.run ?? "";
  return VARIABLES.every((variable) => run.includes(`printenv ${variable}`)) && /\bexit 1\b/.test(run);
}

function tauriBuildJobs(): [string, Job][] {
  return Object.entries(workflow.jobs).filter(([, job]) =>
    (job.steps ?? []).some((step) => (step.uses ?? "").startsWith(`${TAURI_ACTION}@`)),
  );
}

function skillBuildJobs(): [string, Job][] {
  return Object.entries(workflow.jobs).filter(([, job]) =>
    (job.steps ?? []).some((step) => SKILL_BUILD.test(step.run ?? "")),
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

/** The guard must run under bash (Windows runners default to PowerShell, which has no printenv), never be skipped, and stop the job when it fails. */
function expectGuardCannotBeSkipped(guard: Step | undefined): void {
  expect(guard?.shell).toBe("bash");
  expect(guard?.if).toBeUndefined();
  expect(guard?.["continue-on-error"]).toBeUndefined();
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

      test("the guard runs under bash, is never skipped and its failure stops the job", () => {
        expectGuardCannotBeSkipped(steps[guardIndex]);
      });

      test("no step other than the guard mentions either variable in a script", () => {
        // A step between the guard and the build could export one through GITHUB_ENV.
        const mentions = steps.filter(
          (step, i) => i !== guardIndex && VARIABLES.some((variable) => (step.run ?? "").includes(variable)),
        );
        expect(mentions).toEqual([]);
      });

      for (const variable of VARIABLES) {
        test(`the guard script fails when ${variable} is set, including empty, and passes when unset`, () => {
          const script = steps[guardIndex]?.run ?? "";

          const set = runGuard(script, { [variable]: "some/value" });
          expect(set.exitCode).toBe(1);
          expect(set.stdout).toContain(`::error::${variable} is set`);

          expect(runGuard(script, { [variable]: "" }).exitCode).toBe(1);
          expect(runGuard(script, {}).exitCode).toBe(0);
        });
      }
    });
  }

  test("recognizes every command that stages the skill, and no other", () => {
    for (const command of [
      "bun run build:skill",
      "bun run build",
      "bun run build && echo done",
      "bun run build:windows",
      "bun run build:windows:quick",
      "bun run build:macos",
      "bun run tauri:build",
      "cd packages/desktop-app && bun run tauri:build",
      "bun run --cwd packages/desktop-app tauri:build",
      "bun run --cwd ../.. build:skill",
      "bun run scripts/build-skill.ts --target x86_64-pc-windows-msvc",
      "bun scripts/build-skill.ts",
    ]) {
      expect(SKILL_BUILD.test(command), command).toBe(true);
    }
    for (const command of [
      "bun run build:sidecars",
      "bun build --compile src/install.ts --outfile x",
      "bun run --cwd packages/desktop-app build",
      "bun run build:skill-repo",
      "bun run --cwd packages/skill build",
      "bunx tauri build",
    ]) {
      expect(SKILL_BUILD.test(command), command).toBe(false);
    }
  });

  test("release.yml has the two jobs that stage the skill this guard covers", () => {
    expect(skillBuildJobs().map(([name]) => name)).toEqual(["build-tauri-macos-arm", "build-tauri"]);
  });

  for (const [jobName, job] of skillBuildJobs()) {
    describe(`${jobName} (skill staging)`, () => {
      const steps = job.steps ?? [];
      const skillIndex = steps.findIndex((step) => SKILL_BUILD.test(step.run ?? ""));
      const guardIndex = steps.findIndex(isGuard);

      test("runs the guard before `bun run build:skill`", () => {
        expect(guardIndex).toBeGreaterThanOrEqual(0);
        expect(guardIndex).toBeLessThan(skillIndex);
      });

      test("the guard runs under bash, is never skipped and its failure stops the job", () => {
        expectGuardCannotBeSkipped(steps[guardIndex]);
      });
    });
  }

  test("no workflow, job or step sets either variable in an env block", () => {
    const setters: string[] = [];
    const sets = (env?: Record<string, unknown>) => VARIABLES.some((variable) => env && variable in env);
    if (sets(workflow.env)) setters.push("workflow");
    for (const [jobName, job] of Object.entries(workflow.jobs)) {
      if (sets(job.env)) setters.push(`job ${jobName}`);
      for (const step of job.steps ?? []) {
        if (sets(step.env)) setters.push(`step ${jobName}/${step.name ?? "(unnamed)"}`);
      }
    }
    expect(setters).toEqual([]);
  });
});
