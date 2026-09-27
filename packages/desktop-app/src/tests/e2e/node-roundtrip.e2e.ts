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

  it('serves typed nodes inside a children tree', async () => {
    // Node pages populate the store from this tree, so its nodes must share the
    // single-node read's typed shape rather than storage's type bucket.
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
    const child = tree?.children?.find((c) => c.id === childId) as
      | (Record<string, unknown> & { properties?: Record<string, unknown> })
      | undefined;
    expect(child?.firstName).toBe('Ada');
    expect(child?.lastName).toBe('Lovelace');
    expect(child?.properties).toEqual({});
  });

  it('create and move replies carry the store placement for the written edge', async () => {
    const parent = crypto.randomUUID();
    const a = crypto.randomUUID();
    const b = crypto.randomUUID();
    const root = await h.adapter.createNode({ id: parent, nodeType: 'text', content: 'p' });
    expect(root.placement).toBeNull();

    const createdA = await h.adapter.createNode({ id: a, nodeType: 'text', content: 'a', parentId: parent });
    await h.adapter.createNode({ id: b, nodeType: 'text', content: 'b', parentId: parent });
    expect(createdA.placement).toMatchObject({ parentId: parent, respread: [] });

    // Move b before a: its new key must sort below a's.
    const moved = await h.adapter.moveNode(b, 1, parent, { type: 'beginning' });
    expect(moved.node.version).toBe(2);
    expect(moved.placement!.parentId).toBe(parent);
    expect(moved.placement!.order).toBeLessThan(createdA.placement!.order);

    const toRoot = await h.adapter.moveNode(b, 2, null, null);
    expect(toRoot.placement).toBeNull();
  });

  it('moveChildrenToParent returns each transferred edge’s store order', async () => {
    const from = crypto.randomUUID();
    const to = crypto.randomUUID();
    const c1 = crypto.randomUUID();
    const c2 = crypto.randomUUID();
    await h.adapter.createNode({ id: from, nodeType: 'text', content: 'from' });
    await h.adapter.createNode({ id: to, nodeType: 'text', content: 'to' });
    await h.adapter.createNode({ id: c1, nodeType: 'text', content: 'c1', parentId: from });
    await h.adapter.createNode({ id: c2, nodeType: 'text', content: 'c2', parentId: from });

    const { nodes, orders } = await h.adapter.moveChildrenToParent(to, [
      { id: c1, version: 1 },
      { id: c2, version: 1 }
    ]);

    expect(nodes.map((n) => [n.id, n.version])).toEqual([
      [c1, 2],
      [c2, 2]
    ]);
    expect(orders.map((o) => o.nodeId)).toEqual([c1, c2]);
    expect(orders[0].order).toBeLessThan(orders[1].order);
    expect((await h.adapter.getChildren(to)).map((n) => n.id)).toEqual([c1, c2]);
  });

  it('createNode returns the new node id string', async () => {
    const id = crypto.randomUUID();
    const result = await h.adapter.createNode({ id, nodeType: 'text', content: 'id check' });
    expect(typeof result.id).toBe('string');
    expect(result.id.length).toBeGreaterThan(0);
  });
});
