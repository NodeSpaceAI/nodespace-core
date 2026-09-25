/**
 * E2E: Node CRUD round-trip via HttpAdapter → dev-proxy → nodespaced → SQLite
 *
 * These tests exercise the full write→read round-trip, verifying that data
 * persists through the gRPC serialization layer and the SQLite store.
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

describe('Node CRUD round-trip (HTTP → gRPC → SQLite)', () => {
  it('creates a node and reads it back', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'text', content: 'hello e2e' });

    const node = await h.adapter.getNode(id);

    expect(node).not.toBeNull();
    expect(node!.id).toBe(id);
    expect(node!.nodeType).toBe('text');
    expect(node!.content).toBe('hello e2e');
    expect(node!.version).toBe(1);
  });

  it('updates a node and reads back the new content', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'text', content: 'original' });

    const updated = await h.adapter.updateNode(id, 1, { content: 'updated' });

    expect(updated.content).toBe('updated');
    expect(updated.version).toBe(2);

    const fetched = await h.adapter.getNode(id);
    expect(fetched!.content).toBe('updated');
    expect(fetched!.version).toBe(2);
  });

  it('version increments on each update', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'text', content: 'v1' });

    const v2 = await h.adapter.updateNode(id, 1, { content: 'v2' });
    expect(v2.version).toBe(2);

    const v3 = await h.adapter.updateNode(id, 2, { content: 'v3' });
    expect(v3.version).toBe(3);
  });

  it('deletes a node and confirms it is absent', async () => {
    const id = crypto.randomUUID();
    await h.adapter.createNode({ id, nodeType: 'text', content: 'to delete' });

    await h.adapter.deleteNode(id, 1);

    const node = await h.adapter.getNode(id);
    expect(node).toBeNull();
  });

  it('returns null for a non-existent node', async () => {
    const node = await h.adapter.getNode('00000000-0000-0000-0000-000000000000');
    expect(node).toBeNull();
  });

  it('creates a parent-child hierarchy and reads children', async () => {
    const parentId = crypto.randomUUID();
    const childId = crypto.randomUUID();

    await h.adapter.createNode({ id: parentId, nodeType: 'text', content: 'parent' });
    await h.adapter.createNode({ id: childId, nodeType: 'text', content: 'child', parentId });

    const children = await h.adapter.getChildren(parentId);
    expect(children).toHaveLength(1);
    expect(children[0].id).toBe(childId);
    expect(children[0].content).toBe('child');
  });

  it('persists node properties through the round-trip', async () => {
    const id = crypto.randomUUID();
    // Properties travel flat in both directions: the daemon stores them under the
    // node-type bucket ("text"), and the transport flattens that bucket on read.
    const properties = { priority: 'high', tags: ['a', 'b'], count: 42 };

    await h.adapter.createNode({ id, nodeType: 'text', content: 'with props', properties });

    const node = await h.adapter.getNode(id);
    expect(node).not.toBeNull();
    expect(node!.properties).toEqual(properties);
  });

  it('serves flat properties for nodes inside a children tree', async () => {
    // Node pages populate the store from this tree, so its nodes must share the
    // single-node read's flat shape rather than storage's type bucket.
    const parentId = crypto.randomUUID();
    const childId = crypto.randomUUID();
    await h.adapter.createNode({ id: parentId, nodeType: 'text', content: 'tree parent' });
    await h.adapter.createNode({
      id: childId,
      nodeType: 'person',
      content: '',
      parentId,
      properties: { first_name: 'Ada', last_name: 'Lovelace' }
    });

    const tree = await h.adapter.getChildrenTree(parentId);
    const child = tree?.children?.find((c) => c.id === childId);
    expect(child?.properties).toMatchObject({ first_name: 'Ada', last_name: 'Lovelace' });
    expect(child?.properties).not.toHaveProperty('person');
  });

  it('createNode returns the new node id string', async () => {
    const id = crypto.randomUUID();
    const result = await h.adapter.createNode({ id, nodeType: 'text', content: 'id check' });
    expect(typeof result).toBe('string');
    expect(result.length).toBeGreaterThan(0);
  });
});
