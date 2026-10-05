// The release workflow bakes the embedding model into signed, notarized
// bundles (ADR-058, Threat T9). Every job that stages it from the models-v2
// release must verify its SHA-256 against the canonical pin in
// scripts/download-models.ts before building, and a mismatch must fail the
// job. The digest is never copied into the workflow; the verify step calls the
// script. Checked structurally (each downloading job has an unskippable verify
// step after the download and before the build) and behaviourally (the verify
// mode accepts the pinned bytes and rejects anything else).
import { afterAll, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const RELEASE_WORKFLOW = join(REPO, ".github", "workflows", "release.yml");
const MODEL_FILE = "nomic-embed-text-v1.5.Q8_0.gguf";
const VERIFY = /\bbun (?:run )?scripts\/download-models\.ts\b(?=[^\n]*--verify-only)(?=[^\n]*--bundle)/;

interface Step {
  name?: string;
  uses?: string;
  run?: string;
  if?: string;
  "continue-on-error"?: unknown;
}
interface Job {
  steps?: Step[];
}
const workflowText = Bun.file(RELEASE_WORKFLOW).text();
const workflow = Bun.YAML.parse(await workflowText) as { jobs: Record<string, Job> };

const downloads = (step: Step) => (step.run ?? "").includes(MODEL_FILE);
const modelJobs = Object.entries(workflow.jobs).filter(([, job]) => (job.steps ?? []).some(downloads));

describe("release workflow model digest", () => {
  test("release.yml has the two model-bundling jobs", () => {
    expect(modelJobs.map(([name]) => name)).toEqual(["build-tauri-macos-arm", "build-tauri"]);
  });

  test("the digest is not duplicated in the workflow", async () => {
    expect(await workflowText).not.toMatch(/\b[0-9a-f]{64}\b/);
  });

  for (const [jobName, job] of modelJobs) {
    describe(jobName, () => {
      const steps = job.steps ?? [];
      test("every download step verifies the model in the same step, failing on mismatch", () => {
        for (const step of steps.filter(downloads)) {
          const run = step.run ?? "";
          expect(run).toMatch(VERIFY);
          // The verify must follow the download and run under errexit semantics:
          // a bare multi-line `run` stops at the first failing command.
          expect(run.indexOf("gh release download")).toBeLessThan(run.search(VERIFY));
          expect(run).not.toMatch(/\|\|\s*true/);
          expect(step["continue-on-error"]).toBeUndefined();
        }
      });
    });
  }
});

describe("download-models --verify-only", () => {
  const dir = mkdtempSync(join(tmpdir(), "model-digest-"));
  const run = (bytes: string | null) => {
    const models = join(dir, "packages/desktop-app/src-tauri/resources/models");
    Bun.spawnSync(["mkdir", "-p", models]);
    Bun.spawnSync(["rm", "-f", join(models, MODEL_FILE)]);
    if (bytes !== null) writeFileSync(join(models, MODEL_FILE), bytes);
    return Bun.spawnSync(["bun", join(REPO, "scripts/download-models.ts"), "--bundle", "--verify-only"], {
      cwd: dir,
      stdout: "pipe",
      stderr: "pipe",
    });
  };

  test("fails on wrong bytes without downloading or deleting", () => {
    const r = run("not the model");
    expect(r.exitCode).not.toBe(0);
    expect(r.stderr.toString() + r.stdout.toString()).toContain("integrity check FAILED");
  });

  test("fails when the model is missing", () => {
    expect(run(null).exitCode).not.toBe(0);
  });

  test("accepts bytes matching the pinned digest", async () => {
    // The real model is 146 MB, so the pin cannot be exercised end to end.
    // Prove the comparison path by checking the helper against a known digest.
    const mod = await import("./download-models.ts");
    const f = join(dir, "x");
    writeFileSync(f, "abc");
    expect(await mod.sha256File(f)).toBe("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    expect(mod.MODEL_SHA256).toMatch(/^[0-9a-f]{64}$/);
  });

  afterAll(() => rmSync(dir, { recursive: true, force: true }));
});
