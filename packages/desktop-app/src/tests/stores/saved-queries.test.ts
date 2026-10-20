/**
 * Saved Queries store — grouping of saved query nodes by target type for the
 * sidebar's Types and Views list.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import type { Node } from '$lib/types';

const mockQueryNodes = vi.fn<(query: unknown) => Promise<Node[]>>();
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    queryNodes: (query: unknown) => mockQueryNodes(query)
  }
}));

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { savedQueriesData, SEEDED_AGENT_QUERY_IDS } from '$lib/stores/saved-queries.svelte';

function makeQuery(id: string, name: string, targetType?: string): Node {
  return {
    id,
    nodeType: 'query',
    content: name,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    ...(targetType === undefined ? {} : { targetType })
  } as unknown as Node;
}

describe('savedQueriesData', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    savedQueriesData.reset();
  });

  it('fetches query nodes only', async () => {
    mockQueryNodes.mockResolvedValue([]);

    await savedQueriesData.loadSavedQueries();

    expect(mockQueryNodes).toHaveBeenCalledWith({ nodeType: 'query' });
  });

  it('groups queries under the type they target', async () => {
    mockQueryNodes.mockResolvedValue([
      makeQuery('q1', 'Specs by Status', 'spec'),
      makeQuery('q2', 'Plans by Status', 'plan'),
      makeQuery('q3', 'Open specs', 'spec')
    ]);

    await savedQueriesData.loadSavedQueries();

    expect(savedQueriesData.forType('spec').map((q) => q.id)).toEqual(['q3', 'q1']);
    expect(savedQueriesData.forType('plan').map((q) => q.id)).toEqual(['q2']);
    expect(savedQueriesData.forType('task')).toEqual([]);
  });

  it('orders each type by name, case-insensitively, then id', async () => {
    mockQueryNodes.mockResolvedValue([
      makeQuery('b', 'beta', 'spec'),
      makeQuery('a2', 'Alpha', 'spec'),
      makeQuery('a1', 'alpha', 'spec'),
      makeQuery('c', 'Charlie', 'spec')
    ]);

    await savedQueriesData.loadSavedQueries();

    expect(savedQueriesData.forType('spec').map((q) => q.id)).toEqual(['a1', 'a2', 'b', 'c']);
  });

  it('does not list all-types queries (target "*") or queries with no target', async () => {
    mockQueryNodes.mockResolvedValue([
      makeQuery('all', 'Everything', '*'),
      makeQuery('none', 'Untargeted'),
      makeQuery('ok', 'Tasks', 'task')
    ]);

    await savedQueriesData.loadSavedQueries();

    expect(savedQueriesData.queries.map((q) => q.id)).toEqual(['ok']);
    expect(savedQueriesData.has('all')).toBe(false);
    expect(savedQueriesData.has('ok')).toBe(true);
  });

  it('picks up a renamed, retargeted or deleted query on reload', async () => {
    mockQueryNodes.mockResolvedValue([makeQuery('q1', 'Old', 'spec')]);
    await savedQueriesData.loadSavedQueries();
    expect(savedQueriesData.forType('spec').map((q) => q.name)).toEqual(['Old']);

    mockQueryNodes.mockResolvedValue([makeQuery('q1', 'New', 'plan')]);
    await savedQueriesData.loadSavedQueries();
    expect(savedQueriesData.forType('spec')).toEqual([]);
    expect(savedQueriesData.forType('plan').map((q) => q.name)).toEqual(['New']);

    mockQueryNodes.mockResolvedValue([]);
    await savedQueriesData.loadSavedQueries();
    expect(savedQueriesData.queries).toEqual([]);
  });

  it('keeps the previous list when a load fails', async () => {
    mockQueryNodes.mockResolvedValue([makeQuery('q1', 'A', 'spec')]);
    await savedQueriesData.loadSavedQueries();

    mockQueryNodes.mockRejectedValue(new Error('daemon down'));
    await savedQueriesData.loadSavedQueries();

    expect(savedQueriesData.queries.map((q) => q.id)).toEqual(['q1']);
  });

  it('drops a load that resolves after a database switch', async () => {
    let resolveLoad: (nodes: Node[]) => void = () => {};
    mockQueryNodes.mockReturnValue(new Promise((resolve) => (resolveLoad = resolve)));

    const pending = savedQueriesData.loadSavedQueries();
    savedQueriesData.invalidateForDatabaseSwitch();
    resolveLoad([makeQuery('stale', 'Stale', 'spec')]);
    await pending;

    expect(savedQueriesData.queries).toEqual([]);
  });

  it('hides the seeded agent-helper queries but lists a user-created query on the same type', async () => {
    const [readyTasks, needsShaping] = [...SEEDED_AGENT_QUERY_IDS].slice(3);
    mockQueryNodes.mockResolvedValue([
      makeQuery(readyTasks, 'Ready tasks', 'task'),
      makeQuery(needsShaping, 'Needs shaping', 'task'),
      makeQuery('user-q', 'My tasks', 'task')
    ]);

    await savedQueriesData.loadSavedQueries();

    expect(savedQueriesData.forType('task').map((q) => q.id)).toEqual(['user-q']);
    expect(savedQueriesData.has(readyTasks)).toBe(false);
  });

  it('keeps SEEDED_AGENT_QUERY_IDS equal to the Rust CORE_QUERY_IDS', () => {
    const rust = readFileSync(
      resolve(__dirname, '../../../../core/src/services/query_service/core_queries.rs'),
      'utf8'
    );
    const block = rust.match(/pub const CORE_QUERY_IDS: &\[&str\] = &\[([^\]]*)\]/)?.[1] ?? '';
    const constNames = block.match(/[A-Z_]+_QUERY_ID/g) ?? [];
    const ids = constNames.map((name) => {
      const m = rust.match(new RegExp(`pub const ${name}: &str = "([0-9a-f-]+)"`));
      return m?.[1];
    });
    expect(ids).toHaveLength(10);
    expect(new Set(ids)).toEqual(new Set(SEEDED_AGENT_QUERY_IDS));
  });
});
