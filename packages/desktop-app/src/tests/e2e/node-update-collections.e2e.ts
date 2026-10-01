/**
 * E2E: collection membership changes on a node update, via
 * dev-proxy → nodespaced → SQLite.
 *
 * The adapter's `updateNode` carries no collection fields, so these tests
 * send the `PATCH` themselves: what is under test is the proxy's mapping of
 * the request body onto `UpdateNodeRequest`'s repeated collection fields.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { DaemonTestHarness } from './daemon-harness';

let h: DaemonTestHarness;

beforeAll(async () => {
  h = await DaemonTestHarness.start();
}, 15_000);

afterAll(async () => {
  await h?.stop();
});

interface CollectionSummary {
  id: string;
  content: string;
}

async function patchNode(id: string, body: Record<string, unknown>): Promise<Response> {
  return fetch(`${h.baseUrl}/api/nodes/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
}

async function collectionIdByName(name: string): Promise<string | undefined> {
  const res = await fetch(`${h.baseUrl}/api/collections`);
  const collections = (await res.json()) as CollectionSummary[];
  return collections.find((c) => c.content === name)?.id;
}

async function memberIds(collectionId: string): Promise<string[]> {
  const res = await fetch(
    `${h.baseUrl}/api/collections/${encodeURIComponent(collectionId)}/members`
  );
  const members = (await res.json()) as Array<{ id: string }>;
  return members.map((m) => m.id);
}

async function createTextNode(content: string): Promise<string> {
  const id = crypto.randomUUID();
  await h.adapter.createNode({ id, nodeType: 'text', content });
  return id;
}

describe('Node update collection membership (HTTP → gRPC → SQLite)', () => {
  it('adds a node to a collection by path, creating the collection', async () => {
    const nodeId = await createTextNode('joins by path');
    const name = `by-path-${crypto.randomUUID()}`;

    const res = await patchNode(nodeId, { version: 1, addToCollections: [name] });
    expect(res.status).toBe(200);

    const collectionId = await collectionIdByName(name);
    expect(collectionId).toBeDefined();
    expect(await memberIds(collectionId!)).toContain(nodeId);
  });

  it('adds a node to a collection by id, then removes it by id', async () => {
    const seedId = await createTextNode('creates the collection');
    const nodeId = await createTextNode('joins by id');
    const name = `by-id-${crypto.randomUUID()}`;
    await patchNode(seedId, { version: 1, addToCollections: [name] });
    const collectionId = (await collectionIdByName(name))!;

    const added = await patchNode(nodeId, { addToCollectionIds: [collectionId] });
    expect(added.status).toBe(200);
    expect(await memberIds(collectionId)).toContain(nodeId);

    const removed = await patchNode(nodeId, { removeFromCollectionIds: [collectionId] });
    expect(removed.status).toBe(200);
    const remaining = await memberIds(collectionId);
    expect(remaining).not.toContain(nodeId);
    expect(remaining).toContain(seedId);
  });

  it('rejects a path add and an id add on one request', async () => {
    const seedId = await createTextNode('creates the collection');
    const nodeId = await createTextNode('sends both');
    const name = `both-${crypto.randomUUID()}`;
    await patchNode(seedId, { version: 1, addToCollections: [name] });
    const collectionId = (await collectionIdByName(name))!;
    const otherName = `both-other-${crypto.randomUUID()}`;

    const res = await patchNode(nodeId, {
      addToCollections: [otherName],
      addToCollectionIds: [collectionId]
    });

    expect(res.status).toBe(400);
    expect(await memberIds(collectionId)).not.toContain(nodeId);
    expect(await collectionIdByName(otherName)).toBeUndefined();
  });

  it('rejects a collection field that is not an array of strings', async () => {
    const nodeId = await createTextNode('sends a bare string');

    const res = await patchNode(nodeId, { addToCollections: 'not-a-list' });

    expect(res.status).toBe(400);
    const body = (await res.json()) as { code: string };
    expect(body.code).toBe('INVALID_ARGUMENT');
  });
});
