/**
 * Unit tests for the collection-authoring service — the shared "New node" and
 * "Add existing" logic behind the collection viewer and the sidebar sub-panel.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { Node } from '$lib/types';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: { createNode: vi.fn() }
}));

vi.mock('$lib/services/collection-service', () => ({
  collectionService: { addNodeToCollection: vi.fn() }
}));

import { invoke } from '@tauri-apps/api/core';
import { backendAdapter } from '$lib/services/backend-adapter';
import { collectionService } from '$lib/services/collection-service';
import { createNodeInCollection, searchAddableNodes } from '$lib/services/collection-authoring';

const invokeMock = vi.mocked(invoke);
const createNodeMock = vi.mocked(backendAdapter.createNode);
const addNodeToCollectionMock = vi.mocked(collectionService.addNodeToCollection);

// Minimal Node builder (only the required fields; nodeType/id are what the
// filter cares about).
function makeNode(id: string, nodeType: string): Node {
  return {
    id,
    nodeType,
    content: id,
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {}
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('createNodeInCollection', () => {
  it('mints an empty text root node then attaches it, returning the new id', async () => {
    const returnedId = await createNodeInCollection('col-42');

    // createNode was called once with a text root (empty content, no parent).
    expect(createNodeMock).toHaveBeenCalledTimes(1);
    const createArg = createNodeMock.mock.calls[0][0];
    expect(createArg).toEqual(
      expect.objectContaining({ nodeType: 'text', content: '', parentId: null })
    );

    // The generated id is threaded through the membership edge and returned.
    expect(typeof createArg.id).toBe('string');
    expect(createArg.id.length).toBeGreaterThan(0);
    expect(returnedId).toBe(createArg.id);
    expect(addNodeToCollectionMock).toHaveBeenCalledTimes(1);
    expect(addNodeToCollectionMock).toHaveBeenCalledWith(returnedId, 'col-42');
  });

  it('creates the node before writing the membership edge', async () => {
    await createNodeInCollection('col-order');

    // The node must exist before it can be made a member.
    expect(createNodeMock.mock.invocationCallOrder[0]).toBeLessThan(
      addNodeToCollectionMock.mock.invocationCallOrder[0]
    );
  });

  it('mints a distinct id per call', async () => {
    const first = await createNodeInCollection('col-1');
    const second = await createNodeInCollection('col-1');
    expect(first).not.toBe(second);
  });
});

describe('searchAddableNodes', () => {
  it('returns [] without calling the backend for an empty or whitespace query', async () => {
    expect(await searchAddableNodes('', 'col-1', new Set())).toEqual([]);
    expect(await searchAddableNodes('   ', 'col-1', new Set())).toEqual([]);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('trims the query and passes it to search_roots with a limit of 20', async () => {
    invokeMock.mockResolvedValue([]);

    await searchAddableNodes('  hello  ', 'col-1', new Set());

    expect(invokeMock).toHaveBeenCalledWith('search_roots', {
      params: { query: 'hello', limit: 20 }
    });
  });

  it('keeps a task matched by title, which the embeddings-only search could not return', async () => {
    // `search_roots` now merges keyword and semantic hits server-side, so
    // "add existing" can offer a non-embeddable node (ADR-029) whose title the
    // user typed. `task` is a content type, so nothing downstream filters it.
    invokeMock.mockResolvedValue([makeNode('task-1', 'task')]);

    const found = await searchAddableNodes('Draft Q3 architecture review', 'col-1', new Set());

    expect(found.map((n) => n.id)).toEqual(['task-1']);
  });

  it('preserves the backend ranking of the merged result list', async () => {
    // One already-ranked list arrives from the backend; the filter step may
    // only drop rows, never reorder the ones it keeps.
    invokeMock.mockResolvedValue([
      makeNode('keyword-hit', 'task'),
      makeNode('person-1', 'person'), // dropped: non-content
      makeNode('semantic-hit', 'text')
    ]);

    const found = await searchAddableNodes('Draft Q3 architecture review', 'col-1', new Set());

    expect(found.map((n) => n.id)).toEqual(['keyword-hit', 'semantic-hit']);
  });

  it('filters out the collection itself, excluded ids, and non-content node types', async () => {
    const results: Node[] = [
      makeNode('text-1', 'text'),
      makeNode('task-1', 'task'),
      makeNode('code-1', 'code-block'),
      makeNode('col-1', 'text'), // the collection id itself (even if content type)
      makeNode('already-in', 'text'), // already a member (excluded)
      makeNode('person-1', 'person'), // non-content
      makeNode('schema-1', 'schema'), // non-content
      makeNode('settings-1', 'database-settings'), // non-content
      makeNode('other-collection', 'collection'), // non-content
      makeNode('divider-1', 'horizontal-line') // non-content
    ];
    invokeMock.mockResolvedValue(results);

    const out = await searchAddableNodes('acp', 'col-1', new Set(['already-in']));

    // Only genuine, addable content survives, in original order.
    expect(out.map((n) => n.id)).toEqual(['text-1', 'task-1', 'code-1']);
  });

  it('keeps all content nodes when nothing is excluded', async () => {
    const results: Node[] = [
      makeNode('a', 'text'),
      makeNode('b', 'task'),
      makeNode('c', 'header'),
      makeNode('d', 'date')
    ];
    invokeMock.mockResolvedValue(results);

    const out = await searchAddableNodes('q', 'col-x', new Set());

    expect(out.map((n) => n.id)).toEqual(['a', 'b', 'c', 'd']);
  });
});
