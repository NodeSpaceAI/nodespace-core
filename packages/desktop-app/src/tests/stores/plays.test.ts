/**
 * Plays store — the list behind the "Plays" navigation section.
 *
 * `loadPlays` fetches every participating play into the node store and the
 * store derives each row's title and state from the node held there, so a
 * switch flipped in the play's viewer shows without a reload.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

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

import { playsData } from '$lib/stores/plays.svelte';
import {
  sharedNodeStore,
  SimplePersistenceCoordinator
} from '$lib/services/shared-node-store.svelte';

function makePlay(id: string, content: string, fields: Record<string, unknown> = {}): Node {
  return {
    id,
    nodeType: 'play',
    content,
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {},
    isSeeded: false,
    rules: [],
    enabled: true,
    ...fields
  } as unknown as Node;
}

describe('playsData', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sharedNodeStore.clearAll();
    playsData.reset();
  });

  afterEach(() => {
    sharedNodeStore.clearAll();
  });

  it('lists every play the query returns, ordered by title', async () => {
    mockQueryNodes.mockResolvedValue([
      makePlay('p-user', 'weekly review'),
      makePlay('p-core', 'Task status', { isSeeded: true }),
      makePlay('p-b', 'Archive done tasks')
    ]);

    await playsData.loadPlays();

    // The default query applies the participation rule: no archived plays
    // come back, and the store asks for nothing beyond it.
    expect(mockQueryNodes).toHaveBeenCalledWith({ nodeType: 'play' });
    expect(playsData.plays.map((p) => p.title)).toEqual([
      'Archive done tasks',
      'Task status',
      'weekly review'
    ]);
  });

  it('derives the three row states from enabled and the suspension', async () => {
    mockQueryNodes.mockResolvedValue([
      makePlay('p-on', 'A on'),
      makePlay('p-off', 'B off', { enabled: false }),
      makePlay('p-suspended', 'C suspended', {
        suspendedAt: '2026-03-01T10:00:00.000Z',
        suspendedReason: 'error',
        suspendedMessage: 'Action 2 failed: no such field'
      })
    ]);

    await playsData.loadPlays();

    expect(playsData.plays.map((p) => [p.id, p.state, p.suspendedMessage])).toEqual([
      ['p-on', 'on', undefined],
      ['p-off', 'off', undefined],
      ['p-suspended', 'suspended', 'Action 2 failed: no such field']
    ]);
  });

  it('shows a play switched off as off, not as waiting on its suspension', async () => {
    mockQueryNodes.mockResolvedValue([
      makePlay('p', 'Play', {
        enabled: false,
        suspendedAt: '2026-03-01T10:00:00.000Z',
        suspendedMessage: 'stale'
      })
    ]);

    await playsData.loadPlays();

    expect(playsData.plays[0].state).toBe('off');
    expect(playsData.plays[0].suspendedMessage).toBeUndefined();
  });

  it('names a play with no title "Untitled play"', async () => {
    mockQueryNodes.mockResolvedValue([makePlay('p', '')]);

    await playsData.loadPlays();

    expect(playsData.plays[0].title).toBe('Untitled play');
  });

  it('reads each listed play from the node store, so a change there shows without a reload', async () => {
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play')]);
    await playsData.loadPlays();
    expect(playsData.plays[0].state).toBe('on');

    // The viewer's switch writes to the node store; no reload follows.
    sharedNodeStore.setNode(makePlay('p', 'Play renamed', { enabled: false }), {
      type: 'database',
      reason: 'test'
    });

    expect(playsData.plays).toEqual([
      { id: 'p', nodeType: 'play', title: 'Play renamed', state: 'off', suspendedMessage: undefined }
    ]);
    expect(mockQueryNodes).toHaveBeenCalledTimes(1);
  });

  it('a load does not put back what the node store already holds at that version', async () => {
    // The viewer switched the play off; the write has not come back yet, so
    // the stored node still carries the version the backend returns.
    sharedNodeStore.setNode(makePlay('p', 'Play', { enabled: false }), {
      type: 'database',
      reason: 'test'
    });
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play', { enabled: true })]);

    await playsData.loadPlays();

    expect(playsData.plays[0].state).toBe('off');
  });

  it('a load replaces a stored play with a newer version', async () => {
    sharedNodeStore.setNode(makePlay('p', 'Play'), { type: 'database', reason: 'test' });
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play', { enabled: false, version: 2 })]);

    await playsData.loadPlays();

    expect(playsData.plays[0].state).toBe('off');
  });

  it('a load leaves alone a play whose local write is still in flight', async () => {
    // The viewer switched the play off and the backend has committed it, but
    // the write's response has not landed: the load carries the newer version.
    sharedNodeStore.setNode(makePlay('p', 'Play', { enabled: false }), {
      type: 'database',
      reason: 'test'
    });
    const hasPending = vi
      .spyOn(SimplePersistenceCoordinator.getInstance(), 'hasPending')
      .mockImplementation((id) => id === 'p');
    const setNode = vi.spyOn(sharedNodeStore, 'setNode');
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play', { enabled: false, version: 2 })]);

    await playsData.loadPlays();

    // No write to the node store, so no conflict notice for the user's own switch.
    expect(setNode).not.toHaveBeenCalled();
    expect(playsData.plays.map((p) => [p.id, p.state])).toEqual([['p', 'off']]);

    hasPending.mockRestore();
    setNode.mockRestore();
  });

  it('the latest of two overlapping loads wins, whichever resolves last', async () => {
    let resolveFirst: (nodes: Node[]) => void = () => {};
    mockQueryNodes.mockReturnValueOnce(new Promise<Node[]>((resolve) => (resolveFirst = resolve)));
    mockQueryNodes.mockResolvedValueOnce([makePlay('new', 'Newer')]);

    const first = playsData.loadPlays();
    await playsData.loadPlays();
    resolveFirst([makePlay('old', 'Older')]);
    await first;

    expect(playsData.plays.map((p) => p.id)).toEqual(['new']);
    expect(playsData.has('old')).toBe(false);
  });

  it('pins the listed plays in the node store, and releases them when it stops listing them', async () => {
    const pinNodes = vi.spyOn(sharedNodeStore, 'pinNodes');
    const unpinAll = vi.spyOn(sharedNodeStore, 'unpinAll');
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play'), makePlay('q', 'Other')]);

    await playsData.loadPlays();
    expect(pinNodes).toHaveBeenLastCalledWith('plays-navigation-section', ['p', 'q']);

    playsData.invalidateForDatabaseSwitch();
    expect(unpinAll).toHaveBeenCalledWith('plays-navigation-section');
    expect(playsData.plays).toEqual([]);

    pinNodes.mockRestore();
    unpinAll.mockRestore();
  });

  it('drops a play deleted from the node store at once', async () => {
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play'), makePlay('q', 'Other')]);
    await playsData.loadPlays();

    sharedNodeStore.deleteNode('p', { type: 'database', reason: 'test' }, true);

    expect(playsData.plays.map((p) => p.id)).toEqual(['q']);
  });

  it('knows which plays are listed', async () => {
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play')]);
    await playsData.loadPlays();

    expect(playsData.has('p')).toBe(true);
    expect(playsData.has('other')).toBe(false);
  });

  it('drops a play the next load no longer returns', async () => {
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play'), makePlay('q', 'Other')]);
    await playsData.loadPlays();

    mockQueryNodes.mockResolvedValue([makePlay('q', 'Other')]);
    await playsData.loadPlays();

    expect(playsData.plays.map((p) => p.id)).toEqual(['q']);
  });

  it('keeps the current list when a load fails', async () => {
    mockQueryNodes.mockResolvedValue([makePlay('p', 'Play')]);
    await playsData.loadPlays();

    mockQueryNodes.mockRejectedValue(new Error('daemon unreachable'));
    await playsData.loadPlays();

    expect(playsData.plays.map((p) => p.id)).toEqual(['p']);
  });

  it('discards a load that resolves after a database switch', async () => {
    let resolveLoad: (nodes: Node[]) => void = () => {};
    mockQueryNodes.mockReturnValue(new Promise<Node[]>((resolve) => (resolveLoad = resolve)));

    const load = playsData.loadPlays();
    playsData.invalidateForDatabaseSwitch();
    resolveLoad([makePlay('from-previous-database', 'Stale')]);
    await load;

    expect(playsData.plays).toEqual([]);
  });
});
