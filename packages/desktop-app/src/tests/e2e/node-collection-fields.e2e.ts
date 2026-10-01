/**
 * E2E: collection membership set on a node create or update, via
 * dev-proxy → nodespaced → SQLite.
 *
 * The adapter's `createNode` and `updateNode` carry no collection fields, so
 * these tests send the requests themselves: what is under test is the proxy's
 * mapping of the request body onto the proto's repeated collection fields.
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

interface ErrorBody {
  code: string;
  message: string;
}

function send(method: string, route: string, body: Record<string, unknown>): Promise<Response> {
  return fetch(`${h.baseUrl}${route}`, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
}

function patchNode(id: string, body: Record<string, unknown>): Promise<Response> {
  return send('PATCH', `/api/nodes/${encodeURIComponent(id)}`, body);
}

function postNode(id: string, body: Record<string, unknown>): Promise<Response> {
  return send('POST', '/api/nodes', { id, nodeType: 'text', content: 'created', ...body });
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

/** Create a collection with one member, returning the collection's id. */
async function seedCollection(name: string): Promise<{ collectionId: string; seedId: string }> {
  const seedId = await createTextNode('creates the collection');
  const res = await patchNode(seedId, { addToCollections: [name] });
  expect(res.status).toBe(200);
  const collectionId = await collectionIdByName(name);
  expect(collectionId).toBeDefined();
  return { collectionId: collectionId!, seedId };
}

async function expectPathsAndIdsRefused(res: Response): Promise<void> {
  expect(res.status).toBe(400);
  const body = (await res.json()) as ErrorBody;
  expect(body.code).toBe('INVALID_ARGUMENT');
  expect(body.message).toContain('not both');
}

async function expectNotAStringList(res: Response, field: string): Promise<void> {
  expect(res.status).toBe(400);
  const body = (await res.json()) as ErrorBody;
  expect(body.code).toBe('INVALID_ARGUMENT');
  expect(body.message).toBe(`${field} must be an array of strings`);
}

describe('Node update collection membership (HTTP → gRPC → SQLite)', () => {
  it('adds a node to a collection by path, creating the collection', async () => {
    const nodeId = await createTextNode('joins by path');
    const name = `by-path-${crypto.randomUUID()}`;

    const res = await patchNode(nodeId, { addToCollections: [name] });
    expect(res.status).toBe(200);

    const collectionId = await collectionIdByName(name);
    expect(collectionId).toBeDefined();
    expect(await memberIds(collectionId!)).toContain(nodeId);
  });

  it('adds a node to a collection by id, then removes it by id', async () => {
    const { collectionId, seedId } = await seedCollection(`by-id-${crypto.randomUUID()}`);
    const nodeId = await createTextNode('joins by id');

    const added = await patchNode(nodeId, { addToCollectionIds: [collectionId] });
    expect(added.status).toBe(200);
    expect(await memberIds(collectionId)).toContain(nodeId);

    const removed = await patchNode(nodeId, { removeFromCollectionIds: [collectionId] });
    expect(removed.status).toBe(200);
    const remaining = await memberIds(collectionId);
    expect(remaining).not.toContain(nodeId);
    expect(remaining).toContain(seedId);
  });

  it('applies a content change and a collection change from one request', async () => {
    const { collectionId } = await seedCollection(`with-content-${crypto.randomUUID()}`);
    const nodeId = await createTextNode('before');

    const res = await patchNode(nodeId, {
      version: 1,
      content: 'after',
      addToCollectionIds: [collectionId]
    });
    expect(res.status).toBe(200);

    const node = await h.adapter.getNode(nodeId);
    expect(node!.content).toBe('after');
    expect(node!.version).toBe(2);
    expect(await memberIds(collectionId)).toContain(nodeId);
  });

  it('rejects a path add and an id add on one request', async () => {
    const { collectionId } = await seedCollection(`both-${crypto.randomUUID()}`);
    const nodeId = await createTextNode('sends both');
    const otherName = `both-other-${crypto.randomUUID()}`;

    const res = await patchNode(nodeId, {
      addToCollections: [otherName],
      addToCollectionIds: [collectionId]
    });

    await expectPathsAndIdsRefused(res);
    expect(await memberIds(collectionId)).not.toContain(nodeId);
    expect(await collectionIdByName(otherName)).toBeUndefined();
  });

  it('rejects a collection field that is not an array of strings', async () => {
    const nodeId = await createTextNode('sends a bare string');

    const res = await patchNode(nodeId, { addToCollections: 'not-a-list' });

    await expectNotAStringList(res, 'addToCollections');
  });
});

describe('Node create collection membership (HTTP → gRPC → SQLite)', () => {
  it('creates a node in a collection by path, creating the collection', async () => {
    const nodeId = crypto.randomUUID();
    const name = `create-by-path-${crypto.randomUUID()}`;

    const res = await postNode(nodeId, { collections: [name] });
    expect(res.status).toBe(200);

    const collectionId = await collectionIdByName(name);
    expect(collectionId).toBeDefined();
    expect(await memberIds(collectionId!)).toContain(nodeId);
  });

  it('creates a node in a collection by id', async () => {
    const { collectionId } = await seedCollection(`create-by-id-${crypto.randomUUID()}`);
    const nodeId = crypto.randomUUID();

    const res = await postNode(nodeId, { collectionIds: [collectionId] });
    expect(res.status).toBe(200);

    expect(await memberIds(collectionId)).toContain(nodeId);
  });

  it('rejects a path and an id on one request, creating nothing', async () => {
    const { collectionId } = await seedCollection(`create-both-${crypto.randomUUID()}`);
    const nodeId = crypto.randomUUID();
    const otherName = `create-both-other-${crypto.randomUUID()}`;

    const res = await postNode(nodeId, {
      collections: [otherName],
      collectionIds: [collectionId]
    });

    await expectPathsAndIdsRefused(res);
    expect(await h.adapter.getNode(nodeId)).toBeNull();
    expect(await collectionIdByName(otherName)).toBeUndefined();
  });

  it('rejects a collection field that is not an array of strings', async () => {
    const nodeId = crypto.randomUUID();

    const res = await postNode(nodeId, { collections: 'not-a-list' });

    await expectNotAStringList(res, 'collections');
    expect(await h.adapter.getNode(nodeId)).toBeNull();
  });
});
