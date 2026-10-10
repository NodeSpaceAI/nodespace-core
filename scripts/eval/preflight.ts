/**
 * Preflight gate — refuse to score scenarios when the environment cannot
 * produce a valid result.
 *
 * An eval that scores a broken environment is worse than no eval, because its
 * output gets cited. The motivating case: a matrix run reported its two
 * "no tools called" scenarios as PASSING while every turn was dying on
 * `Context window exceeded` before inference ran. A turn that times out calls
 * no tools, so a negative assertion scores it green. The run produced a
 * plausible partial result from an environment where nothing worked, and it
 * took reading daemon logs to notice.
 *
 * Each check below answers a question whose wrong answer silently corrupts
 * scores rather than failing loudly:
 *   1. daemon reachable       — otherwise every turn errors and every negative
 *                               assertion passes
 *   2. requested model loaded — otherwise the score belongs to a different
 *                               model than the one it will be filed under
 *   3. granted window > prompt— the case above; catches in ~2s what otherwise
 *                               takes ~8min of inference to half-discover
 *
 * Failures exit with EXIT_ENVIRONMENT, distinct from a scenario failure, and
 * no results file is written.
 */

import { providerOf, REMOTE_MODEL_PREFIX, type EvalEnv } from "./env.ts";
import type { GuidanceProvenance } from "./types.ts";

/** Scenario assertions failed. */
export const EXIT_FAILED = 1;
/**
 * The environment could not produce a valid result. Distinct from EXIT_FAILED
 * so a caller can tell "the agent regressed" from "this machine cannot run the
 * eval" without parsing output.
 */
export const EXIT_ENVIRONMENT = 2;
/**
 * The command line was wrong — nothing was run. Distinct again, so a wrapper
 * script cannot mistake "you invoked this incorrectly" for a real result.
 */
export const EXIT_USAGE = 64;

export class EnvironmentError extends Error {
  constructor(
    message: string,
    /** What the operator should do about it. */
    readonly remedy: string,
  ) {
    super(message);
    this.name = "EnvironmentError";
  }
}

/** What the daemon reports about the model it has loaded. */
export interface DaemonStatus {
  loaded: boolean;
  modelId: string;
  grantedNCtx: number;
  hostRamBytes: number;
  /** On-disk path of the database the daemon is actually serving. */
  databasePath: string;
  /**
   * True when `modelId` could only be matched to the requested model by
   * filename, because the daemon reported a resolved path instead of a catalog
   * id. The exact build is unconfirmed in that case; recorded in the results
   * file so the caveat outlives the terminal session that saw the warning.
   */
  modelMatchedByPath?: boolean;
}

/**
 * Size of the agent's tool-registered system prompt, in tokens.
 *
 * Kept in step with `N_CTX_MINIMUM` in packages/nlp-engine/src/chat/mod.rs,
 * whose doc comment records the same ~6,600-token measurement and refuses to
 * load a model into a window below 16,384 for this reason. The exact figure
 * varies with the registered tool set and seeded skills, so this is a floor
 * used to catch an unusable window, not an accounting of a specific run.
 */
export const AGENT_SYSTEM_PROMPT_TOKENS = 6_600;

/**
 * Headroom the eval requires beyond the system prompt itself.
 *
 * The prompt is only the floor: every turn also carries the conversation so
 * far, the injected entity types, and tool results — and multi-turn scenario
 * groups grow all three. A window that merely clears the prompt would pass
 * preflight and then fail mid-group, which is the failure this gate exists to
 * prevent rather than relocate.
 */
const CONTEXT_HEADROOM_TOKENS = 4096;

/** Ask the daemon what it has loaded. Throws EnvironmentError if unreachable. */
export function readDaemonStatus(env: EvalEnv): DaemonStatus {
  // A missing binary throws rather than returning a non-zero exit, and an
  // unhandled throw here would surface as a stack trace with the generic
  // failure exit code — indistinguishable from a scenario failure, which is
  // precisely the confusion this gate exists to remove.
  const spawn = () =>
    Bun.spawnSync(
      [env.nsBin, "--socket", env.socket, "--json", "model", "status"],
      {
        stdout: "pipe",
        stderr: "pipe",
      },
    );
  let r: ReturnType<typeof spawn>;
  try {
    r = spawn();
  } catch (e) {
    throw new EnvironmentError(
      `Could not run the nodespace CLI at ${env.nsBin}: ${e}`,
      `Build it (cargo build --release -p nodespace-cli) or set NS_BIN to its path.`,
    );
  }

  if (r.exitCode !== 0) {
    const stderr = r.stderr.toString().trim();
    // Distinguish "this CLI predates `model status`" from "the daemon is down".
    // Both exit non-zero, and reporting the former as the latter sends the
    // operator to restart a daemon that was never the problem.
    if (/unrecognized subcommand|unexpected argument/i.test(stderr)) {
      throw new EnvironmentError(
        `The CLI at ${env.nsBin} has no \`model status\` subcommand, so the ` +
          `granted context window cannot be checked.`,
        `Rebuild it: cargo build --release -p nodespace-cli`,
      );
    }
    throw new EnvironmentError(
      `Daemon is not reachable on ${env.socket} (\`model status\` exited ${r.exitCode}).\n` +
        (stderr ? `  ${stderr}\n` : ""),
      `Start the test daemon and confirm it is listening on ${env.socket}, or point\n` +
        `  NODESPACED_SOCKET at the socket it is actually using. See ENV_USAGE in\n` +
        `  env.ts for every variable this harness reads.`,
    );
  }

  let parsed: {
    loaded?: boolean;
    model_id?: string;
    granted_n_ctx?: number;
    host_ram_bytes?: number;
  };
  try {
    parsed = JSON.parse(r.stdout.toString());
  } catch (e) {
    throw new EnvironmentError(
      `Could not parse \`model status --json\` output: ${e}`,
      `This usually means NS_BIN (${env.nsBin}) is an older build without\n` +
        `  \`model status\`. Rebuild the CLI: cargo build --release -p nodespace-cli`,
    );
  }

  return {
    loaded: parsed.loaded ?? false,
    modelId: parsed.model_id ?? "",
    grantedNCtx: parsed.granted_n_ctx ?? 0,
    hostRamBytes: parsed.host_ram_bytes ?? 0,
    databasePath: readServedDatabasePath(env),
  };
}

/**
 * How many seeded skills must be semantically retrievable before scoring.
 *
 * EVERY seeded skill, not "at least one": a partially-populated index is the
 * same defect in a quieter form. Stage-2 scopes the tool surface to the
 * retrieved skills' whitelists, so a run where `Graph Editing` alone is missing
 * scores every update scenario against a surface with no `update_node`.
 *
 * Read from the database rather than hardcoded. The seed list lives in Rust
 * (`skill_pipeline::seed_skill_nodes`) and this harness cannot import it, so a
 * literal here would silently under-wait the day a ninth skill is added — the
 * gate would pass with the new skill unembedded, which is exactly the state it
 * exists to prevent. Counting the rows is self-maintaining: they are inserted
 * synchronously at daemon startup, so they are all present well before any of
 * them is embedded.
 */
const FALLBACK_SEEDED_SKILL_COUNT = 8;

/** How long to wait for the skill index, and how often to re-probe. */
const SKILL_INDEX_TIMEOUT_MS = 120_000;
const SKILL_INDEX_POLL_MS = 3_000;

/**
 * Per-probe ceiling.
 *
 * `spawnSync` blocks the whole process, so a daemon that stops responding
 * mid-request hangs the probe forever — and with it `awaitSkillIndex`, whose
 * deadline is only consulted between probes. The bounded wait it advertises is
 * only bounded if each probe is. Generous relative to a search that normally
 * returns in well under a second, so a slow machine is not mistaken for a hung
 * daemon; a timed-out probe returns `null` like any other failure and the wait
 * loop keeps polling until its own deadline.
 */
const SKILL_PROBE_TIMEOUT_MS = 15_000;

/** Run a `--type skill` search and return the result count, or `null` on error. */
function skillSearchCount(env: EvalEnv, query: string, limit: number): number | null {
  const r = Bun.spawnSync(
    [
      env.nsBin,
      "--socket",
      env.socket,
      "--json",
      "search",
      query,
      "--type",
      "skill",
      "--threshold",
      "0.01",
      "--limit",
      String(limit),
    ],
    { stdout: "pipe", stderr: "pipe", timeout: SKILL_PROBE_TIMEOUT_MS },
  );
  // A killed probe reports `exitCode: null`, which is also not 0 — both are
  // "no usable count", which is what `null` means to the caller.
  if (r.exitCode !== 0) return null;
  try {
    const parsed = JSON.parse(r.stdout.toString()) as { count?: number };
    return parsed.count ?? 0;
  } catch {
    return null;
  }
}

/**
 * How many skill nodes exist, regardless of whether they are embedded yet.
 *
 * An empty query with `--type skill` enumerates by type, which does NOT depend
 * on embeddings — the rows are inserted synchronously at daemon startup. That
 * is what makes it usable as the denominator: it is known correct immediately,
 * while the numerator below is what has to catch up.
 *
 * Falls back to the known seed count if the enumeration FAILS (`null`), so a CLI
 * hiccup degrades to today's behaviour rather than waving the gate through with
 * a denominator of zero.
 *
 * A successful enumeration returning 0 is a different thing entirely and must
 * not take that fallback. The rows are inserted synchronously at startup, so
 * zero of them means seeding did not happen — and substituting 8 makes the gate
 * wait the full timeout for skills that cannot ever appear, then report a
 * generic "index not ready". The real fault is knowable now, so it is reported
 * now.
 */
export function seededSkillCount(
  env: EvalEnv,
  // Injected so the fail/zero/real-count branches are testable without a
  // daemon, the same way `awaitSkillIndex` takes its probe.
  enumerate: (env: EvalEnv) => number | null = (e) => skillSearchCount(e, "", 100),
): number {
  const count = enumerate(env);
  if (count === null) return FALLBACK_SEEDED_SKILL_COUNT;
  if (count === 0) {
    throw new EnvironmentError(
      `The daemon reports zero skill nodes, so no amount of waiting will make the ` +
        `skill index ready — Stage-2 would score every turn against an empty tool ` +
        `surface.\n` +
        `  Skill rows are inserted synchronously at daemon startup, so an empty ` +
        `enumeration means seeding never ran against this database.`,
      `Confirm the daemon booted against the seeded database and re-check:\n` +
        `    sqlite3 <db> "SELECT COUNT(*) FROM node WHERE node_type='skill'"\n` +
        `  Zero rows means the daemon must be restarted against a seeded database.`,
    );
  }
  return count;
}

/**
 * Number of seeded skill nodes that carry an embedding right now.
 *
 * Counts embedded rows in SQL rather than issuing a semantic search, because
 * no CLI query can answer this question. `search --type skill` with a real
 * query is filtered by the default Knowledge scope (text/header/code-block/
 * schema/table), which does not include `skill`, and the CLI exposes no
 * `--scope` flag to widen it (`packages/cli/src/commands/search.rs`). The
 * scope filter is bypassed only for an ENUMERATE query, so the two obvious
 * probes sit on opposite sides of `should_skip_scope_filter`
 * (`packages/core/src/ops/search_ops.rs`):
 *
 *   search "update an existing node" --type skill  -> always 0 (scope-filtered)
 *   search ""                        --type skill  -> 8       (bypass applies)
 *
 * The previous implementation used the first as its numerator and the second
 * as its denominator, so the gate compared 8 against a structurally-zero count
 * and could never pass — it blocked every run of the matrix, for every model,
 * until its 120s timeout expired.
 *
 * Switching the numerator to the enumerate form is NOT the fix: `enumerate_nodes`
 * queries rows by type and never consults embeddings, so it returns every skill
 * the instant the daemon boots and would wave through exactly the cold-index
 * state this gate exists to catch.
 *
 * Reading the embedding table directly is the only probe that asserts the real
 * property, and only its non-stale rows do: a row exists, stale, from the
 * moment its node is created. The path comes from the daemon's own `database
 * list`, so it cannot drift onto a different database than the one being
 * scored.
 */
function retrievableSkillCount(env: EvalEnv): number {
  return embeddedSkillCount(readServedDatabasePath(env));
}

/**
 * Skill rows carrying an embedding, read from the served database.
 *
 * Returns 0 — never a fallback count — when the query cannot be run at all, so
 * an unreadable database keeps the gate waiting rather than waving it through.
 *
 * "Cannot be run" includes `sqlite3` not being installed. `Bun.spawnSync`
 * THROWS on a missing executable rather than returning a non-zero `exitCode`,
 * so the exit-code branch alone does not deliver the guarantee above: the throw
 * would escape through `awaitSkillIndex` into `gate()`, which converts only
 * `EnvironmentError` into an actionable message and rethrows anything else as a
 * raw stack trace — on the environment-vs-model boundary this gate exists to
 * police. The whole probe call is therefore wrapped, which also covers an
 * injected probe that throws, not just the default one.
 *
 * `sqlite3` is a real dependency of this gate. The other mentions of it in this
 * file are remediation TEXT printed for a human, never executed, so they do not
 * establish it as already-required — this is the first place the harness runs
 * it. It ships with macOS and every mainstream Linux; a machine without it
 * fails closed here rather than silently scoring.
 */
export function embeddedSkillCount(
  dbPath: string,
  run: (db: string) => { exitCode: number | null; stdout: string } = (db) => {
    const r = Bun.spawnSync(
      [
        "sqlite3",
        db,
        // `stale = 0`: a node's embedding row is marked stale when the node is
        // created or edited, and holds a current vector only once the debounced
        // worker has run. Counted, stale rows let the wait return before skills
        // seeded mid-run could be retrieved.
        "SELECT COUNT(DISTINCT e.node_id) FROM node n " +
          "JOIN embedding e ON e.node_id = n.id WHERE n.node_type = 'skill' AND e.stale = 0",
      ],
      { stdout: "pipe", stderr: "pipe", timeout: SKILL_PROBE_TIMEOUT_MS },
    );
    return { exitCode: r.exitCode, stdout: r.stdout.toString() };
  },
): number {
  if (!dbPath) return 0;
  let r: { exitCode: number | null; stdout: string };
  try {
    r = run(dbPath);
  } catch {
    // Deliberately unnarrowed. This also swallows a programming error inside a
    // caller-supplied probe, which is a real cost — but the alternative is
    // matching on Bun's "Executable not found in $PATH" message, which couples
    // this gate to a runtime's error text. Failing closed is the safe direction
    // either way: a swallowed throw leaves the count at 0, so the gate reports
    // an actionable EnvironmentError rather than scoring against an index it
    // could not verify.
    //
    // `readServedDatabasePath` is evaluated by the CALLER, outside this try, so
    // its own fail-closed EnvironmentError still propagates instead of being
    // flattened into a misleading "index not ready".
    return 0;
  }
  if (r.exitCode !== 0) return 0;
  return Number.parseInt(r.stdout.trim(), 10) || 0;
}

/**
 * Block until the seeded skills are semantically retrievable.
 *
 * Embeddings are generated on a **30-second debounce** (`EmbeddingService`'s
 * `debounce_duration_secs`), so a daemon started against a purged database
 * serves turns for half a minute with an EMPTY skill index. Stage-1 still emits
 * a query, retrieval returns nothing, and `stage2_tools` fails open to the full
 * tool surface with no skill guidance injected — the daemon logs
 * `candidates=0 routed_skills=`.
 *
 * Measured cost of not waiting, on the locked model: the first turn of a fresh
 * chain got 13 tools and no guidance, emitted a `create_schema` call missing the
 * required top-level `name`, and failed twice. The following turns then
 * cascaded — `create_node` naming a type that was never created, `update_node`
 * inventing an id — so a single cold first turn reds out an entire group. The
 * same chain passes end to end once the index is populated.
 *
 * This is why the check waits rather than failing: the condition resolves
 * itself, and the documented setup (`--between-runs` purging the database and
 * restarting the daemon) re-creates it before EVERY rep. A gate that only
 * reported it would make each rep a manual retry.
 */
export function awaitSkillIndex(
  env: EvalEnv,
  timeoutMs: number = SKILL_INDEX_TIMEOUT_MS,
  // Derived from the database by default (see `seededSkillCount`); injectable
  // so the wait/timeout behaviour stays testable without a daemon.
  //
  // This used to sit before `probe` because `probe`'s default read it; that
  // dependency is gone (the default is now `(e) => retrievableSkillCount(e)`,
  // which counts embedded rows and needs no expected value). The order is kept
  // only because it is the published signature and every caller passes
  // positionally — there is no longer a forward-reference hazard either way.
  expected: number = seededSkillCount(env),
  // Injected so the wait/timeout logic is testable without a daemon: the
  // interesting behaviour is "waits for a late index, gives up on one that
  // never arrives", and neither is expressible against a live process.
  probe: (env: EvalEnv) => number = (e) => retrievableSkillCount(e),
  sleep: (ms: number) => void = (ms) => Bun.sleepSync(ms),
  now: () => number = Date.now,
): void {
  const started = now();
  let count = probe(env);
  if (count >= expected) return;

  console.error(
    `[preflight] Skill index not ready (${count}/${expected} retrievable). ` +
      `Embeddings run on a ~30s debounce; waiting so turns are not scored against ` +
      `an empty index.`,
  );

  while (now() - started < timeoutMs) {
    sleep(SKILL_INDEX_POLL_MS);
    count = probe(env);
    if (count >= expected) {
      console.error(
        `[preflight] Skill index ready (${count}/${expected}) after ` +
          `${Math.round((now() - started) / 1000)}s.`,
      );
      return;
    }
  }

  throw new EnvironmentError(
    `Only ${count} of ${expected} seeded skills are semantically retrievable ` +
      `after ${Math.round(timeoutMs / 1000)}s, so Stage-2 routing would score against an ` +
      `incomplete skill index.\n` +
      `  Retrieval scopes the tool surface: a missing skill means its tools are never ` +
      `offered, and the turn reds out for "not calling" a tool it was never given.`,
    `Confirm the embedding worker is running and the database is the seeded one:\n` +
      `    sqlite3 <db> "SELECT n.node_type, COUNT(DISTINCT e.node_id) FROM node n\n` +
      `      LEFT JOIN embedding e ON e.node_id=n.id WHERE n.node_type='skill' GROUP BY 1"\n` +
      `  An empty embedding table with 8 skill rows means the worker never ran. An\n` +
      `  isolated daemon loads no embedding model unless NODESPACED_MODEL_PATH names\n` +
      `  one; without it the daemon log says "NLP model not found".`,
  );
}

/**
 * The database file the daemon resolved to serve.
 *
 * Read from the daemon rather than inferred from the environment because the
 * database registry overrides the boot-time path (ADR-053): setting
 * `NODESPACED_DB_PATH` alone leaves the daemon serving the real user database,
 * and nothing in the eval's own configuration reveals that.
 */
function readServedDatabasePath(env: EvalEnv): string {
  // Fails CLOSED. Every other check failing open costs a missed detection; this
  // one guards against writing test schemas and chat nodes into live user data,
  // which cannot be undone. "I could not determine which database this is" must
  // therefore stop the run, not wave it through.
  const unknown = (detail: string) =>
    new EnvironmentError(
      `Could not determine which database the daemon is serving (${detail}), so it ` +
        `cannot be confirmed that this run will not write into live user data.`,
      `Confirm the daemon is isolated — its startup log line \`served_db_path=\` must\n` +
        `  point inside your test directory — and that ${env.nsBin} is a current build\n` +
        `  (cargo build --release -p nodespace-cli).`,
    );

  const r = Bun.spawnSync(
    [env.nsBin, "--socket", env.socket, "--json", "database", "list"],
    { stdout: "pipe", stderr: "pipe" },
  );
  if (r.exitCode !== 0) {
    throw unknown(`\`database list\` exited ${r.exitCode}`);
  }
  let parsed: { databases?: Array<{ path?: string; is_default?: boolean }> };
  try {
    parsed = JSON.parse(r.stdout.toString());
  } catch (e) {
    throw unknown(`could not parse \`database list --json\`: ${e}`);
  }
  const dbs = parsed.databases ?? [];
  // The daemon serves the default database for header-less requests, which is
  // what aichat.ts sends.
  const path = (dbs.find((d) => d.is_default) ?? dbs[0])?.path;
  if (!path) {
    throw unknown("the daemon reported no databases");
  }
  return path;
}

/**
 * Seeded node types whose content the local agent's guidance is assembled
 * from. Cross-referenced against `seed_agent_nodes` in
 * packages/daemon/src/services/assembly.rs, which seeds
 * `PromptAssembler::seed_agent_guidance_nodes()` (root_node_type
 * "agent-guidance" — NOT "prompt"; the type was renamed under
 * ADR-064's markdown-children refactor) and `seed_skill_nodes()`
 * (root_node_type "skill"). Update here if a seeded root type changes again.
 */
const SEEDED_GUIDANCE_TYPES = ["agent-guidance", "skill"];

/** The id of a database's settings node, where its provider configs live. */
const DATABASE_SETTINGS_ID = "database-settings-singleton";

/**
 * Read back which seeded prompt/skill content the daemon is actually serving.
 *
 * Seeding is content-versioned (`_seed.key`/`_seed.version` on each seeded
 * node) and only replaces stale content when `seed_nodes_from_templates` runs
 * again, which happens on daemon startup — a long-running daemon started
 * before a guidance edit landed keeps serving the old content indefinitely,
 * and nothing about the daemon being reachable or the right model being
 * loaded would reveal that. This is the eval's own check for exactly that:
 * read every seeded prompt/skill node's version back through the same `node
 * query --json` path an operator would use to check by hand, and record it in
 * provenance so a stale-guidance run cannot be mistaken for a fresh one
 * without re-querying the database.
 *
 * Best-effort: a query failure or a CLI predating `node query` degrades to an
 * empty entry for that type (recorded, not thrown) rather than aborting the
 * run — this check exists to make staleness visible, not to gate the run the
 * way database-isolation or model-match do, since older builds legitimately
 * lack it.
 */
export function readGuidanceProvenance(env: EvalEnv): GuidanceProvenance {
  const guidance: GuidanceProvenance = {};
  for (const nodeType of SEEDED_GUIDANCE_TYPES) {
    const r = Bun.spawnSync(
      [
        env.nsBin,
        "--socket",
        env.socket,
        "--json",
        "node",
        "query",
        "--type",
        nodeType,
        "--limit",
        "200",
      ],
      { stdout: "pipe", stderr: "pipe" },
    );
    guidance[nodeType] =
      r.exitCode === 0 ? extractSeedEntries(r.stdout.toString()) : [];
  }
  return guidance;
}

/**
 * Pull `{key, version}` out of `node query --json` output for one node type.
 *
 * Separated from `readGuidanceProvenance` so the parsing — the part that can
 * actually have a bug — is unit-testable against a fixed JSON string, without
 * spawning the CLI or a daemon. Returns `[]` (never throws) on unparseable
 * input or a node missing `_seed.key`, matching the caller's best-effort
 * contract: a node with no seed metadata is not "this type's guidance is
 * unseeded," it just is not counted.
 */
export function extractSeedEntries(
  queryJson: string,
): Array<{ key: string; version: string }> {
  let parsed: { nodes?: Array<{ properties?: Record<string, unknown> }> };
  try {
    parsed = JSON.parse(queryJson);
  } catch {
    return [];
  }
  return (parsed.nodes ?? [])
    .map((n) => {
      const seed = n.properties?._seed as
        | { key?: string; version?: string }
        | undefined;
      return seed?.key
        ? { key: seed.key, version: seed.version ?? "(no version)" }
        : null;
    })
    .filter((x): x is { key: string; version: string } => x !== null);
}

/**
 * The provider config a remote model id names: `<config-id>` in
 * `openai-compat:<config-id>[:<model>]`.
 */
export function remoteConfigId(model: string): string {
  return model.slice(REMOTE_MODEL_PREFIX.length).split(":")[0] ?? "";
}

/**
 * The ids of the provider configs in a `node get --json` payload of the
 * database settings node. Where the providers sit in the payload depends on how
 * the store hoists the settings fields, so this finds the `providers` array
 * wherever it is. Returns `[]` on unparseable input or no providers.
 */
export function providerIdsIn(settingsJson: string): string[] {
  const find = (value: unknown): unknown[] | undefined => {
    if (typeof value !== "object" || value === null) return undefined;
    for (const [key, inner] of Object.entries(value)) {
      if (key === "providers" && Array.isArray(inner)) return inner;
      const found = find(inner);
      if (found) return found;
    }
    return undefined;
  };
  let parsed: unknown;
  try {
    parsed = JSON.parse(settingsJson);
  } catch {
    return [];
  }
  return (find(parsed) ?? [])
    .map((p) => (p as { id?: unknown } | null)?.id)
    .filter((id): id is string => typeof id === "string");
}

/**
 * A remote model needs the provider it names to be configured on the database
 * the daemon serves; otherwise every turn fails to find its endpoint, makes no
 * tool calls, and a negative assertion passes.
 */
function assertRemoteProvider(env: EvalEnv): void {
  const configId = remoteConfigId(env.model);
  const r = Bun.spawnSync(
    [
      env.nsBin,
      "--socket",
      env.socket,
      "--json",
      "node",
      "get",
      DATABASE_SETTINGS_ID,
    ],
    { stdout: "pipe", stderr: "pipe" },
  );
  const ids = r.exitCode === 0 ? providerIdsIn(r.stdout.toString()) : [];
  if (!ids.includes(configId)) {
    throw new EnvironmentError(
      `NS_MODEL names provider config "${configId}", but the served database ` +
        `configures ${ids.length === 0 ? "none" : ids.join(", ")}. Every turn would ` +
        `fail to reach an endpoint and call no tools.`,
      `Add the provider to the isolated database's settings (never to the real\n` +
        `  one), then use its id in NS_MODEL=openai-compat:<config-id>:<model>.`,
    );
  }
}

/**
 * Assert the environment can produce a valid result.
 *
 * `systemPromptTokens` is the eval's own system-prompt size — the thing the
 * granted window must exceed. Throws EnvironmentError on the first failure.
 */
export function preflight(
  env: EvalEnv,
  status: DaemonStatus,
  systemPromptTokens: number = AGENT_SYSTEM_PROMPT_TOKENS,
): void {
  // 2. The eval must not be writing into the real user database. Scenarios
  // create schemas, chat nodes, and instances; run against `~/.nodespace` they
  // land in live data and cannot be cleanly undone. This is easy to do by
  // accident, because the registry overrides `NODESPACED_DB_PATH` and the
  // daemon starts perfectly happily either way.
  const realHome = `${process.env.HOME ?? ""}/.nodespace/`;
  if (process.env.HOME && status.databasePath.startsWith(realHome)) {
    throw new EnvironmentError(
      `The daemon is serving the REAL user database (${status.databasePath}).\n` +
        `  This eval creates schemas, chat nodes, and instances; running it here would ` +
        `write test data into live user data.`,
      `Isolate the daemon with NODESPACE_HOME (not NODESPACED_DB_PATH, which the\n` +
        `  database registry overrides). The embedding model follows NODESPACE_HOME\n` +
        `  too, and provider settings (API keys) are fields of each database's own\n` +
        `  settings node, so the isolated daemon's database holds no provider keys.\n` +
        `  Name your own model copy for skill retrieval:\n` +
        `    NODESPACE_HOME=/tmp/nodespaced-test \\\n` +
        `      NODESPACED_MODEL_PATH=$HOME/.nodespace/models/nomic-embed-text-v1.5.Q8_0.gguf \\\n` +
        `      NODESPACED_SOCKET=${env.socket} target/release/nodespaced\n` +
        `  Then confirm with: grep served_db_path <daemon log>`,
    );
  }

  // 3. A remote model must name a provider configured on this database. It is
  // loaded like a local one (the daemon connects to the endpoint), so the
  // checks below apply to it too, except for the window it is granted.
  const remote = providerOf(env.model) === "openai-compat";
  if (remote) assertRemoteProvider(env);

  // 3. A model must be loaded, and it must be the one being scored.
  if (!status.loaded) {
    throw new EnvironmentError(
      "The daemon is running but has no chat model loaded, so every turn would " +
        "fail before inference.",
      `Load one: ${env.nsBin} --socket ${env.socket} model load ${env.model}`,
    );
  }

  // The daemon normally reports the catalog id it loaded the model BY, so exact
  // equality holds. It falls back to the resolved GGUF path only when the id is
  // unavailable, hence the basename fallback.
  //
  // Substring matching is deliberately NOT used: `NS_MODEL=gemma-4-e4b` would
  // then also accept a loaded `gemma-4-e4b-q8`. Quantization changes tool-calling
  // behavior materially, so that is exactly the "scores filed against a model
  // that did not produce them" case this check exists to stop.
  if (status.modelId !== env.model) {
    const basename = status.modelId.split("/").pop() ?? "";
    const matchesPath = basename
      .toLowerCase()
      .includes(env.model.toLowerCase());
    if (!matchesPath) {
      throw new EnvironmentError(
        `The daemon has "${status.modelId}" loaded, but this run is labelled for ` +
          `"${env.model}". The scores would be filed against a model that did not produce them.`,
        `Load the requested model:\n` +
          `    ${env.nsBin} --socket ${env.socket} model load ${env.model}\n` +
          `  or set NS_MODEL to what is already loaded.`,
      );
    }
    // Matched a path rather than an id, so the exact build cannot be confirmed
    // from the id alone. Proceed, but say so — a silent near-match is how a run
    // gets filed under the wrong quantization.
    status.modelMatchedByPath = true;
    console.error(
      `[preflight] Warning: the daemon reports "${status.modelId}" (a path, not a ` +
        `catalog id). Matched "${env.model}" by filename; the exact build could not ` +
        `be confirmed.`,
    );
  }

  // 4. The granted window must hold the system prompt with room to work. A
  // remote model's window is the provider's, not one the daemon grants.
  const required = systemPromptTokens + CONTEXT_HEADROOM_TOKENS;
  if (!remote && status.grantedNCtx < required) {
    throw new EnvironmentError(
      `The granted context window (${status.grantedNCtx} tokens) is too small for this ` +
        `eval's system prompt (~${systemPromptTokens} tokens plus ${CONTEXT_HEADROOM_TOKENS} ` +
        `tokens of headroom for history and tool results).\n` +
        `  Every turn would die on context overflow before inference runs. Scenarios that ` +
        `assert "no tools called" would then score as PASSING, so the run would report a ` +
        `plausible partial result from an environment where nothing worked.`,
      `The window is sized to host memory at load time (host RAM here: ` +
        `${(status.hostRamBytes / 1e9).toFixed(1)} GB). Free memory and reload the model, ` +
        `or use a smaller one.`,
    );
  }
}

/** Print an EnvironmentError and exit — never as scenario results. */
export function abortOnEnvironment(
  evalName: string,
  e: EnvironmentError,
): never {
  console.error(
    `\n── ${evalName}: ENVIRONMENT NOT USABLE ─────────────────────────────`,
  );
  console.error(e.message);
  console.error(`\n  How to fix:\n  ${e.remedy}`);
  console.error(
    `\n  No results file was written: a score from this environment would not mean\n` +
      `  anything.`,
  );
  console.error(
    `────────────────────────────────────────────────────────────────────\n`,
  );
  process.exit(EXIT_ENVIRONMENT);
}
