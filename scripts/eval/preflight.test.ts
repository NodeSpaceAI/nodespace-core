/**
 * Unit tests for preflight.ts's pure parsing helpers.
 *
 * Runs via `bun run test:scripts` (and so under `bun run test:all`). Deliberate
 * exception to the project-wide "never use `bun test`" rule: this file touches
 * no DOM, and cannot run under Vitest anyway (imports `bun:test`, and scripts/
 * is outside every Vitest project glob).
 */

import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { providerOf } from "./env.ts";
import {
  awaitSkillIndex,
  embeddedSkillCount,
  extractSeedEntries,
  providerIdsIn,
  remoteConfigId,
  seededSkillCount,
} from "./preflight.ts";

describe("extractSeedEntries", () => {
  test("extracts key and version from seeded nodes", () => {
    const json = JSON.stringify({
      nodes: [
        {
          properties: { _seed: { key: "node-creation", version: "3" } },
        },
        {
          properties: { _seed: { key: "graph-editing", version: "1" } },
        },
      ],
    });
    expect(extractSeedEntries(json)).toEqual([
      { key: "node-creation", version: "3" },
      { key: "graph-editing", version: "1" },
    ]);
  });

  test("skips nodes with no _seed metadata", () => {
    const json = JSON.stringify({
      nodes: [
        { properties: { _seed: { key: "a", version: "1" } } },
        { properties: {} },
        { properties: { _seed: {} } },
      ],
    });
    expect(extractSeedEntries(json)).toEqual([{ key: "a", version: "1" }]);
  });

  test("falls back to a placeholder when version is missing", () => {
    const json = JSON.stringify({
      nodes: [{ properties: { _seed: { key: "a" } } }],
    });
    expect(extractSeedEntries(json)).toEqual([
      { key: "a", version: "(no version)" },
    ]);
  });

  test("returns an empty list for no nodes", () => {
    expect(extractSeedEntries(JSON.stringify({ nodes: [] }))).toEqual([]);
  });

  test("returns an empty list rather than throwing on unparseable JSON", () => {
    expect(extractSeedEntries("not json")).toEqual([]);
  });

  test("returns an empty list when the nodes key is absent", () => {
    expect(extractSeedEntries(JSON.stringify({}))).toEqual([]);
  });
});

describe("awaitSkillIndex", () => {
  // Embeddings run on a ~30s debounce, so a daemon started against a purged
  // database serves its first turns with an EMPTY skill index. Stage-2 then
  // fails open to the full tool surface with no skill guidance, and the
  // malformed write that follows cascades through the rest of its group —
  // measured live: create_schema missing its required top-level `name`, then
  // create_node naming a type that was never created.
  const env = {
    nsBin: "x",
    socket: "s",
    log: "l",
    model: "m",
    timeoutMs: 1,
    aichat: "a",
  };
  const noSleep = () => {};

  test("returns immediately when every seeded skill is already retrievable", () => {
    let probes = 0;
    awaitSkillIndex(
      env,
      60_000,
      8,
      () => {
        probes++;
        return 8;
      },
      noSleep,
    );
    expect(probes).toBe(1);
  });

  test("waits for an index that populates late, then proceeds", () => {
    const counts = [0, 0, 8];
    let i = 0;
    let slept = 0;
    expect(() =>
      awaitSkillIndex(
        env,
        60_000,
        8,
        () => counts[Math.min(i++, counts.length - 1)]!,
        () => {
          slept++;
        },
      ),
    ).not.toThrow();
    expect(slept).toBe(2);
  });

  test("throws when the index never populates", () => {
    // A clock driven by the sleep callback, so the timeout is reached without
    // waiting in real time.
    let t = 0;
    expect(() =>
      awaitSkillIndex(
        env,
        10_000,
        8,
        () => 0,
        () => {
          t += 4_000;
        },
        () => t,
      ),
    ).toThrow(/seeded skills are semantically retrievable/);
  });

  // The denominator is read from the database, not hardcoded, so adding a
  // ninth skill cannot silently make the gate under-wait. A run whose index
  // holds 8 of 9 must still be treated as not ready.
  test("waits for every seeded skill, not a hardcoded eight", () => {
    let t = 0;
    expect(() =>
      awaitSkillIndex(
        env,
        10_000,
        9,
        () => 8,
        () => {
          t += 4_000;
        },
        () => t,
      ),
    ).toThrow(/Only 8 of 9/);
  });

  test("treats a partially-populated index as not ready", () => {
    // 7 of 8 is the quieter form of the same defect: if the missing one is
    // Graph Editing, every update scenario scores against a surface that never
    // offered update_node.
    let t = 0;
    expect(() =>
      awaitSkillIndex(
        env,
        10_000,
        8,
        () => 7,
        () => {
          t += 4_000;
        },
        () => t,
      ),
    ).toThrow(/Only 7 of 8/);
  });
});

describe("seededSkillCount", () => {
  const env = {
    nsBin: "x",
    socket: "s",
    log: "l",
    model: "m",
    timeoutMs: 1,
    aichat: "a",
  };

  test("uses the enumerated row count as the denominator", () => {
    expect(seededSkillCount(env, () => 9)).toBe(9);
  });

  // A CLI hiccup is not evidence about how many skills exist, so it degrades
  // to the known seed count rather than to a denominator of zero (which would
  // wave the gate through against an empty index).
  test("falls back to the known seed count when enumeration fails", () => {
    expect(seededSkillCount(env, () => null)).toBe(8);
  });

  // A SUCCESSFUL enumeration returning zero is the opposite situation: the
  // rows are inserted synchronously at startup, so zero of them means seeding
  // never ran. Substituting the fallback made the gate poll for eight skills
  // that cannot appear, burning the full 120s timeout before reporting a
  // generic "index not ready" — the wrong fault, reported late.
  test("reports broken seeding instead of waiting for skills that cannot appear", () => {
    expect(() => seededSkillCount(env, () => 0)).toThrow(/zero skill nodes/);
  });
});

// -- embeddedSkillCount -------------------------------------------------
//
// The gate this feeds must be able to FAIL. Its predecessor could not: the
// numerator was scope-filtered to a structural zero while the denominator was
// an enumerate, so it blocked every run until timeout. These assert both
// directions — a warm index passes, and a cold one does not — because a probe
// that always returns the seed count is exactly as broken as one that always
// returns zero, just in the opposite direction.

describe("embeddedSkillCount", () => {
  test("reports the embedded row count on a warm index", () => {
    const n = embeddedSkillCount("/db", () => ({ exitCode: 0, stdout: "8\n" }));
    expect(n).toBe(8);
  });

  test("reports 0 on a cold index, so the gate keeps waiting", () => {
    // Rows exist but carry no embeddings yet — the state the gate exists for.
    const n = embeddedSkillCount("/db", () => ({ exitCode: 0, stdout: "0\n" }));
    expect(n).toBe(0);
  });

  test("reports a partial index, so a half-embedded state does not pass", () => {
    const n = embeddedSkillCount("/db", () => ({ exitCode: 0, stdout: "5\n" }));
    expect(n).toBe(5);
  });

  test("fails closed when the query cannot run", () => {
    // An unreadable database must keep the gate waiting rather than wave it
    // through on a fallback count.
    expect(embeddedSkillCount("/db", () => ({ exitCode: 1, stdout: "" }))).toBe(0);
    expect(embeddedSkillCount("/db", () => ({ exitCode: null, stdout: "" }))).toBe(0);
  });

  test("fails closed on unparseable output", () => {
    expect(embeddedSkillCount("/db", () => ({ exitCode: 0, stdout: "oops" }))).toBe(0);
  });

  test("fails closed when the probe THROWS", () => {
    // `Bun.spawnSync` throws on a missing executable rather than returning a
    // non-zero exitCode, so a machine without `sqlite3` took neither of the
    // guarded branches — the throw escaped into `gate()`, which renders only
    // EnvironmentError actionably and rethrows the rest as a stack trace.
    const n = embeddedSkillCount("/db", () => {
      throw new Error("Executable not found in $PATH: sqlite3");
    });
    expect(n).toBe(0);
  });

  test("fails closed when no database path is known", () => {
    let ran = false;
    const n = embeddedSkillCount("", () => {
      ran = true;
      return { exitCode: 0, stdout: "8\n" };
    });
    expect(n).toBe(0);
    expect(ran).toBe(false);
  });

  test("the default query counts only skills whose vector exists", () => {
    // A node's embedding row is written, stale, when the node is created; the
    // vector lands on the worker's ~30s debounce. Counting the stale row let
    // the wait return before skills seeded mid-run could be retrieved.
    const dir = mkdtempSync(join(tmpdir(), "preflight-"));
    const db = join(dir, "test.db");
    try {
      const sql =
        "CREATE TABLE node (id TEXT, node_type TEXT);" +
        "CREATE TABLE embedding (node_id TEXT, stale INTEGER);" +
        "INSERT INTO node VALUES ('s1','skill'),('s2','skill'),('t1','text');" +
        "INSERT INTO embedding VALUES ('s1',0),('s2',1),('t1',0);";
      expect(Bun.spawnSync(["sqlite3", db, sql]).exitCode).toBe(0);
      expect(embeddedSkillCount(db)).toBe(1);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("remote provider ids", () => {
  test("a remote model id names its provider config, with or without a model", () => {
    expect(remoteConfigId("openai-compat:abc-123:google/gemma-4-31b-it")).toBe(
      "abc-123",
    );
    expect(remoteConfigId("openai-compat:abc-123")).toBe("abc-123");
  });

  test("a model whose name contains colons keeps the config id whole", () => {
    expect(remoteConfigId("openai-compat:abc-123:qwen/qwen3:free")).toBe(
      "abc-123",
    );
  });

  test("provider ids are found wherever the store hoists the providers array", () => {
    const nested = JSON.stringify({
      properties: {
        "database-settings": {
          providers: [{ id: "p1", api_key: "secret" }, { id: "p2" }],
        },
      },
    });
    const flat = JSON.stringify({ properties: { providers: [{ id: "p1" }] } });
    expect(providerIdsIn(nested)).toEqual(["p1", "p2"]);
    expect(providerIdsIn(flat)).toEqual(["p1"]);
  });

  test("no providers, bad JSON and entries without an id yield none", () => {
    expect(providerIdsIn(JSON.stringify({ properties: {} }))).toEqual([]);
    expect(providerIdsIn("not json")).toEqual([]);
    expect(
      providerIdsIn(JSON.stringify({ providers: [{ name: "x" }, null] })),
    ).toEqual([]);
  });

  test("the provider of a chat follows its model id", () => {
    expect(providerOf("openai-compat:abc:google/gemma-4-31b-it")).toBe(
      "openai-compat",
    );
    expect(providerOf("gemma-4-e4b-q4km")).toBe("native");
  });
});
