/**
 * Graph end-state capture and comparison.
 *
 * WHY THIS EXISTS
 *
 * The matrix used to score TRAJECTORY — which tool fired, how many times, in
 * what order. That disagrees with the product in three measured ways:
 *
 *   - A correct result scored as a failure. A model that reached the right end
 *     state by a shorter path (update_node without a preceding resolve_query)
 *     lost a point for the path, not the result.
 *   - Self-correction scored as a failure. A rejected create_node, corrected by
 *     the model and persisted on the second call, red-lined on an exactly-once
 *     rule — punishing the recovery behavior we want.
 *   - Severity flattened. Two search_nodes calls (wasted latency, nothing
 *     persisted) scored identically to two create_schema calls (a spurious type
 *     in the user's graph) and to zero search_nodes on a query (a wrong answer).
 *
 * Asserting on the graph's END STATE fixes all three at once, and the third
 * falls out for free rather than needing a severity table: two search_nodes
 * calls change nothing and pass, two create_schema calls leave an unexpected
 * type behind and fail the same `noUnexpectedSchemas` clause that every other
 * write scenario already carries.
 *
 * This is tau-bench's design — the agent operates on a database through an API
 * and is graded by diffing final DB state against a goal state — and it is the
 * closest published analogue to what NodeSpace's agent actually does.
 *
 * WHAT IS AND IS NOT ASSERTED
 *
 * A snapshot is deliberately NOT a whole-database dump. It covers what the
 * scenarios act on — nodes (with their properties), schemas, and edges between
 * observed nodes — read back through the same `nodespace` CLI every eval turn
 * already goes through. Anything outside that (embeddings, collections,
 * lifecycle transitions) is invisible here by design: an assertion this module
 * cannot express belongs in a scenario's trajectory diagnostic, not in a
 * silently-passing end-state clause.
 */

import type { EvalEnv } from "./env.ts";

/** One node as the CLI reports it, narrowed to the fields assertions read. */
export interface SnapshotNode {
  id: string;
  node_type: string;
  content: string;
  properties: Record<string, unknown>;
}

/** One edge, as discovered by traversing a known node's relationships. */
export interface SnapshotEdge {
  from: string;
  relation: string;
  to: string;
}

/**
 * The graph as it stood at one instant, scoped to what the eval acts on.
 *
 * `schemas` is a list of schema ids (node types the user can record against)
 * rather than the full definitions: every scenario's end-state clause asks
 * whether a type EXISTS or whether an unexpected one appeared, and neither
 * needs the field list. `schemaCallsAreSound` remains the trajectory-side
 * assertion for "the type persisted with usable fields".
 */
export interface GraphSnapshot {
  nodes: SnapshotNode[];
  schemas: string[];
  edges: SnapshotEdge[];
  /**
   * The snapshot could not be captured (the CLI failed, the daemon went away).
   *
   * Recorded rather than thrown, and distinguished from an empty graph: an
   * empty snapshot would make "no unexpected nodes" pass and "node exists"
   * fail, silently converting an environment fault into a model verdict. Every
   * end-state assertion checks this first and declines to score.
   */
  captureError?: string;
}

/**
 * Relationship names traversed when discovering edges.
 *
 * The daemon exposes relationship reads per (node, relation) pair rather than
 * as a whole-graph edge dump, so discovery has to name the relations it walks.
 * These are the four the validator treats as universal — legal between any two
 * nodes regardless of their schemas (see node_service/relationship.rs's "are
 * universal" error). A custom relation defined on a user type is NOT walked:
 * no scenario asserts one today, and probing invented names would multiply CLI
 * round-trips per snapshot for edges nothing reads.
 */
const UNIVERSAL_RELATIONS = ["member_of", "has_child", "mentions", "has_role"];

/**
 * Node types excluded from a snapshot.
 *
 * Chat nodes are the eval's own scaffolding — the runner creates one per group
 * and every turn appends message nodes under it — so counting them as graph
 * state would fail `noUnexpectedNodes` on every scenario for the harness's own
 * bookkeeping. The whole chat family is listed, with the message type: a chat
 * is always one of the subtypes, and a query for the abstract `ai-chat` base
 * returns them all.
 * `date` nodes are auto-created by the daemon as containers rather than by any
 * model action, and would fail the same clause for the same reason.
 */
const SCAFFOLDING_TYPES = [
  "ai-chat",
  "ai-chat-native",
  "ai-chat-pty",
  "ai-chat-message",
  "date",
];

/**
 * The types a snapshot queries: the caller's predicted types plus every
 * registered schema, without the harness's own scaffolding.
 */
export function snapshotQueryTypes(types: string[], schemas: string[]): string[] {
  return [...new Set([...types, ...schemas])].filter(
    (t) => !SCAFFOLDING_TYPES.includes(t),
  );
}

/**
 * Nodes pulled per `node query` page.
 *
 * The daemon clamps a query to 500 rows without saying so, so asking for more
 * returns 500 and looks like the whole list. A fresh database already holds a
 * few hundred `text` nodes (the bodies of the seeded skills and guidance), and
 * rows come back oldest first, so a read cut short drops the node a turn just
 * wrote and a scenario asserting nothing was written passes over a write. A
 * type is therefore read a page at a time until a page comes back short.
 */
export const QUERY_PAGE = 500;

/** Pages read for one type before the capture gives up rather than loop. */
const MAX_PAGES = 100;

function runCli(env: EvalEnv, args: string[]): unknown {
  const r = Bun.spawnSync(
    [env.nsBin, "--socket", env.socket, "--json", ...args],
    { stdout: "pipe", stderr: "pipe" },
  );
  if (r.exitCode !== 0) {
    throw new Error(
      `nodespace ${args.join(" ")} failed (exit ${r.exitCode}): ${r.stderr
        .toString()
        .trim()}`,
    );
  }
  const out = r.stdout.toString().trim();
  return out ? JSON.parse(out) : null;
}

/**
 * Bookkeeping keys the daemon writes itself. Excluded from the flattened view
 * so `minProperties` counts what the MODEL wrote, not what persistence added:
 * a node whose only real field is `due_date` would otherwise satisfy
 * `minProperties: 2` purely because `_schema_version` rode along beside it.
 */
const PERSISTENCE_KEYS = new Set(["_schema_version"]);

/**
 * Lift a node's properties out of their type-keyed wrapper.
 *
 * The daemon serialises typed properties nested under the node's own type:
 *
 *     {"task": {"_schema_version": 1, "due_date": "2026-08-06", "status": "open"}}
 *
 * Every consumer here - `nodeSatisfies`, `populatedCount`, and the two failure
 * renderers - asks questions about the INNER keys (`due_date`, `status`), so
 * passing the wrapper through means a keyed lookup finds nothing and
 * `populatedCount` returns 1 (the wrapper) no matter how many fields were
 * written. That made `minProperties: 2` unsatisfiable by construction and red-
 * lined correct writes: measured against DeepSeek V4 Pro, scenarios 9, 10b and
 * 13 each failed 3/3 reps on writes that had persisted exactly what was asked.
 *
 * Unwrapping here rather than at each call site keeps the snapshot the single
 * definition of "this node's properties", so matching, counting and the failure
 * messages cannot drift apart.
 *
 * Only the node's OWN type key is lifted, and only when it holds an object.
 * A node that stores flat properties, or one carrying a key that merely shares
 * a name with some other type, is left exactly as it was - the wrapper is
 * identified by matching `node_type`, not by guessing from shape.
 *
 * NOTE: the CLI now flattens properties itself (`output.rs::node_to_json`), so
 * against a current build this function's wrapper branch never fires and it
 * simply copies. It is kept because `NS_BIN` is overridable and defaults to a
 * prebuilt `target/release/nodespace` that can predate that change — exactly
 * the stale-binary case `preflight.ts` already warns about. Unwrapping a shape
 * the CLI no longer emits is harmless; failing to unwrap one an older binary
 * still does would silently zero out `populatedCount` and red-line correct
 * writes. This is input tolerance at the harness boundary, not a compatibility
 * shim in the product.
 */
export function flattenTypeKeyedProperties(
  props: Record<string, unknown>,
  nodeType: string,
): Record<string, unknown> {
  const wrapped = props[nodeType];
  const isWrapper =
    typeof wrapped === "object" && wrapped !== null && !Array.isArray(wrapped);

  // Keys beside the wrapper are real properties and must survive: the daemon is
  // not required to put everything inside it, and dropping a sibling here would
  // trade one silent-miss bug for another.
  //
  // PRECEDENCE on a name collision — `{task: {status: "inner"}, status: "outer"}`
  // — the wrapper wins, because it is the one the daemon writes typed field
  // values into; a same-named sibling is the anomaly. No daemon serialization
  // produces this today, so the rule is stated rather than defended.
  const merged: Record<string, unknown> = isWrapper
    ? { ...omit(props, nodeType), ...(wrapped as Record<string, unknown>) }
    : { ...props };

  for (const k of PERSISTENCE_KEYS) delete merged[k];
  return merged;
}

function omit(o: Record<string, unknown>, key: string): Record<string, unknown> {
  const { [key]: _dropped, ...rest } = o;
  return rest;
}

/** Narrow one CLI node object into a SnapshotNode, tolerating absent fields. */
export function toSnapshotNode(raw: unknown): SnapshotNode | null {
  if (typeof raw !== "object" || raw === null) return null;
  const o = raw as Record<string, unknown>;
  if (typeof o.id !== "string" || typeof o.node_type !== "string") return null;
  // `properties` inlines as nested JSON (output.rs::node_to_json), but degrades
  // to a raw string if the daemon's encoding ever breaks. Treat a non-object as
  // "no properties" rather than crashing the snapshot.
  const rawProps =
    typeof o.properties === "object" &&
    o.properties !== null &&
    !Array.isArray(o.properties)
      ? (o.properties as Record<string, unknown>)
      : {};
  const props = flattenTypeKeyedProperties(rawProps, o.node_type);
  return {
    id: o.id,
    node_type: o.node_type,
    content: typeof o.content === "string" ? o.content : "",
    properties: props,
  };
}

/** Pull the `nodes` array out of a CLI node-list payload. */
export function readNodeList(payload: unknown): SnapshotNode[] {
  if (typeof payload !== "object" || payload === null) return [];
  const nodes = (payload as Record<string, unknown>).nodes;
  if (!Array.isArray(nodes)) return [];
  return nodes.map(toSnapshotNode).filter((n): n is SnapshotNode => n !== null);
}

/**
 * The schema ids in `schema list --json` output, which carries them under
 * `schemas`, not `nodes`. Pure, so the shape is pinned by a test: a reader of
 * a shape the CLI does not emit finds no schema at all, and a snapshot that
 * queries no type is empty, so every "nothing was written" clause passes
 * vacuously and every "was created" clause fails.
 */
export function listedSchemaIds(output: unknown): string[] {
  const schemas = (output as { schemas?: unknown } | null)?.schemas;
  if (!Array.isArray(schemas)) return [];
  return schemas.flatMap((s) => {
    const id = (s as { id?: unknown } | null)?.id;
    return typeof id === "string" && id !== "" ? [id] : [];
  });
}

/** Pull target ids out of a `relationship get` payload. */
export function readRelatedIds(payload: unknown): string[] {
  if (typeof payload !== "object" || payload === null) return [];
  const related = (payload as Record<string, unknown>).nodes;
  if (!Array.isArray(related)) return [];
  const ids: string[] = [];
  for (const r of related) {
    if (typeof r === "string") {
      ids.push(r);
      continue;
    }
    if (typeof r === "object" && r !== null) {
      const id = (r as Record<string, unknown>).id;
      if (typeof id === "string") ids.push(id);
    }
  }
  return ids;
}

/**
 * Capture the graph's current state.
 *
 * `types` is a SEED set the caller supplies, because the daemon has no "query
 * every node" verb. It is not the coverage boundary: `schema list` is
 * enumerated first and folded into the query set, so a type the fixture never
 * anticipated still lands in the snapshot.
 *
 * That makes the enumeration COMPLETE, which is what the negative clauses
 * (`expectNoWrites`, `noUnexpectedNodes`) rest on — they would silently pass
 * on any write this function cannot see. The daemon rejects a node whose type
 * has no schema outright ("Unknown node_type 'x': no such core type or schema.
 * Create the schema first"), verified against a live daemon, so every node
 * that can exist has a registered type and every registered type is queried.
 * A model inventing a custom type must call create_schema first, which puts
 * the type in `schema list` before any instance of it can exist.
 */
export function captureSnapshot(
  env: EvalEnv,
  types: string[],
  options: {
    edgesFor?: (n: SnapshotNode) => boolean;
    /**
     * Relations walked beside the universal ones: names a fixture's scenarios
     * assert that a type declares (`blocks`, or the derived `assignee` and
     * `project` of a task). Each adds one CLI round-trip per walked node.
     */
    relations?: string[];
  } = {},
): GraphSnapshot {
  try {
    // A schema node's id IS the type identifier it defines (`schema get` takes
    // the same value), so the ids double as the list of queryable types.
    const schemas = listedSchemaIds(runCli(env, ["schema", "list"])).sort();

    const queryTypes = snapshotQueryTypes(types, schemas);

    const nodes: SnapshotNode[] = [];
    const seen = new Set<string>();
    for (const t of queryTypes) {
      // A type with no instances is a normal outcome and the CLI reports it as
      // an empty list. A type that does not exist at all can error instead —
      // tolerated for the same reason, since the caller's predicted type set
      // intentionally names types a given run may never create.
      const queryPage = (offset: number): unknown =>
        runCli(env, [
          "node",
          "query",
          "--type",
          t,
          "--limit",
          String(QUERY_PAGE),
          "--offset",
          String(offset),
        ]);
      let payload: unknown;
      try {
        payload = queryPage(0);
      } catch (e) {
        // Only a type the schema list does not name may be absent. One it
        // does name exists, so a failure reading it is a transport fault, and
        // dropping the type silently would leave its writes unseen.
        if (schemas.includes(t)) throw e;
        continue;
      }
      for (let page = 0; ; page++) {
        const listed = readNodeList(payload);
        for (const n of listed) {
          if (seen.has(n.id)) continue;
          seen.add(n.id);
          nodes.push(n);
        }
        if (listed.length < QUERY_PAGE) break;
        // Not tolerated like the first page: a later page that fails, or a
        // type too large to read, would leave the list short without a sign,
        // which is how a write goes unseen.
        if (page + 1 >= MAX_PAGES) {
          throw new Error(
            `type '${t}' has more than ${MAX_PAGES * QUERY_PAGE} nodes; the ` +
              `snapshot would miss the ones past them`,
          );
        }
        payload = queryPage((page + 1) * QUERY_PAGE);
      }
    }

    const edges: SnapshotEdge[] = [];
    const walk = options.edgesFor ?? (() => true);
    const walked = [
      ...new Set([...UNIVERSAL_RELATIONS, ...(options.relations ?? [])]),
    ];
    for (const n of nodes) {
      if (!walk(n)) continue;
      for (const rel of walked) {
        // Deliberately NOT caught, unlike the node query above.
        //
        // That asymmetry is the point. A node query can fail for an expected
        // reason (the seed set names types a run may never create), but
        // `relationship get` is called on an id the daemon just handed back,
        // for a relation that is universal by definition — it has no expected
        // failure. Verified against a live daemon: it exits 0 for an undeclared
        // relation AND for a nonexistent node id, so there is nothing
        // legitimate here to tolerate.
        //
        // Swallowing it would therefore only ever hide a daemon that died
        // mid-walk, and the snapshot would come back clean with a truncated
        // edge list — scoring 11c as "no 'mentions' edge was recorded", a dead
        // daemon filed as a model failure. Letting it reach the outer catch
        // sets `captureError`, and every assertion then declines to score.
        //
        // The one exception is a relation the fixture named (`relations`): it
        // is declared on some types and not others, and the daemon refuses it
        // on a node whose type does not declare it. That refusal, and only
        // that one, means "this node has no such edge"; any other failure is
        // still the dead daemon above and still fails the capture.
        let payload: unknown;
        try {
          payload = runCli(env, ["relationship", "get", n.id, "--type", rel]);
        } catch (e) {
          const declared = UNIVERSAL_RELATIONS.includes(rel);
          if (!declared && /not declared for node type/i.test(String(e))) continue;
          throw e;
        }
        for (const target of readRelatedIds(payload)) {
          edges.push({ from: n.id, relation: rel, to: target });
        }
      }
    }

    return { nodes, schemas, edges };
  } catch (e) {
    return {
      nodes: [],
      schemas: [],
      edges: [],
      captureError: e instanceof Error ? e.message : String(e),
    };
  }
}

/**
 * Why a snapshot of a freshly seeded database cannot be trusted, or null.
 *
 * A capture that queries no type returns an empty snapshot without erroring,
 * and every "nothing was written" clause then passes over a write while every
 * "was created" clause fails. A database the daemon has just seeded always
 * holds schemas and nodes, so a snapshot with none is blind, not empty.
 */
export function snapshotBlindness(snap: GraphSnapshot): string | null {
  if (snap.captureError) return `it failed: ${snap.captureError}`;
  if (snap.schemas.length === 0) return "it found no schemas";
  if (snap.nodes.length === 0) {
    return "it found no nodes in a database that is seeded with some";
  }
  return null;
}

/**
 * What changed between two snapshots.
 *
 * The unit every end-state assertion scores against: a scenario asserts what
 * ITS OWN turn did, so a node that already existed before the turn must not
 * satisfy a "created" clause, and a spurious type left behind by an earlier
 * scenario must not fail a later one's "no unexpected types" clause.
 */
export interface GraphDiff {
  addedNodes: SnapshotNode[];
  addedSchemas: string[];
  addedEdges: SnapshotEdge[];
  /** Nodes present in both snapshots whose properties or content changed. */
  changedNodes: Array<{ before: SnapshotNode; after: SnapshotNode }>;
  /** Neither snapshot can be trusted — see `GraphSnapshot.captureError`. */
  captureError?: string;
}

function edgeKey(e: SnapshotEdge): string {
  return `${e.from}|${e.relation}|${e.to}`;
}

/**
 * Serialize a value with object keys in a stable order.
 *
 * Plain `JSON.stringify` is key-order sensitive, so a daemon that reserializes
 * a node's properties in a different order would report a change that did not
 * happen. That direction is not harmless: `expectNoWrites` fails on any
 * `changedNodes` entry, so a read scenario would red out because a JSON object
 * came back with its keys in a different order — a false failure on five
 * scenarios, from something the model had no part in.
 *
 * WHAT IT NOW COMPARES: flattened properties, since `toSnapshotNode` lifts the
 * type-keyed wrapper before a node reaches here. `_schema_version` is dropped
 * with it, so a node whose ONLY difference is a persistence-bumped schema
 * version no longer counts as changed (measured: 1 changed node before, 0
 * after). That is the intended direction — `expectNoWrites` should not red-line
 * a read scenario because persistence touched a field the model never saw — but
 * it is a deliberate loosening of the one comparator behind a false-PASS path,
 * so it is recorded here rather than left to be inferred, and pinned by
 * `diffSnapshots` in end-state.test.ts.
 *
 * The loosening is bounded to keys in `PERSISTENCE_KEYS`. Any field a model can
 * actually write still compares exactly.
 *
 * SOUNDNESS RESTS ON THE INPUT DOMAIN, so state it: this is the only
 * comparator behind `changedNodes`, and `changedNodes` is what
 * `expectNoWrites` reads, making it one of the few places a false PASS could
 * originate. It is safe because the values it sees always come from
 * `JSON.parse` of the CLI's stdout (see `toSnapshotNode`), which can only
 * produce objects, arrays, strings, numbers, booleans and null. The `?? "null"`
 * fallback collapses `undefined` with `null`, and a `Date` would render as
 * `{}` — neither is reachable from parsed JSON. Feed this anything that did
 * not come from a parse and that guarantee is gone.
 */
function stableStringify(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(",")}]`;
  if (typeof value === "object" && value !== null) {
    const entries = Object.entries(value as Record<string, unknown>)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([k, v]) => `${JSON.stringify(k)}:${stableStringify(v)}`);
    return `{${entries.join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

export function diffSnapshots(
  before: GraphSnapshot,
  after: GraphSnapshot,
): GraphDiff {
  const captureError = before.captureError ?? after.captureError;
  if (captureError) {
    return {
      addedNodes: [],
      addedSchemas: [],
      addedEdges: [],
      changedNodes: [],
      captureError,
    };
  }

  const beforeById = new Map(before.nodes.map((n) => [n.id, n]));
  const beforeSchemas = new Set(before.schemas);
  const beforeEdges = new Set(before.edges.map(edgeKey));

  const addedNodes = after.nodes.filter((n) => !beforeById.has(n.id));
  const addedSchemas = after.schemas.filter((s) => !beforeSchemas.has(s));
  const addedEdges = after.edges.filter((e) => !beforeEdges.has(edgeKey(e)));

  const changedNodes: GraphDiff["changedNodes"] = [];
  for (const a of after.nodes) {
    const b = beforeById.get(a.id);
    if (!b) continue;
    if (
      b.content !== a.content ||
      stableStringify(b.properties) !== stableStringify(a.properties)
    ) {
      changedNodes.push({ before: b, after: a });
    }
  }

  return { addedNodes, addedSchemas, addedEdges, changedNodes };
}
