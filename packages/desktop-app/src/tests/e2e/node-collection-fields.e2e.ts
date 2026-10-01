/**
 * E2E: collection membership set on a node create or update, or by the
 * add-by-path route, via dev-proxy → nodespaced → SQLite.
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
  parentCollectionIds: string[];
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

function addToCollectionPath(id: string, body: Record<string, unknown>): Promise<Response> {
  return send('POST', `/api/nodes/${encodeURIComponent(id)}/collections`, body);
}

async function collectionByName(name: string): Promise<CollectionSummary | undefined> {
  const res = await fetch(`${h.baseUrl}/api/collections`);
  const collections = (await res.json()) as CollectionSummary[];
  return collections.find((c) => c.content === name);
}

async function collectionIdByName(name: string): Promise<string | undefined> {
  return (await collectionByName(name))?.id;
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

describe('Add a node to a collection path (HTTP → gRPC → SQLite)', () => {
  it('creates the missing segments and returns the leaf collection id', async () => {
    const nodeId = await createTextNode('joins the leaf');
    const parentName = `path-parent-${crypto.randomUUID()}`;
    const leafName = `path-leaf-${crypto.randomUUID()}`;

    const res = await addToCollectionPath(nodeId, {
      collectionPath: `${parentName}:${leafName}`
    });
    expect(res.status).toBe(200);
    const returnedId = (await res.json()) as string;

    const parent = await collectionByName(parentName);
    const leaf = await collectionByName(leafName);
    expect(parent).toBeDefined();
    expect(leaf).toBeDefined();
    expect(returnedId).toBe(leaf!.id);
    expect(leaf!.parentCollectionIds).toContain(parent!.id);
    expect(await memberIds(returnedId)).toContain(nodeId);
    expect(await memberIds(parent!.id)).not.toContain(nodeId);
  });

  it('returns the existing leaf when the path already exists', async () => {
    const name = `path-existing-${crypto.randomUUID()}`;
    const { collectionId, seedId } = await seedCollection(name);
    const nodeId = await createTextNode('joins an existing collection');

    const res = await addToCollectionPath(nodeId, { collectionPath: name });
    expect(res.status).toBe(200);

    expect(await res.json()).toBe(collectionId);
    const members = await memberIds(collectionId);
    expect(members).toContain(nodeId);
    expect(members).toContain(seedId);
  });

  it('fails for a node that does not exist', async () => {
    const name = `path-missing-node-${crypto.randomUUID()}`;

    const res = await addToCollectionPath(crypto.randomUUID(), { collectionPath: name });

    expect(res.status).toBe(404);
    const body = (await res.json()) as ErrorBody;
    expect(body.code).toBe('NOT_FOUND');
  });

  it('rejects a path with an empty segment', async () => {
    const nodeId = await createTextNode('sends a trailing colon');
    const name = `path-invalid-${crypto.randomUUID()}`;

    for (const collectionPath of ['', `${name}:`]) {
      const res = await addToCollectionPath(nodeId, { collectionPath });

      expect(res.status).toBe(400);
      const body = (await res.json()) as ErrorBody;
      expect(body.code).toBe('INVALID_ARGUMENT');
    }
    expect(await collectionIdByName(name)).toBeUndefined();
  });

  it('rejects a collection path that is not a string', async () => {
    const nodeId = await createTextNode('sends a list');

    const res = await addToCollectionPath(nodeId, { collectionPath: ['not-a-string'] });

    expect(res.status).toBe(400);
    const body = (await res.json()) as ErrorBody;
    expect(body.code).toBe('INVALID_ARGUMENT');
    expect(body.message).toBe('collectionPath must be a string');
  });
});
