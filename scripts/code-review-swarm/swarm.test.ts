// Covers the code review swarm kit: a mock run of swarm.workflow.js with fake
// agents (fresh, resume and lean), the args builder, the unit builder and the
// preflight/teardown scripts against throwaway local git repos. No real agent,
// no network and no GitHub call happens here.
//
// DOM-free on purpose: this file runs under `bun test scripts/`.
import { afterEach, beforeEach, describe, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

// Several tests start bun, bash and git subprocesses; give them room on a busy machine.
setDefaultTimeout(30_000);

const KIT = dirname(new URL(import.meta.url).pathname);
const WORKFLOW_SRC = readFileSync(join(KIT, "swarm.workflow.js"), "utf8").replace("export const meta", "const meta");

let tmp: string;
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "swarm-test-"));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

// --- mock run of the workflow -------------------------------------------------

type Call = { phase: string; label: string; model: string; prompt: string };

async function runWorkflow(args: Record<string, unknown>) {
  const calls: Call[] = [];
  const fakeAgent = async (prompt: string, opts: { phase: string; label: string; model: string; schema?: { properties?: Record<string, unknown> } }) => {
    calls.push({ phase: opts.phase, label: opts.label, model: opts.model, prompt });
    const p = opts.schema?.properties ?? {};
    if (p.candidates && p.totals) {
      return {
        candidates: [
          { id: "F-001", repo: "core", title: "t1", severity: "P1", category: "bug", sources: ["a"] },
          { id: "F-002", repo: "sync", title: "t2", severity: "P3", category: "dead-code", sources: ["b"] },
          { id: "F-003", repo: "sync", title: "t3", severity: "P2", category: "bug", sources: ["b"] },
        ],
        totals: { raw: 5, afterDedupe: 3, overflow: 0 },
      };
    }
    if (p.candidates) return { unit: "u", lens: "l", candidates: [] };
    if (p.verdict) return { id: "x", verifier: "v", verdict: "CONFIRMED", evidence: "e" };
    if (p.decision) return { id: "x", decision: opts.label.endsWith("F-003") ? "REJECT" : "APPROVE", finalSeverity: "P2", reasoning: "r" };
    if (p.url || p.skipped) return { id: "x", url: "https://example.test/1", repo: "core", number: 1 };
    if (p.reportPath) return { reportPath: "/x/report.md", counts: { filed: 2 }, markdown: "# r" };
    return null;
  };
  const parallel = (th: Array<() => unknown>) => Promise.all(th.map((t) => Promise.resolve().then(t).catch(() => null)));
  const pipeline = async (items: unknown[], ...stages: Array<(prev: unknown, it: unknown, i: number) => unknown>) =>
    Promise.all(
      items.map(async (it, i) => {
        let prev: unknown = it;
        for (const st of stages) prev = await st(prev, it, i);
        return prev;
      }),
    );
  const logs: string[] = [];
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const fn = new AsyncFunction("args", "agent", "parallel", "pipeline", "phase", "log", WORKFLOW_SRC);
  const result = await fn(args, fakeAgent, parallel, pipeline, () => {}, (m: string) => logs.push(m));
  const seq = (phase: string) => calls.filter((c) => c.phase === phase).map((c) => `${c.label}:${c.model}`);
  return { result, calls, logs, seq };
}

const units = [
  { key: "core-p2-critical-01", repo: "core", tier: "critical", model: "opus", lenses: ["deep", "hygiene"], loc: 1 },
  { key: "sync-p3-tests-01", repo: "sync", tier: "tests", model: "sonnet", lenses: ["tests"], loc: 1 },
];
const base = { stateDir: "/state/night-2", units, maxCandidates: 60, docsRepo: "/docs/checkout", runDate: "2030-01-02" };

describe("swarm.workflow.js mock run", () => {
  test("fresh run: finders by tier and lens, then consolidate, verify, review, file, report", async () => {
    const { result, seq, calls } = await runWorkflow(base);
    expect(seq("Find")).toEqual(["find:core-p2-critical-01__deep:opus", "find:core-p2-critical-01__hygiene:sonnet", "find:sync-p3-tests-01__tests:sonnet"]);
    expect(seq("Consolidate")).toEqual(["consolidate:opus"]);
    // P0/P1 get an opus and a sonnet skeptic, P2 two sonnets, P3 one sonnet.
    expect(seq("Verify").sort()).toEqual(["verify:F-001:1:opus", "verify:F-001:2:sonnet", "verify:F-002:1:sonnet", "verify:F-003:1:sonnet", "verify:F-003:2:sonnet"]);
    // Opus chairs P0 to P2, sonnet chairs P3.
    expect(seq("Review").sort()).toEqual(["review:F-001:opus", "review:F-002:sonnet", "review:F-003:opus"]);
    // Only approved candidates are filed; the rejected one never reaches a filer.
    expect(seq("File").sort()).toEqual(["file:F-001:sonnet", "file:F-002:sonnet"]);
    expect(seq("Report")).toEqual(["report:sonnet"]);
    expect(result).toEqual({ report: "/x/report.md", counts: { filed: 2 }, failedFind: [] });
    // Phases run in order.
    const phases = calls.map((c) => c.phase);
    expect(phases.indexOf("Consolidate")).toBeGreaterThan(phases.lastIndexOf("Find"));
    expect(phases.lastIndexOf("Verify")).toBeLessThan(phases.indexOf("Report"));
    // Per candidate the stages run in order.
    const idx = (label: string) => calls.findIndex((c) => c.label === label);
    expect(idx("verify:F-001:1")).toBeLessThan(idx("review:F-001"));
    expect(idx("review:F-001")).toBeLessThan(idx("file:F-001"));
  });

  test("prompts use the parameters from args and carry the quality gate and footer rules", async () => {
    const { calls } = await runWorkflow(base);
    const find = calls.find((c) => c.phase === "Find")!;
    expect(find.prompt).toContain("/state/night-2/src-core");
    expect(find.prompt).toContain("/state/night-2/findings/core-p2-critical-01__deep.json");
    expect(find.prompt).toContain("/docs/checkout");
    expect(find.prompt).toContain("Read-only on the code");
    expect(find.prompt).toContain("report at most 12 candidates");
    const review = calls.find((c) => c.label === "review:F-001")!;
    expect(review.prompt).toContain("APPROVE only if you personally confirm it from the code");
    expect(review.prompt).toContain("/docs/checkout");
    const coreIssue = calls.find((c) => c.label === "file:F-001")!;
    expect(coreIssue.prompt).toContain("End the body with this exact last line: 🤖 Generated with [Claude Code](https://claude.com/claude-code)");
    expect(coreIssue.prompt).toContain("on 2030-01-02");
    expect(coreIssue.prompt).toContain("Do not pass --assignee");
    const syncIssue = calls.find((c) => c.label === "file:F-002")!;
    expect(syncIssue.prompt).toContain('Do NOT add any "Generated with Claude Code" or robot footer');
    expect(syncIssue.prompt).not.toContain("End the body with this exact last line");
    for (const c of calls) {
      expect(c.prompt).not.toContain("/Users/");
      expect(c.prompt).not.toContain("~/nodespace");
    }
  });

  test("without a runDate the script falls back to a plain string and does not throw", async () => {
    const { calls } = await runWorkflow({ ...base, runDate: undefined });
    expect(calls.find((c) => c.label === "file:F-001")!.prompt).toContain("on (date not given);");
  });

  test("without a docs checkout the prompts say so instead of naming a path", async () => {
    const { calls } = await runWorkflow({ ...base, docsRepo: undefined });
    const find = calls.find((c) => c.phase === "Find")!;
    expect(find.prompt).toContain("No docs checkout is available on this machine");
  });

  test("resume: finished work is skipped, unfinished work continues", async () => {
    const { result, seq, logs, calls } = await runWorkflow({
      ...base,
      reuseCandidates: true,
      done: { find: ["core-p2-critical-01__deep"], verify: ["F-001"], review: { "F-002": "APPROVE" }, filed: ["F-001"] },
    });
    expect(seq("Find")).toEqual(["find:core-p2-critical-01__hygiene:sonnet", "find:sync-p3-tests-01__tests:sonnet"]);
    expect(logs[0]).toContain("1 already done, 2 to run");
    expect(calls.find((c) => c.phase === "Consolidate")!.prompt).toContain("it already exists from an earlier run");
    // F-001 verified earlier, F-002 reviewed earlier, F-003 untouched.
    expect(seq("Verify").sort()).toEqual(["verify:F-002:1:sonnet", "verify:F-003:1:sonnet", "verify:F-003:2:sonnet"]);
    expect(seq("Review").sort()).toEqual(["review:F-001:opus", "review:F-003:opus"]);
    // F-001 is already filed, F-002 still has to be filed.
    expect(seq("File")).toEqual(["file:F-002:sonnet"]);
    expect(result.report).toBe("/x/report.md");
  });

  test("a rerun with everything done spawns only the consolidator and the reporter", async () => {
    const { seq } = await runWorkflow({
      ...base,
      reuseCandidates: true,
      done: {
        find: ["core-p2-critical-01__deep", "core-p2-critical-01__hygiene", "sync-p3-tests-01__tests"],
        verify: ["F-001", "F-002", "F-003"],
        review: { "F-001": "APPROVE", "F-002": "APPROVE", "F-003": "REJECT" },
        filed: ["F-001", "F-002"],
      },
    });
    expect(seq("Find")).toEqual([]);
    expect(seq("Verify")).toEqual([]);
    expect(seq("Review")).toEqual([]);
    expect(seq("File")).toEqual([]);
    expect(seq("Consolidate")).toEqual(["consolidate:opus"]);
    expect(seq("Report")).toEqual(["report:sonnet"]);
  });

  test("lean profile drops the hygiene pass on critical units and the tests tier", async () => {
    const { seq } = await runWorkflow({ ...base, profile: "lean" });
    expect(seq("Find")).toEqual(["find:core-p2-critical-01__deep:opus"]);
  });

  test("stops without filing when consolidation fails", async () => {
    const calls: string[] = [];
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
    const fn = new AsyncFunction("args", "agent", "parallel", "pipeline", "phase", "log", WORKFLOW_SRC);
    const agent = async (_p: string, o: { label: string }) => {
      calls.push(o.label);
      return o.label === "consolidate" ? null : { unit: "u", lens: "l", candidates: [] };
    };
    const parallel = (th: Array<() => unknown>) => Promise.all(th.map((t) => Promise.resolve().then(t)));
    const result = await fn(base, agent, parallel, async () => [], () => {}, () => {});
    expect(result.stopped).toBe("consolidation failed");
    expect(calls.some((l) => l.startsWith("file:") || l === "report")).toBe(false);
  });
});

// --- helpers for the CLI scripts ----------------------------------------------

function runBun(script: string, argv: string[], env: Record<string, string>) {
  return spawnSync("bun", [join(KIT, script), ...argv], { encoding: "utf8", env: { ...process.env, HOME: tmp, ...env } });
}

const put = (root: string, rel: string, lines = 3) => {
  const p = join(root, rel);
  mkdirSync(dirname(p), { recursive: true });
  writeFileSync(p, Array.from({ length: lines }, (_, i) => `// line ${i}`).join("\n"));
};

// --- make-units ----------------------------------------------------------------

describe("make-units.ts", () => {
  test("reads the pinned snapshots, assigns tiers and parts, and filters by part", () => {
    const dir = join(tmp, "night");
    const core = join(dir, "src-core");
    const sync = join(dir, "src-sync");
    put(core, "packages/core/src/db/store.rs");
    put(core, "packages/core/src/services/node.rs");
    put(core, "packages/daemon/src/main.rs");
    put(core, "packages/agent/src/chat.ts");
    put(core, "packages/desktop-app/src/lib/components/Button.svelte");
    put(core, "packages/desktop-app/src/lib/stores/nodes.ts");
    put(core, "packages/core/tests/db_test.rs");
    put(core, "packages/desktop-app/src/tests/skipped.test.ts"); // frontend unit tests are not reviewed
    put(core, "packages/core/src/db/generated/skip.rs"); // generated code is not reviewed
    put(core, "scripts/build-pkg.sh");
    put(core, "packages/core/src/models/node.rs"); // a source directory named models is reviewed
    put(core, "packages/core/src/ops/entity_types_block.rs"); // block/lock in a name is not a lockfile
    put(core, "scripts/gate-lock.ts");
    put(core, "packages/core/Cargo.lock"); // real lockfiles stay out (extension not collected anyway)
    put(core, "scripts/pkg-resources/postinstall"); // extension-less root-run installer scripts
    put(core, "scripts/pkg-resources/component.plist");
    put(sync, "nodespaced-sync/src/lib.rs");
    put(sync, "nodespaced-sync/tests/e2e.rs");
    put(sync, "examples/demo.rs");
    // A live checkout must never be read: files only in a decoy tree must not appear.
    put(join(tmp, "live-core"), "packages/core/src/db/live_only.rs");

    const all = runBun("make-units.ts", ["--part", "0"], { SWARM_DIR: dir, CORE_REPO: join(tmp, "live-core") });
    expect(all.status).toBe(0);
    const units = JSON.parse(readFileSync(join(dir, "units.json"), "utf8"));
    const where = (f: string) => units.find((u: { repo: string; files: string[] }) => u.files.includes(f));
    expect(where("packages/core/src/db/store.rs")).toMatchObject({ repo: "core", part: 1, tier: "critical", model: "opus", lenses: ["deep", "hygiene"] });
    expect(where("packages/core/src/services/node.rs").part).toBe(1);
    expect(where("packages/daemon/src/main.rs")).toMatchObject({ part: 2, tier: "critical" });
    expect(where("packages/agent/src/chat.ts")).toMatchObject({ part: 2, tier: "high", model: "sonnet", lenses: ["deep"] });
    expect(where("packages/desktop-app/src/lib/components/Button.svelte")).toMatchObject({ part: 3, tier: "low", lenses: ["hygiene"] });
    expect(where("packages/desktop-app/src/lib/stores/nodes.ts")).toMatchObject({ part: 3, tier: "high" });
    expect(where("packages/core/tests/db_test.rs")).toMatchObject({ part: 3, tier: "tests", lenses: ["tests"] });
    expect(where("scripts/build-pkg.sh")).toMatchObject({ part: 2, tier: "infra", model: "opus", lenses: ["deep"] });
    expect(where("packages/core/src/models/node.rs")).toMatchObject({ repo: "core", part: 2 });
    expect(where("packages/core/src/ops/entity_types_block.rs")).toMatchObject({ repo: "core", part: 2 });
    expect(where("scripts/gate-lock.ts")).toMatchObject({ repo: "core", tier: "infra" });
    expect(where("scripts/pkg-resources/postinstall")).toMatchObject({ repo: "core", tier: "infra" });
    expect(where("scripts/pkg-resources/component.plist")).toBeUndefined();
    expect(where("nodespaced-sync/src/lib.rs")).toMatchObject({ repo: "sync", part: 1, tier: "critical" });
    expect(where("examples/demo.rs")).toMatchObject({ repo: "sync", part: 1, tier: "high" });
    expect(where("nodespaced-sync/tests/e2e.rs")).toMatchObject({ repo: "sync", part: 3, tier: "tests" });
    expect(where("packages/desktop-app/src/tests/skipped.test.ts")).toBeUndefined();
    expect(where("packages/core/src/db/generated/skip.rs")).toBeUndefined();
    expect(where("packages/core/src/db/live_only.rs")).toBeUndefined();
    expect(units.every((u: { key: string; repo: string; part: number }) => u.key.startsWith(`${u.repo}-p${u.part}-`))).toBe(true);

    const two = runBun("make-units.ts", ["--part", "2"], { SWARM_DIR: dir });
    expect(two.status).toBe(0);
    const part2 = JSON.parse(readFileSync(join(dir, "units.json"), "utf8"));
    expect(part2.length).toBeGreaterThan(0);
    expect(part2.every((u: { part: number }) => u.part === 2)).toBe(true);
    const plan = JSON.parse(readFileSync(join(dir, "plan-units.json"), "utf8"));
    expect(plan.map((u: { key: string }) => u.key)).toEqual(part2.map((u: { key: string }) => u.key));
    expect(plan[0].files).toBeUndefined();
  });

  test("splits a tier into several units when the line cap is exceeded", () => {
    const dir = join(tmp, "night");
    const core = join(dir, "src-core");
    for (let i = 0; i < 3; i++) put(core, `packages/daemon/src/m${i}.rs`, 3000);
    mkdirSync(join(dir, "src-sync"), { recursive: true });
    expect(runBun("make-units.ts", ["--part", "2"], { SWARM_DIR: dir }).status).toBe(0);
    const units = JSON.parse(readFileSync(join(dir, "units.json"), "utf8"));
    expect(units.map((u: { key: string; loc: number }) => [u.key, u.loc])).toEqual([
      ["core-p2-critical-01", 6000],
      ["core-p2-critical-02", 3000],
    ]);
  });

  test("defaults the state dir to one directory per night under HOME", () => {
    const night = join(tmp, "nodespace-swarm", "night-3");
    put(join(night, "src-core"), "packages/nodespace-types/src/a.ts");
    mkdirSync(join(night, "src-sync"), { recursive: true });
    const r = runBun("make-units.ts", ["--part", "3"], {});
    expect(r.status).toBe(0);
    expect(existsSync(join(night, "units.json"))).toBe(true);
  });

  test("fails with a clear message when preflight has not run", () => {
    const r = runBun("make-units.ts", ["--part", "1"], { SWARM_DIR: join(tmp, "empty") });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain("Run preflight.sh first");
  });

  test("rejects an invalid part", () => {
    const r = runBun("make-units.ts", ["--part", "9"], { SWARM_DIR: join(tmp, "x") });
    expect(r.status).not.toBe(0);
  });
});

// --- make-args -------------------------------------------------------------------

describe("make-args.ts", () => {
  const plan = [
    { key: "core-p1-critical-01", repo: "core", tier: "critical", part: 1, model: "opus", lenses: ["deep", "hygiene"], loc: 5 },
    { key: "core-p2-high-01", repo: "core", tier: "high", part: 2, model: "sonnet", lenses: ["deep"], loc: 5 },
    { key: "sync-p3-tests-01", repo: "sync", tier: "tests", part: 3, model: "sonnet", lenses: ["tests"], loc: 5 },
  ];
  const stateDir = () => {
    const dir = join(tmp, "night");
    for (const d of ["findings", "verdicts", "review", "filed"]) mkdirSync(join(dir, d), { recursive: true });
    writeFileSync(join(dir, "plan-units.json"), JSON.stringify(plan));
    return dir;
  };
  const make = (dir: string, argv: string[], env: Record<string, string> = {}) => {
    const r = runBun("make-args.ts", argv, { SWARM_DIR: dir, ...env });
    expect(r.status).toBe(0);
    expect(JSON.parse(readFileSync(join(dir, "args.json"), "utf8"))).toEqual(JSON.parse(r.stdout));
    return JSON.parse(r.stdout);
  };

  test("keeps only the units of the requested part and starts empty", () => {
    const dir = stateDir();
    const a = make(dir, ["--part", "2"]);
    expect(a.units.map((u: { key: string }) => u.key)).toEqual(["core-p2-high-01"]);
    expect(a).toMatchObject({ stateDir: dir, part: 2, profile: "full", maxCandidates: 60, reuseCandidates: false });
    expect(a.done).toEqual({ find: [], verify: [], review: {}, filed: [] });
    expect(make(dir, ["--part", "0"]).units).toHaveLength(3);
    expect(make(dir, ["--part", "1", "--profile", "lean", "--max", "40"])).toMatchObject({ profile: "lean", maxCandidates: 40 });
  });

  test("builds the done sets from the state directory", () => {
    const dir = stateDir();
    writeFileSync(join(dir, "findings", "core-p1-critical-01__deep.json"), "{}");
    writeFileSync(join(dir, "findings", "notes.txt"), "ignored");
    writeFileSync(join(dir, "candidates.json"), JSON.stringify({ candidates: [{ id: "F-001", severity: "P1" }, { id: "F-002", severity: "P3" }, { id: "F-003", severity: "P2" }] }));
    // F-001 needs both verdicts, F-002 (P3) needs one, F-003 has only one of two.
    writeFileSync(join(dir, "verdicts", "F-001__1.json"), "{}");
    writeFileSync(join(dir, "verdicts", "F-001__2.json"), "{}");
    writeFileSync(join(dir, "verdicts", "F-002__1.json"), "{}");
    writeFileSync(join(dir, "verdicts", "F-003__1.json"), "{}");
    writeFileSync(join(dir, "review", "F-001.json"), JSON.stringify({ decision: "APPROVE" }));
    writeFileSync(join(dir, "review", "F-002.json"), "{ not json");
    writeFileSync(join(dir, "filed", "F-001.json"), "{}");
    const a = make(dir, ["--part", "1"]);
    expect(a.done.find).toEqual(["core-p1-critical-01__deep"]);
    expect(a.done.verify.sort()).toEqual(["F-001", "F-002"]);
    expect(a.done.review).toEqual({ "F-001": "APPROVE" });
    expect(a.done.filed).toEqual(["F-001"]);
    // The hygiene finder file is still missing, so the candidates are not reused yet.
    expect(a.reuseCandidates).toBe(false);
    writeFileSync(join(dir, "findings", "core-p1-critical-01__hygiene.json"), "{}");
    expect(make(dir, ["--part", "1"]).reuseCandidates).toBe(true);
  });

  test("lean profile needs no hygiene file on critical units to reuse the candidates", () => {
    const dir = stateDir();
    writeFileSync(join(dir, "findings", "core-p1-critical-01__deep.json"), "{}");
    writeFileSync(join(dir, "candidates.json"), JSON.stringify({ candidates: [] }));
    expect(make(dir, ["--part", "1"]).reuseCandidates).toBe(false);
    expect(make(dir, ["--part", "1", "--profile", "lean"]).reuseCandidates).toBe(true);
  });

  test("passes the repository paths through, from the environment or the saved config", () => {
    const dir = stateDir();
    writeFileSync(join(dir, "config.json"), JSON.stringify({ coreRepo: "/saved/core", syncRepo: "/saved/sync", docsRepo: "/saved/docs" }));
    expect(make(dir, ["--part", "1"])).toMatchObject({ coreRepo: "/saved/core", syncRepo: "/saved/sync", docsRepo: "/saved/docs" });
    expect(make(dir, ["--part", "1"], { DOCS_REPO: "/env/docs" }).docsRepo).toBe("/env/docs");
    expect(make(dir, ["--part", "1"]).runDate).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  });

  test("refuses an empty selection, for example a plan built for another part", () => {
    const dir = stateDir();
    writeFileSync(join(dir, "plan-units.json"), JSON.stringify(plan.filter((u) => u.part === 3)));
    const r = runBun("make-args.ts", ["--part", "2"], { SWARM_DIR: dir });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain("has no units for part 2");
  });

  test("tells the user to run make-units first", () => {
    const r = runBun("make-args.ts", ["--part", "1"], { SWARM_DIR: join(tmp, "none") });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain("make-units.ts");
  });
});

// --- preflight.sh and teardown.sh --------------------------------------------------

const git = (cwd: string, ...a: string[]) => {
  const r = spawnSync("git", ["-C", cwd, "-c", "user.email=t@example.test", "-c", "user.name=t", ...a], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(`git ${a.join(" ")}: ${r.stderr}`);
  return r.stdout.trim();
};

function makeRepoWithOrigin(name: string) {
  const origin = join(tmp, `${name}-origin.git`);
  const work = join(tmp, name);
  spawnSync("git", ["init", "-q", "--bare", "-b", "main", origin]);
  spawnSync("git", ["clone", "-q", origin, work]);
  writeFileSync(join(work, "a.txt"), name);
  git(work, "add", ".");
  git(work, "commit", "-q", "-m", "init");
  git(work, "push", "-q", "origin", "HEAD:main");
  return work;
}

const haveTools = ["git", "jq", "bash"].every((t) => spawnSync("which", [t]).status === 0);

describe.skipIf(!haveTools)("preflight.sh and teardown.sh", () => {
  function fakeGh() {
    const bin = join(tmp, "bin");
    mkdirSync(bin, { recursive: true });
    // Prints one JSON object per issue, like `gh issue list -q '.[]|{...}'`.
    writeFileSync(join(bin, "gh"), '#!/bin/bash\necho "{\\"repo\\":\\"x\\",\\"number\\":1,\\"title\\":\\"t\\",\\"state\\":\\"OPEN\\"}"\necho "$@" >> "$GH_LOG"\n');
    chmodSync(join(bin, "gh"), 0o755);
    return bin;
  }
  const preflight = (env: Record<string, string>, argv: string[] = ["--part", "2"]) =>
    spawnSync("bash", [join(KIT, "preflight.sh"), ...argv], { encoding: "utf8", env: { ...process.env, HOME: tmp, PATH: `${fakeGh()}:${process.env.PATH}`, GH_LOG: join(tmp, "gh.log"), ...env } });

  test("pins both repos, records the shas, lists issues of three repos and is idempotent", () => {
    const core = makeRepoWithOrigin("core");
    const sync = makeRepoWithOrigin("sync");
    const dir = join(tmp, "night");
    const env = { SWARM_DIR: dir, CORE_REPO: core, SYNC_REPO: sync, DOCS_REPO: "" };
    const first = preflight(env);
    expect(first.status).toBe(0);
    const shas = JSON.parse(readFileSync(join(dir, "shas.json"), "utf8"));
    expect(shas.core).toBe(git(core, "rev-parse", "origin/main"));
    expect(shas.sync).toBe(git(sync, "rev-parse", "origin/main"));
    expect(readFileSync(join(dir, "src-core", "a.txt"), "utf8")).toBe("core");
    expect(readFileSync(join(dir, "src-sync", "a.txt"), "utf8")).toBe("sync");
    for (const d of ["findings", "verdicts", "review", "filed", "scratch", "logs"]) expect(existsSync(join(dir, d))).toBe(true);
    expect(JSON.parse(readFileSync(join(dir, "existing-issues.json"), "utf8"))).toHaveLength(3);
    const ghCalls = readFileSync(join(tmp, "gh.log"), "utf8");
    for (const r of ["core", "sync", "cloud"]) expect(ghCalls).toContain(`NodeSpaceAI/nodespace-${r}`);
    expect(ghCalls).toContain("state=all");
    expect(ghCalls).toContain("--paginate");
    expect(ghCalls).not.toContain("--limit");
    expect(JSON.parse(readFileSync(join(dir, "config.json"), "utf8"))).toMatchObject({ coreRepo: core, syncRepo: sync });

    // A new commit on origin must NOT move the snapshots on a re-run.
    writeFileSync(join(core, "b.txt"), "b");
    git(core, "add", ".");
    git(core, "commit", "-q", "-m", "second");
    git(core, "push", "-q", "origin", "HEAD:main");
    writeFileSync(join(dir, "findings", "keep.json"), "{}");
    rmSync(join(tmp, "gh.log"));
    const second = preflight(env);
    expect(second.status).toBe(0);
    expect(JSON.parse(readFileSync(join(dir, "shas.json"), "utf8"))).toEqual(shas);
    expect(existsSync(join(dir, "src-core", "b.txt"))).toBe(false);
    expect(existsSync(join(dir, "findings", "keep.json"))).toBe(true);
    expect(existsSync(join(tmp, "gh.log"))).toBe(false);

    // Teardown removes the snapshots and keeps the results.
    const td = spawnSync("bash", [join(KIT, "teardown.sh")], { encoding: "utf8", env: { ...process.env, HOME: tmp, SWARM_DIR: dir } });
    expect(td.status).toBe(0);
    expect(existsSync(join(dir, "src-core"))).toBe(false);
    expect(existsSync(join(dir, "src-sync"))).toBe(false);
    expect(existsSync(join(dir, "shas.json"))).toBe(true);
    expect(existsSync(join(dir, "findings", "keep.json"))).toBe(true);
    expect(git(core, "worktree", "list")).not.toContain("src-core");
  });

  test("uses one directory per night under HOME when SWARM_DIR is not set", () => {
    const core = makeRepoWithOrigin("core");
    const sync = makeRepoWithOrigin("sync");
    expect(preflight({ CORE_REPO: core, SYNC_REPO: sync }, ["--part", "3"]).status).toBe(0);
    expect(existsSync(join(tmp, "nodespace-swarm", "night-3", "src-core"))).toBe(true);
    expect(existsSync(join(tmp, "nodespace-swarm", "night-2"))).toBe(false);
  });

  test("finds the sync checkout under HOME/nodespace and next to core as a fallback", () => {
    const core = makeRepoWithOrigin("core");
    const home = join(tmp, "nodespace");
    mkdirSync(home, { recursive: true });
    const sync = makeRepoWithOrigin(["nodespace", "sync"].join("-"));
    const r = preflight({ CORE_REPO: core, SWARM_DIR: join(tmp, "n1") });
    expect(r.status).toBe(0); // the sibling directory next to core is found
    expect(JSON.parse(readFileSync(join(tmp, "n1", "config.json"), "utf8")).syncRepo).toBe(sync);
  });

  test("fails with a clear message when the sync checkout is missing", () => {
    const core = makeRepoWithOrigin("core");
    const r = preflight({ CORE_REPO: core, SWARM_DIR: join(tmp, "n2") });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain("Cannot find the sync checkout");
    expect(existsSync(join(tmp, "n2", "src-core"))).toBe(false);
  });

  test("rejects an invalid part", () => {
    const r = preflight({}, ["--part", "7"]);
    expect(r.status).toBe(2);
  });
});
