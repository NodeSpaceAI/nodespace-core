/**
 * Unsaved placeholder instances in SharedNodeStore.
 *
 * A new instance of a type whose schema has required fields without a default
 * is rejected by the backend if created empty. It lives only in the store until
 * an edit fills every such field; that edit is then written through the store's
 * normal create path, exactly once.
 */
import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest';
import { SharedNodeStore, SimplePersistenceCoordinator } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import type { Node } from '$lib/types';

const TYPE = 'widget';
const VIEWER = { type: 'viewer' as const, viewerId: 'test' };

function placeholder(id: string): Node {
  return {
    lifecycleStatus: 'active',
    id,
    nodeType: TYPE,
    content: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {},
    mentions: []
  } as Node;
}

const hasDescription = (n: Node) => {
  const v = n.properties?.description;
  return typeof v === 'string' && v.trim() !== '';
};

describe('SharedNodeStore unsaved placeholders', () => {
  let store: SharedNodeStore;
  let createNodeSpy: MockInstance<typeof backendAdapter.createNode>;
  let updateNodeSpy: MockInstance<typeof backendAdapter.updateNode>;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    SimplePersistenceCoordinator.resetInstance();
    store = SharedNodeStore.getInstance();
    createNodeSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockImplementation(async (input) => ({ id: (input as Node).id, placement: null }));
    updateNodeSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockImplementation(async (id: string) => ({ ...placeholder(id), version: 3 }));
    vi.spyOn(backendAdapter, 'getNode').mockImplementation(async (id: string) => ({
      ...placeholder(id),
      version: 2
    }));
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    vi.restoreAllMocks();
  });

  const settle = () => new Promise((resolve) => setTimeout(resolve, 20));

  it('holds the node in the store without writing to the backend', async () => {
    store.createUnsavedPlaceholder(placeholder('p1'), hasDescription);
    await settle();

    expect(store.getNode('p1')).toBeDefined();
    expect(store.isUnsavedPlaceholder('p1')).toBe(true);
    expect(store.isNodePersisted('p1')).toBe(false);
    expect(createNodeSpy).not.toHaveBeenCalled();
  });

  it('keeps an edit that leaves required fields empty local', async () => {
    store.createUnsavedPlaceholder(placeholder('p1'), hasDescription);

    store.updateNode('p1', { content: 'My widget' }, VIEWER);
    store.updateNode('p1', { properties: { description: '   ' } }, VIEWER);
    await settle();

    expect(store.getNode('p1')?.content).toBe('My widget');
    expect(store.isUnsavedPlaceholder('p1')).toBe(true);
    expect(createNodeSpy).not.toHaveBeenCalled();
    expect(updateNodeSpy).not.toHaveBeenCalled();
  });

  it('creates the node once when the edit that fills the last required field lands', async () => {
    store.createUnsavedPlaceholder(placeholder('p1'), hasDescription);
    store.updateNode('p1', { content: 'My widget' }, VIEWER);

    store.updateNode('p1', { properties: { description: 'Does things' } }, VIEWER);
    await vi.waitFor(() => expect(createNodeSpy).toHaveBeenCalledTimes(1));

    expect(createNodeSpy.mock.calls[0][0]).toEqual(
      expect.objectContaining({
        id: 'p1',
        nodeType: TYPE,
        content: 'My widget',
        properties: { description: 'Does things' }
      })
    );
    expect(store.isUnsavedPlaceholder('p1')).toBe(false);
    await vi.waitFor(() => expect(store.isNodePersisted('p1')).toBe(true));
  });

  it('later edits update the persisted node and never create it a second time', async () => {
    store.createUnsavedPlaceholder(placeholder('p1'), hasDescription);
    store.updateNode('p1', { properties: { description: 'Does things' } }, VIEWER);
    await vi.waitFor(() => expect(store.isNodePersisted('p1')).toBe(true));

    store.updateNode('p1', { properties: { description: 'Does more things' } }, VIEWER);
    await vi.waitFor(() => expect(updateNodeSpy).toHaveBeenCalled());
    await settle();

    expect(createNodeSpy).toHaveBeenCalledTimes(1);
  });

  it('back-to-back edits around completion still issue a single create', async () => {
    store.createUnsavedPlaceholder(placeholder('p1'), hasDescription);

    store.updateNode('p1', { properties: { description: 'a' } }, VIEWER);
    store.updateNode('p1', { properties: { description: 'ab' } }, VIEWER);
    store.updateNode('p1', { content: 'Named' }, VIEWER);
    await settle();
    await vi.waitFor(() => expect(store.isNodePersisted('p1')).toBe(true));

    expect(createNodeSpy).toHaveBeenCalledTimes(1);
  });

  it('a computed-field write never releases the hold', async () => {
    store.createUnsavedPlaceholder(placeholder('p1'), () => true);

    store.updateNode('p1', { content: 'Preview' }, VIEWER, { isComputedField: true });
    await settle();

    expect(store.isUnsavedPlaceholder('p1')).toBe(true);
    expect(createNodeSpy).not.toHaveBeenCalled();
  });

  it('discards a placeholder silently once the tab that showed it closes', async () => {
    store.createUnsavedPlaceholder(placeholder('p1'), hasDescription);

    store.updateOpenDocumentRoots(['p1']);
    expect(store.getNode('p1')).toBeDefined();

    store.updateOpenDocumentRoots([]);
    await settle();

    expect(store.getNode('p1')).toBeUndefined();
    expect(store.isUnsavedPlaceholder('p1')).toBe(false);
    expect(createNodeSpy).not.toHaveBeenCalled();
  });

  it('keeps a placeholder no tab has displayed yet', () => {
    store.createUnsavedPlaceholder(placeholder('p1'), hasDescription);

    store.updateOpenDocumentRoots(['some-other-node']);

    expect(store.getNode('p1')).toBeDefined();
    expect(store.isUnsavedPlaceholder('p1')).toBe(true);
  });

  it('does not discard a persisted node when its tab closes', async () => {
    store.createUnsavedPlaceholder(placeholder('p1'), hasDescription);
    store.updateOpenDocumentRoots(['p1']);
    store.updateNode('p1', { properties: { description: 'Does things' } }, VIEWER);
    await vi.waitFor(() => expect(store.isNodePersisted('p1')).toBe(true));

    store.updateOpenDocumentRoots([]);

    expect(store.isUnsavedPlaceholder('p1')).toBe(false);
    expect(createNodeSpy).toHaveBeenCalledTimes(1);
  });
});
