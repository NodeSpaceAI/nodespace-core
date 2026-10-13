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
  assertNoModelContention,
  awaitSkillIndex,
  embeddedSkillCount,
  EnvironmentError,
  findModelHolders,
  parseSeedEntries,
  providerIdsIn,
  readGuidanceProvenance,
  remoteConfigId,
  seededSkillCount,
} from "./preflight.ts";

describe("parseSeedEntries", () => {
  const row = (over: Record<string, unknown> = {}) => ({
    type: "skill",
    key: "Node Creation",
    config_version: "c1",
    guidance_version: "g1",
    ...over,
  });

  test("groups rows by node type with both versions", () => {
    const json = JSON.stringify([
      row(),
      row({ type: "agent-guidance", key: "Core" }),
    ]);
    expect(parseSeedEntries(json)).toEqual({
      skill: [
        { key: "Node Creation", configVersion: "c1", guidanceVersion: "g1" },
      ],
      "agent-guidance": [
        { key: "Core", configVersion: "c1", guidanceVersion: "g1" },
      ],
    });
  });

  test("drops a row missing its key or either version", () => {
    const json = JSON.stringify([
      row({ key: null }),
      row({ config_version: null }),
      row({ guidance_version: undefined }),
    ]);
    expect(parseSeedEntries(json)).toEqual({});
  });

  test("returns nothing for empty or unparseable output", () => {
    expect(parseSeedEntries("")).toEqual({});
    expect(parseSeedEntries("not json")).toEqual({});
    expect(parseSeedEntries(JSON.stringify({ not: "rows" }))).toEqual({});
  });
});

describe("readGuidanceProvenance", () => {
  const rows = JSON.stringify([
    { type: "skill", key: "A", config_version: "c", guidance_version: "g" },
    { type: "agent-guidance", key: "B", config_version: "c", guidance_version: "g" },
  ]);

  test("returns every seeded type's entries", () => {
    const g = readGuidanceProvenance("/db", () => ({ exitCode: 0, stdout: rows }));
    expect(Object.keys(g).sort()).toEqual(["agent-guidance", "skill"]);
  });

  test("queries the seed stamp, not node properties through the CLI", () => {
    let sql = "";
    readGuidanceProvenance("/db", (_db, q) => {
      sql = q;
      return { exitCode: 0, stdout: rows };
    });
    expect(sql).toContain("$._seed.config_version");
    expect(sql).toContain("$._seed.guidance_version");
  });

  test("an empty readback fails the run rather than recording nothing", () => {
    expect(() =>
      readGuidanceProvenance("/db", () => ({ exitCode: 0, stdout: "" })),
    ).toThrow(EnvironmentError);
  });

  test("a seeded type with no entries fails the run", () => {
    const onlySkill = JSON.stringify([
      { type: "skill", key: "A", config_version: "c", guidance_version: "g" },
    ]);
    expect(() =>
      readGuidanceProvenance("/db", () => ({ exitCode: 0, stdout: onlySkill })),
    ).toThrow(/agent-guidance/);
  });

  test("an unreadable database fails the run", () => {
    expect(() =>
      readGuidanceProvenance("/db", () => ({ exitCode: 1, stdout: "" })),
    ).toThrow(EnvironmentError);
    expect(() =>
      readGuidanceProvenance("/db", () => {
        throw new Error("Executable not found");
      }),
    ).toThrow(EnvironmentError);
    expect(() => readGuidanceProvenance("")).toThrow(EnvironmentError);
  });
});

describe("findModelHolders", () => {
  const SOCKET = "/tmp/eval.sock";
  const lsof = (...procs: string[][]) =>
    procs.map((lines) => lines.join("\n")).join("\n");
  const proc = (pid: number, cmd: string, cwd: string, ...names: string[]) => [
    `p${pid}`,
    `c${cmd}`,
    "fcwd",
    `n${cwd}`,
    ...names.flatMap((n) => ["f9", `n${n}`]),
  ];

  test("the daemon under test is expected, whatever it holds", () => {
    const out = lsof(
      proc(10, "nodespaced", "/w", SOCKET, "/m/gemma-4-e4b.gguf"),
    );
    expect(findModelHolders(out, SOCKET)).toEqual([]);
  });

  test("another process holding a chat model is named with its cwd", () => {
    const out = lsof(
      proc(10, "nodespaced", "/w", SOCKET, "/m/gemma-4-e4b.gguf"),
      proc(20, "nodespace-app", "/Users/x/wt/pr-9", "/m/gemma-4-e4b.gguf"),
    );
    expect(findModelHolders(out, SOCKET)).toEqual([
      {
        pid: 20,
        command: "nodespace-app",
        cwd: "/Users/x/wt/pr-9",
        models: ["/m/gemma-4-e4b.gguf"],
      },
    ]);
  });

  test("the embedding model is not contention", () => {
    const out = lsof(
      proc(20, "nodespaced", "/w", "/m/nomic-embed-text-v1.5.Q8_0.gguf"),
    );
    expect(findModelHolders(out, SOCKET)).toEqual([]);
  });

  test("an Ollama blob counts, a manifest does not", () => {
    const blob = "/Users/x/.ollama/models/blobs/sha256-0a1b2c";
    const out = lsof(
      proc(30, "ollama", "/", blob, "/Users/x/.ollama/models/manifests/m"),
    );
    expect(findModelHolders(out, SOCKET).map((h) => h.models)).toEqual([
      [blob],
    ]);
  });

  test("a process listed once per file reports each model once", () => {
    const out = lsof(
      proc(20, "llama", "/w", "/m/a.gguf", "/m/a.gguf", "/m/b.gguf"),
    );
    expect(findModelHolders(out, SOCKET)[0]?.models).toEqual([
      "/m/a.gguf",
      "/m/b.gguf",
    ]);
  });
});

describe("assertNoModelContention", () => {
  const env = { socket: "/tmp/eval.sock" };

  test("passes when nothing else holds a model", () => {
    expect(() => assertNoModelContention(env, () => "p1\ncinit\n")).not.toThrow();
  });

  test("names the holder and its working directory", () => {
    const out = "p20\ncnodespace-app\nfcwd\nn/Users/x/wt/pr-9\nf9\nn/m/chat.gguf\n";
    let message = "";
    try {
      assertNoModelContention(env, () => out);
    } catch (e) {
      expect(e).toBeInstanceOf(EnvironmentError);
      message = (e as Error).message;
    }
    expect(message).toContain("pid 20");
    expect(message).toContain("nodespace-app");
    expect(message).toContain("/Users/x/wt/pr-9");
    expect(message).toContain("/m/chat.gguf");
  });

  test("fails closed when open files cannot be listed", () => {
    expect(() =>
      assertNoModelContention(env, () => {
        throw new Error("lsof: not found");
      }),
    ).toThrow(EnvironmentError);
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
