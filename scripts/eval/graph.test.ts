/**
 * Unit tests for the snapshot's type selection.
 *
 * The harness drives every scenario through a chat node of its own, which
 * gains message nodes during the scored turn. If a snapshot queried the
 * chat's type or its messages', every read-only scenario would report a
 * changed or new node and fail `expectNoWrites` on the harness's own
 * bookkeeping.
 */

import { afterAll, describe, expect, test } from "bun:test";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CHAT_NODE_TYPE, MESSAGE_NODE_TYPE } from "../aichat.ts";
import type { EvalEnv } from "./env.ts";
import {
  QUERY_PAGE,
  captureSnapshot,
  listedSchemaIds,
  snapshotBlindness,
  snapshotQueryTypes,
} from "./graph.ts";

/** Temp directories the fake CLIs live in, removed when the file is done. */
const tempDirs: string[] = [];
afterAll(() => {
  for (const d of tempDirs) rmSync(d, { recursive: true, force: true });
});

describe("snapshotQueryTypes", () => {
  test("leaves out the whole chat family and date containers", () => {
    const schemas = [
      "ai-chat",
      "ai-chat-native",
      "ai-chat-pty",
      "ai-chat-message",
      "date",
      "task",
      "text",
    ];
    expect(snapshotQueryTypes([], schemas)).toEqual(["task", "text"]);
  });

  test("a predicted type is queried once, and scaffolding is dropped from it too", () => {
    expect(
      snapshotQueryTypes(["invoice", "task", "ai-chat-native"], ["task"]),
    ).toEqual(["invoice", "task"]);
  });

  test("the type the harness creates its chat as is excluded", () => {
    // `aichat.ts` creates the chat every scenario runs through; a change to
    // its type must not leave the snapshot counting it.
    expect(snapshotQueryTypes([], [CHAT_NODE_TYPE])).toEqual([]);
  });

  test("the type the harness creates its messages as is excluded", () => {
    expect(snapshotQueryTypes([MESSAGE_NODE_TYPE], [MESSAGE_NODE_TYPE])).toEqual([]);
  });
});

describe("listedSchemaIds", () => {
  test("reads ids from the schemas array the CLI emits", () => {
    expect(listedSchemaIds({ count: 2, schemas: [{ id: "task" }, { id: "text" }] })).toEqual(["task", "text"]);
  });

  test("a nodes array, which schema list does not emit, reads as no schemas", () => {
    expect(listedSchemaIds({ nodes: [{ id: "task" }] })).toEqual([]);
  });
});

describe("captureSnapshot against a fake CLI", () => {
  /** A `nodespace` stand-in answering from canned JSON by its arguments. */
  function fakeCli(extra: string): EvalEnv {
    const dir = mkdtempSync(join(tmpdir(), "fake-ns-"));
    tempDirs.push(dir);
    const bin = join(dir, "nodespace");
    writeFileSync(
      bin,
      `#!/bin/sh
case "$*" in
  *"schema list"*) echo '{"count":2,"schemas":[{"id":"task"},{"id":"text"}]}';;
  *"node query --type task"*) echo '{"nodes":[{"id":"t1","node_type":"task","content":"a task","properties":{}}]}';;
  *"node query --type text"*) echo '{"nodes":[{"id":"x1","node_type":"text","content":"a note","properties":{}}]}';;
${extra}
  *"relationship get t1 --type assignee"*) echo '{"nodes":[{"id":"p1"}]}';;
  *"relationship get"*) echo '{"nodes":[]}';;
  *) echo "unexpected: $*" >&2; exit 2;;
esac
`,
    );
    chmodSync(bin, 0o755);
    return { nsBin: bin, socket: "/unused", log: "/unused" } as EvalEnv;
  }

  const undeclared = `  *"relationship get x1 --type assignee"*) echo "Error: Relationship 'assignee' is not declared for node type 'text' in either direction" >&2; exit 1;;`;

  test("queries the types the schema list names, so a written node is seen", () => {
    const snap = captureSnapshot(fakeCli(undeclared), [], { relations: ["assignee"] });
    expect(snap.captureError).toBeUndefined();
    expect(snap.nodes.map((n) => n.id).sort()).toEqual(["t1", "x1"]);
  });

  test("a relation the node's type does not declare is no edge, not a failure", () => {
    const snap = captureSnapshot(fakeCli(undeclared), [], { relations: ["assignee"] });
    expect(snap.captureError).toBeUndefined();
    expect(snap.edges).toEqual([{ from: "t1", relation: "assignee", to: "p1" }]);
  });

  test("any other relationship failure still fails the capture", () => {
    const dead = `  *"relationship get x1 --type assignee"*) echo "Error: transport closed" >&2; exit 1;;`;
    const snap = captureSnapshot(fakeCli(dead), [], { relations: ["assignee"] });
    expect(snap.captureError).toContain("transport closed");
  });
});

describe("captureSnapshot paging", () => {
  /**
   * A CLI whose `text` type holds `total` nodes, served `QUERY_PAGE` at a time
   * by `--offset` the way the daemon does, and nothing else.
   */
  function pagedCli(total: number, failFrom?: number): EvalEnv {
    const dir = mkdtempSync(join(tmpdir(), "fake-ns-paged-"));
    tempDirs.push(dir);
    const bin = join(dir, "nodespace");
    writeFileSync(
      bin,
      `#!/bin/sh
off=0
prev=""
for a in "$@"; do
  [ "$prev" = "--offset" ] && off="$a"
  prev="$a"
done
case "$*" in
  *"schema list"*) echo '{"count":1,"schemas":[{"id":"text"}]}';;
  *"node query --type text"*)
    ${failFrom === undefined ? "" : `[ "$off" -ge ${failFrom} ] && { echo "Error: transport closed" >&2; exit 1; }`}
    n=$(( ${total} - off ))
    [ "$n" -gt ${QUERY_PAGE} ] && n=${QUERY_PAGE}
    [ "$n" -lt 0 ] && n=0
    printf '{"nodes":['
    i=0
    while [ "$i" -lt "$n" ]; do
      [ "$i" -gt 0 ] && printf ','
      printf '{"id":"n%s","node_type":"text","content":"c","properties":{}}' $(( off + i ))
      i=$(( i + 1 ))
    done
    printf ']}\\n';;
  *) echo '{"nodes":[]}';;
esac
`,
    );
    chmodSync(bin, 0o755);
    return { nsBin: bin, socket: "/unused", log: "/unused" } as EvalEnv;
  }

  test("a type larger than one page is read in full, newest node included", () => {
    const total = QUERY_PAGE * 2 + 10;
    const snap = captureSnapshot(pagedCli(total), [], { edgesFor: () => false });
    expect(snap.captureError).toBeUndefined();
    expect(snap.nodes.length).toBe(total);
    expect(snap.nodes.some((n) => n.id === `n${total - 1}`)).toBe(true);
  });

  test("a type of exactly one page is read in full", () => {
    const snap = captureSnapshot(pagedCli(QUERY_PAGE), [], { edgesFor: () => false });
    expect(snap.captureError).toBeUndefined();
    expect(snap.nodes.length).toBe(QUERY_PAGE);
  });

  test("a later page that fails fails the capture instead of reading short", () => {
    const snap = captureSnapshot(pagedCli(QUERY_PAGE * 2, QUERY_PAGE), [], {
      edgesFor: () => false,
    });
    expect(snap.captureError).toContain("transport closed");
  });
});

describe("snapshotBlindness", () => {
  const node = { id: "n", node_type: "text", content: "", properties: {} };
  const healthy = { nodes: [node], schemas: ["text"], edges: [] };

  test("a snapshot with schemas and nodes is not blind", () => {
    expect(snapshotBlindness(healthy)).toBeNull();
  });

  test("no schemas is blind, which is what reading the wrong list shape produced", () => {
    expect(snapshotBlindness({ ...healthy, schemas: [] })).toContain("no schemas");
  });

  test("no nodes in a seeded database is blind", () => {
    expect(snapshotBlindness({ ...healthy, nodes: [] })).toContain("no nodes");
  });

  test("a failed capture is reported with its cause", () => {
    expect(snapshotBlindness({ ...healthy, captureError: "daemon gone" })).toContain(
      "daemon gone",
    );
  });
});
