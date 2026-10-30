// Builds the Workflow args from the state directory, so the run can start
// fresh or resume after a crash or a quota stop.
// usage: bun make-args.ts --part 1|2|3 [--profile lean] [--max 60] > args.json   (--part 0 or omitted = everything)
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { argValue, parsePart, resolveRepos, stateDirFor } from "./config";

type PlanUnit = { key: string; tier: string; part: number; lenses: string[] };

const part = parsePart(argValue(process.argv, "--part", "0"));
const S = stateDirFor(part);
const ls = (d: string) => {
  try {
    return readdirSync(`${S}/${d}`).filter((f) => f.endsWith(".json"));
  } catch {
    return [];
  }
};

if (!existsSync(`${S}/plan-units.json`)) {
  console.error(`Missing ${S}/plan-units.json. Run preflight.sh and make-units.ts --part ${part} first.`);
  process.exit(1);
}
const units: PlanUnit[] = JSON.parse(readFileSync(`${S}/plan-units.json`, "utf8")).filter((u: PlanUnit) => !part || u.part === part);
if (units.length === 0) {
  console.error(`${S}/plan-units.json has no units for part ${part || "all"}. Run make-units.ts --part ${part} with the same SWARM_DIR first.`);
  process.exit(1);
}
const profile = argValue(process.argv, "--profile", "full");
const lean = profile === "lean";
const find = ls("findings").map((f) => f.replace(/\.json$/, ""));
const expected = units.flatMap((u) => u.lenses.filter((l) => !(lean && ((l === "hygiene" && u.tier === "critical") || u.tier === "tests"))).map((l) => `${u.key}__${l}`));
const allFind = expected.every((k) => find.includes(k));
const sev: Record<string, string> = {};
if (existsSync(`${S}/candidates.json`)) for (const c of JSON.parse(readFileSync(`${S}/candidates.json`, "utf8")).candidates ?? []) sev[c.id] = c.severity;
const vfiles = ls("verdicts").map((f) => f.replace(/\.json$/, ""));
const verify = Object.keys(sev).filter((id) => vfiles.includes(`${id}__1`) && (sev[id] === "P3" || vfiles.includes(`${id}__2`)));
const review: Record<string, { decision: string; finalSeverity?: string }> = {};
for (const f of ls("review")) {
  try {
    const parsed = JSON.parse(readFileSync(`${S}/review/${f}`, "utf8"));
    review[f.replace(/\.json$/, "")] = { decision: parsed.decision, finalSeverity: parsed.finalSeverity };
  } catch {
    // A half-written review file counts as not done.
  }
}
const filed = ls("filed").map((f) => f.replace(/\.json$/, ""));
const runDate = new Date().toISOString().slice(0, 10);
if (!/^\d{4}-\d{2}-\d{2}$/.test(runDate)) {
  console.error(`Cannot compute the run date (got "${runDate}"); the filed issues would carry no date.`);
  process.exit(1);
}
const repos = resolveRepos(S);
const args = {
  stateDir: S,
  part,
  profile,
  maxCandidates: Number(argValue(process.argv, "--max", "60")),
  runDate,
  coreRepo: repos.coreRepo,
  syncRepo: repos.syncRepo,
  docsRepo: repos.docsRepo,
  units,
  reuseCandidates: allFind && existsSync(`${S}/candidates.json`),
  done: { find, verify, review, filed },
};
writeFileSync(`${S}/args.json`, JSON.stringify(args));
console.error(`find ${find.length}/${expected.length} verify ${verify.length} review ${Object.keys(review).length} filed ${filed.length} reuseCandidates ${args.reuseCandidates}`);
console.log(JSON.stringify(args));
