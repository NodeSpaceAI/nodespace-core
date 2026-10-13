/**
 * CLI helpers shared by the fixtures that seed a workspace through the
 * `nodespace` CLI, so seeded state is byte-identical to what the daemon would
 * have produced.
 */

import type { EvalEnv } from "../env.ts";

/** The id of the database settings node, which the owner holds a role on. */
export const DATABASE_SETTINGS_ID = "database-settings-singleton";

export function runNs(env: EvalEnv, args: string[]): unknown {
  const r = Bun.spawnSync([env.nsBin, "--socket", env.socket, "--json", ...args], {
    stdout: "pipe",
    stderr: "pipe",
  });
  if (r.exitCode !== 0) {
    throw new Error(
      `nodespace ${args.join(" ")} failed (exit ${r.exitCode}): ` +
        r.stderr.toString().trim(),
    );
  }
  const out = r.stdout.toString().trim();
  return out ? JSON.parse(out) : null;
}

export interface ListedNode {
  id?: string;
  title?: string;
  content?: string;
}

export function listType(env: EvalEnv, type: string): ListedNode[] {
  const out = runNs(env, ["node", "query", "--type", type, "--limit", "200"]) as
    | { nodes?: ListedNode[] }
    | ListedNode[]
    | null;
  return Array.isArray(out) ? out : (out?.nodes ?? []);
}

export function bareId(id: string): string {
  return id.replace(/^nodespace:\/\//, "");
}

/** The node of `type` titled `title`, or null. Case-insensitive. */
export function findByTitle(env: EvalEnv, type: string, title: string): string | null {
  const present = listType(env, type).find(
    (n) => (n.title ?? n.content ?? "").toLowerCase() === title.toLowerCase(),
  );
  return present?.id ? bareId(present.id) : null;
}

/** Create an edge, treating one that exists already as success. */
export function ensureEdge(env: EvalEnv, from: string, type: string, to: string): void {
  try {
    runNs(env, ["relationship", "create", "--from", from, "--type", type, "--to", to]);
  } catch (e) {
    if (!/exist|already|duplicate/i.test(String(e))) throw e;
  }
}

/**
 * The person the database belongs to, who "me" is. Every database is seeded
 * with one, blank, holding the role on the settings node.
 */
export function ownerId(env: EvalEnv): string {
  const held = runNs(env, [
    "relationship",
    "get",
    DATABASE_SETTINGS_ID,
    "--type",
    "has_role",
    "--direction",
    "in",
  ]) as { nodes?: ListedNode[] } | ListedNode[] | null;
  const id = (Array.isArray(held) ? held : (held?.nodes ?? []))[0]?.id;
  if (!id) throw new Error("the database has no owner to name");
  return bareId(id);
}
