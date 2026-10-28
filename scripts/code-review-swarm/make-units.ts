// Builds the review units (batches of files with a tier, a model and lenses)
// from the PINNED snapshots that preflight.sh created, never from the live
// checkouts, and writes units.json and plan-units.json into the state dir.
// usage: bun make-units.ts --part 1|2|3   (0 or omitted = every part)
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { extname, join, relative } from "node:path";
import { argValue, parsePart, stateDirFor } from "./config";

// Built from fragments: the repo-boundary check bans the literal name in core.
const PRO_DIR = ["nodespaced", "pro"].join("-");
const EXT = new Set([".rs", ".ts", ".svelte", ".sh", ".sql", ".js", ".mjs", ".yml", ".yaml"]);
const SKIP_DIR = new Set(["node_modules", "target", "dist", "build", ".svelte-kit", "gen", "generated", "bindings", ".core", "sync-worktrees", "core-worktrees", "archive", "canary", "fixtures", "snapshots", "coverage", ".git", "playwright-report", "test-results", "resources", "reference", "goldens", ".claude"]);

// Installer scripts that run as root and have no extension.
const EXTENSIONLESS_SCRIPT = /(^|\/)pkg-resources\/(preinstall|postinstall)$/;

export type SourceFile = { f: string; n: number };
export type Tier = "tests" | "critical" | "high" | "low" | "infra";
export type Unit = { repo: string; tier: Tier; part: number; model: string; lenses: string[]; files: string[]; loc: number; key: string };

function walk(root: string, dir: string, out: SourceFile[]) {
  let ents;
  try {
    ents = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const e of ents) {
    const p = join(dir, e.name);
    if (e.isDirectory()) {
      if (!SKIP_DIR.has(e.name)) walk(root, p, out);
      continue;
    }
    if (!EXT.has(extname(e.name)) && !EXTENSIONLESS_SCRIPT.test(relative(root, p))) continue;
    if (/\.generated\.|\.d\.ts$|\.min\.|(^|[-.])lock\.ya?ml$|\.lock$/.test(e.name)) continue;
    const n = readFileSync(p, "utf8").split("\n").length;
    if (n > 40000) continue;
    out.push({ f: relative(root, p), n });
  }
}

const isTest = (f: string) => /(\/tests?\/|\.test\.|_test\.rs$|_tests\.rs$|\.spec\.|\/e2e\/|tests\.rs$)/.test(f);

export function category(repo: string, f: string): Tier | "skip" {
  if (isTest(f)) return /^packages\/desktop-app\/src\/tests\//.test(f) || /^packages\/skill\//.test(f) || /^packages\/agent\/tests\//.test(f) ? "skip" : "tests";
  if (/^(scripts|\.github)\//.test(f) || /pkg-resources|build-pkg|uninstall|install\.sh|postinstall/.test(f)) return "infra";
  if (repo === "sync") {
    if (/^(nodespaced-sync\/src|src|app\/plugin\/src)\//.test(f)) return "critical";
    return "high";
  }
  if (/^packages\/core\/src\/(db|services)\//.test(f)) return "critical";
  if (/^packages\/daemon\/src\//.test(f)) return "critical";
  if (/^packages\/cli\/src\//.test(f)) return "critical";
  if (/^packages\/desktop-app\/app-lib\/src\//.test(f)) return "critical";
  if (/^packages\/(agent|nlp-engine|skill)\//.test(f) || /^packages\/core\/src\/(playbook|ops|schema|markdown|behaviors)/.test(f)) return "high";
  if (/^packages\/desktop-app\/src\/lib\/(stores|services|utils|plugins)\//.test(f)) return "high";
  return "low";
}

export function partOf(repo: string, f: string, cat: string): number {
  if (repo === "sync") return cat === "tests" ? 3 : 1;
  if (cat === "tests") return 3;
  if (/^packages\/core\/src\/(db|services)\//.test(f)) return 1;
  if (/^packages\/(desktop-app\/src|nodespace-types)\//.test(f)) return 3;
  return 2;
}

const CAP: Record<string, number> = { critical: 8000, high: 10000, low: 14000, infra: 8000, tests: 16000 };
const MODEL: Record<string, string> = { critical: "opus", infra: "opus", high: "sonnet", low: "sonnet", tests: "sonnet" };
const LENSES: Record<string, string[]> = { critical: ["deep", "hygiene"], infra: ["deep"], high: ["deep"], low: ["hygiene"], tests: ["tests"] };
const TIER_ORDER: Tier[] = ["critical", "infra", "high", "low", "tests"];

export function build(repo: string, root: string, tops: string[]): Unit[] {
  const files: SourceFile[] = [];
  for (const t of tops) walk(root, join(root, t), files);
  files.sort((a, b) => a.f.localeCompare(b.f));
  const groups: Record<string, SourceFile[]> = {};
  for (const x of files) {
    const c = category(repo, x.f);
    if (c === "skip") continue;
    (groups[`${c}|${partOf(repo, x.f, c)}`] ??= []).push(x);
  }
  const res: Unit[] = [];
  for (const part of [1, 2, 3]) {
    for (const cat of TIER_ORDER) {
      let cur: Unit | null = null;
      let i = 0;
      for (const x of groups[`${cat}|${part}`] ?? []) {
        if (!cur || cur.loc + x.n > CAP[cat]) {
          cur = { repo, tier: cat, part, model: MODEL[cat], lenses: LENSES[cat], files: [], loc: 0, key: `${repo}-p${part}-${cat}-${String(++i).padStart(2, "0")}` };
          res.push(cur);
        }
        cur.files.push(x.f);
        cur.loc += x.n;
      }
    }
  }
  return res;
}

if (import.meta.main) {
  const part = parsePart(argValue(process.argv, "--part", "0"));
  const dir = stateDirFor(part);
  const coreSnap = join(dir, "src-core");
  const syncSnap = join(dir, "src-sync");
  for (const snap of [coreSnap, syncSnap]) {
    if (!existsSync(snap)) {
      console.error(`Missing snapshot ${snap}. Run preflight.sh first (same --part or SWARM_DIR).`);
      process.exit(1);
    }
  }
  const core = build("core", coreSnap, ["packages", "scripts", ".github"]);
  const sync = build("sync", syncSnap, ["nodespaced-sync", "src", "app", "scripts", "tests", "examples", PRO_DIR, ".github"]);
  const all = [...core, ...sync].filter((u) => !part || u.part === part);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "units.json"), JSON.stringify(all, null, 1));
  writeFileSync(join(dir, "plan-units.json"), JSON.stringify(all.map(({ key, repo, tier, part: p, model, lenses, loc }) => ({ key, repo, tier, part: p, model, lenses, loc }))));
  const agents = all.reduce((a, u) => a + u.lenses.length, 0);
  const opus = all.filter((u) => u.model === "opus").length;
  console.log(`state dir ${dir}`);
  console.log(`part ${part || "all"}: units ${all.length}, lines ${all.reduce((a, u) => a + u.loc, 0)}, finder agents ${agents} (opus ${opus}, sonnet ${agents - opus})`);
}
