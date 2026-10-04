/**
 * NodeRelationshipsState — the one relationships load both form surfaces
 * share. The staleness rule is request ORDER: the most recently started fetch
 * wins, whichever response lands last.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { flushSync } from 'svelte';

const loadNodeRelationshipsView = vi.fn();
vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: (...args: unknown[]) => loadNodeRelationshipsView(...args)
}));

import { NodeRelationshipsState } from '$lib/services/node-relationships-state.svelte';
import {
  buildRelationshipsView,
  type NodeRelationshipsView
} from '$lib/services/relationship-grouping';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/** A view whose single `one` group has `assignee` as its only row, if given. */
function view(nodeId: string, assignee?: string): NodeRelationshipsView {
  return buildRelationshipsView({
    nodeId,
    nodeType: 'task',
    groups: [
      {
        relationshipName: 'tasks',
        direction: 'in',
        targetType: 'person',
        reverseName: 'assignee',
        sourceType: 'person',
        cardinality: 'one',
        farCardinality: 'many',
        required: null,
        edgeFields: null,
        description: null,
        related: assignee
          ? [{ id: assignee, nodeType: 'person', title: assignee, contentPreview: '', edgeProperties: {} }]
          : [],
        count: assignee ? 1 : 0
      }
    ]
  });
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('NodeRelationshipsState', () => {
  beforeEach(() => loadNodeRelationshipsView.mockReset());

  it('keeps the newest reload when an older one resolves after it', async () => {
    const initial = deferred<NodeRelationshipsView>();
    const first = deferred<NodeRelationshipsView>();
    const second = deferred<NodeRelationshipsView>();
    loadNodeRelationshipsView
      .mockReturnValueOnce(initial.promise)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const state = new NodeRelationshipsState();
    state.load('task-1');
    initial.resolve(view('task-1'));
    await settle();

    // Two quick edits, two reloads; the second lands first.
    void state.reload();
    void state.reload();
    second.resolve(view('task-1', 'ana'));
    await settle();
    first.resolve(view('task-1', 'sam'));
    await settle();

    flushSync();
    expect(state.partitioned.promoted[0].rows.map((r) => r.id)).toEqual(['ana']);
  });

  it("drops a previous node's late response after switching nodes", async () => {
    const a = deferred<NodeRelationshipsView>();
    const b = deferred<NodeRelationshipsView>();
    loadNodeRelationshipsView.mockReturnValueOnce(a.promise).mockReturnValueOnce(b.promise);

    const state = new NodeRelationshipsState();
    state.load('task-a');
    state.load('task-b');
    b.resolve(view('task-b'));
    await settle();
    a.resolve(view('task-a', 'sam'));
    await settle();

    expect(state.view?.nodeId).toBe('task-b');
  });

  it('does not refetch for the node already loaded', () => {
    loadNodeRelationshipsView.mockResolvedValue(view('task-1'));
    const state = new NodeRelationshipsState();
    state.load('task-1');
    state.load('task-1');
    expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(1);
  });

  // Fail-open on a load error is covered through the forms that use this
  // class: see the "fails open" cases in the task, person and generic
  // schema form tests.
});

describe('NodeRelationshipsState — modal count and scheduled reloads', () => {
  beforeEach(() => loadNodeRelationshipsView.mockReset());

  /** A promoted `one` group with an edge, and a `many` group with `parts` edges. */
  function mixedView(parts: number): NodeRelationshipsView {
    const related = (prefix: string, n: number) =>
      Array.from({ length: n }, (_, i) => ({
        id: `${prefix}-${i}`,
        nodeType: 'widget',
        title: `${prefix} ${i}`,
        contentPreview: '',
        edgeProperties: {}
      }));
    const group = (name: string, cardinality: 'one' | 'many', n: number) => ({
      relationshipName: name,
      direction: 'out' as const,
      targetType: 'widget',
      reverseName: `${name}_of`,
      sourceType: 'gadget',
      cardinality,
      farCardinality: 'many' as const,
      required: null,
      edgeFields: null,
      description: null,
      related: related(name, n),
      count: n
    });
    return buildRelationshipsView({
      nodeId: 'n1',
      nodeType: 'gadget',
      groups: [group('owner', 'one', 1), group('parts', 'many', parts)]
    });
  }

  it('counts the related nodes the modal lists, leaving out promoted fields', async () => {
    loadNodeRelationshipsView.mockResolvedValue(mixedView(3));
    const state = new NodeRelationshipsState();
    state.load('n1');
    await settle();
    flushSync();

    expect(state.modalCount).toBe(3);
  });

  it('turns a burst of scheduled reloads into one fetch', async () => {
    vi.useFakeTimers();
    try {
      loadNodeRelationshipsView.mockResolvedValue(mixedView(1));
      const state = new NodeRelationshipsState();
      state.load('n1');
      expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(1);

      for (let i = 0; i < 20; i++) state.scheduleReload();
      expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(1);

      await vi.advanceTimersByTimeAsync(200);
      expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(2);

      // A cancelled schedule never fetches.
      state.scheduleReload();
      state.cancelScheduledReload();
      await vi.advanceTimersByTimeAsync(200);
      expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
