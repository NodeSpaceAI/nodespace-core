// Shared settings for the review swarm kit: which part (night) a run covers,
// where its state directory lives and which repositories it reads. Order of
// precedence for every value: environment variable, then the config.json that
// preflight.sh wrote into the state directory, then the default.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

export const KIT_DIR = dirname(new URL(import.meta.url).pathname);

export function argValue(argv: string[], name: string, fallback: string): string {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] !== undefined ? argv[i + 1] : fallback;
}

/** 0 means every part; otherwise 1 to 3. Anything else is a usage error. */
export function parsePart(raw: string): number {
  const part = Number(raw);
  if (!Number.isInteger(part) || part < 0 || part > 3) {
    throw new Error(`--part must be 1, 2 or 3 (or 0 for everything), got "${raw}"`);
  }
  return part;
}

export function stateDirFor(part: number, env: Record<string, string | undefined> = process.env): string {
  if (env.SWARM_DIR) return resolve(env.SWARM_DIR);
  return join(env.HOME ?? homedir(), "nodespace-swarm", part ? `night-${part}` : "night-all");
}

export type SwarmConfig = { coreRepo?: string; syncRepo?: string; docsRepo?: string };

export function readConfig(dir: string): SwarmConfig {
  const file = join(dir, "config.json");
  if (!existsSync(file)) return {};
  try {
    return JSON.parse(readFileSync(file, "utf8")) as SwarmConfig;
  } catch {
    return {};
  }
}

function gitToplevel(dir: string): string | undefined {
  const r = spawnSync("git", ["-C", dir, "rev-parse", "--show-toplevel"], { encoding: "utf8" });
  return r.status === 0 ? r.stdout.trim() : undefined;
}

/** Repository paths for the run; a path is undefined when it cannot be found. */
export function resolveRepos(dir: string, env: Record<string, string | undefined> = process.env): SwarmConfig {
  const saved = readConfig(dir);
  return {
    coreRepo: env.CORE_REPO || saved.coreRepo || gitToplevel(KIT_DIR),
    syncRepo: env.SYNC_REPO || saved.syncRepo,
    docsRepo: env.DOCS_REPO || saved.docsRepo,
  };
}
