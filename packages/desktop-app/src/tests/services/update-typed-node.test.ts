/**
 * `SharedNodeStore.updateTypedNode()` — the one write path for a core type's
 * typed fields (task, person, project), and `updateNode()`'s routing into it.
 *
 * The same-field and different-field clobber guards are covered by the
 * `updatetasknode-*-clobber-regression` suites, which drive this method
 * through `updateTaskNode()`. This file covers what is new with the generic
 * path: person/project dispatch, the accumulation of superseded typed writes,
 * and the split between typed fields and extension `properties`.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  SharedNodeStore,
  SimplePersistenceCoordinator
} from '../../lib/services/shared-node-store.svelte';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import type { Node, PersonNode, ProjectNode } from '../../lib/types';

const dbSource = { type: 'database' as const, reason: 'initial-load' };
const viewerSource = { type: 'viewer' as const, viewerId: 'pane-1' };

function makeNode(id: string, nodeType: string, typed: Record<string, unknown> = {}): Node {
  return {
    id,
    nodeType,
    content: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {},
    mentions: [],
    ...typed
  } as unknown as Node;
}

describe('updateTypedNode', () => {
  let store: SharedNodeStore;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    SimplePersistenceCoordinator.resetInstance();
    store = SharedNodeStore.getInstance();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    conflictNotifications.dismissAll();
    vi.restoreAllMocks();
  });

  it('applies a person update optimistically and sends it through updatePersonNode', async () => {
    store.setNode(makeNode('p1', 'person', { firstName: 'Ada' }), dbSource);
    const spy = vi.spyOn(backendAdapter, 'updatePersonNode').mockImplementation(
      async (id, version, update) =>
        ({ ...makeNode(id, 'person', { firstName: 'Ada', ...update }), version: version + 1 }) as unknown as PersonNode
    );

    store.updatePersonNode('p1', { lastName: 'Lovelace' }, viewerSource);

    expect((store.getNode('p1') as unknown as PersonNode).lastName).toBe('Lovelace');
    await vi.waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy).toHaveBeenCalledWith('p1', 1, { lastName: 'Lovelace' });
    await vi.waitFor(() => expect(store.getNode('p1')?.version).toBe(2));
  });

  it('reads a cleared field as undefined locally, as the typed wire shape omits it', () => {
    store.setNode(makeNode('p1', 'person', { email: 'ada@example.com' }), dbSource);
    vi.spyOn(backendAdapter, 'updatePersonNode').mockImplementation(() => new Promise(() => {}));

    store.updatePersonNode('p1', { email: null }, viewerSource);

    expect((store.getNode('p1') as unknown as PersonNode).email).toBeUndefined();
  });

  it('carries a superseded write\'s fields in the write that replaced it', async () => {
    // Write A is in flight; B queues; C supersedes B in the coordinator's
    // single queued slot. B's field must still reach the backend — in C.
    store.setNode(makeNode('p1', 'person'), dbSource);
    let releaseFirst!: () => void;
    const sent: Array<Record<string, unknown>> = [];
    vi.spyOn(backendAdapter, 'updatePersonNode').mockImplementation(async (id, version, update) => {
      sent.push({ ...update });
      if (sent.length === 1) {
        await new Promise<void>((resolve) => {
          releaseFirst = resolve;
        });
      }
      return { ...makeNode(id, 'person'), version: version + 1 } as unknown as PersonNode;
    });

    store.updatePersonNode('p1', { firstName: 'Ada' }, viewerSource);
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    store.updatePersonNode('p1', { lastName: 'Lovelace' }, viewerSource);
    store.updatePersonNode('p1', { email: 'ada@example.com' }, viewerSource);
    releaseFirst();

    await vi.waitFor(() => expect(sent).toHaveLength(2));
    expect(sent[0]).toEqual({ firstName: 'Ada' });
    expect(sent[1]).toEqual({ lastName: 'Lovelace', email: 'ada@example.com' });
  });

  it('ignores a typed update aimed at a node of another type', () => {
    store.setNode(makeNode('t1', 'text'), dbSource);
    const spy = vi.spyOn(backendAdapter, 'updatePersonNode');

    store.updatePersonNode('t1', { firstName: 'Ada' }, viewerSource);

    expect(spy).not.toHaveBeenCalled();
    expect((store.getNode('t1') as unknown as Record<string, unknown>).firstName).toBeUndefined();
  });
});

describe('updateNode routing for typed core types', () => {
  let store: SharedNodeStore;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    SimplePersistenceCoordinator.resetInstance();
    store = SharedNodeStore.getInstance();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    store.clearAll();
    SharedNodeStore.resetInstance();
    conflictNotifications.dismissAll();
    vi.restoreAllMocks();
  });

  it('routes a typed project field (e.g. a Kanban move) to updateProjectNode', async () => {
    store.setNode(makeNode('pr1', 'project', { status: 'planning' }), dbSource);
    const typedSpy = vi.spyOn(backendAdapter, 'updateProjectNode').mockImplementation(
      async (id, version, update) =>
        ({ ...makeNode(id, 'project', { status: 'planning', ...update }), version: version + 1 }) as unknown as ProjectNode
    );
    const genericSpy = vi.spyOn(backendAdapter, 'updateNode');

    store.updateNode('pr1', { status: 'active' } as unknown as Partial<Node>, viewerSource);

    await vi.waitFor(() => expect(typedSpy).toHaveBeenCalledWith('pr1', 1, { status: 'active' }));
    expect(genericSpy).not.toHaveBeenCalled();
    expect((store.getNode('pr1') as unknown as ProjectNode).status).toBe('active');
  });

  it('persists an extension-field write on a task through the generic update', async () => {
    // User-defined fields on a core type live in `properties` and must
    // persist — previously the task updater rejected a `properties` write.
    store.setNode(makeNode('t1', 'task', { status: 'open' }), dbSource);
    const genericSpy = vi.spyOn(backendAdapter, 'updateNode').mockImplementation(
      async (id, version) =>
        ({
          ...makeNode(id, 'task', { status: 'open' }),
          properties: { 'custom:store': 'Costco' },
          version: version + 1
        }) as Node
    );
    const typedSpy = vi.spyOn(backendAdapter, 'updateTaskNode');

    store.updateNode('t1', { properties: { 'custom:store': 'Costco' } }, viewerSource);

    await vi.waitFor(() => expect(genericSpy).toHaveBeenCalledTimes(1));
    expect(genericSpy.mock.calls[0][2]).toEqual({ properties: { 'custom:store': 'Costco' } });
    expect(typedSpy).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(store.getNode('t1')?.version).toBe(2));
  });

  it('splits a mixed write: typed fields to the typed update, content to the generic one', async () => {
    store.setNode(makeNode('pr1', 'project', { status: 'planning' }), dbSource);
    const typedSpy = vi.spyOn(backendAdapter, 'updateProjectNode').mockImplementation(
      () => new Promise(() => {})
    );

    store.updateNode(
      'pr1',
      { status: 'active', content: 'Renamed' } as unknown as Partial<Node>,
      viewerSource
    );

    await vi.waitFor(() => expect(typedSpy).toHaveBeenCalledWith('pr1', 1, { status: 'active' }));
    expect(store.getNode('pr1')?.content).toBe('Renamed');
  });

  it('calls onPersistError when the typed write fails, so the caller can revert its field', async () => {
    store.setNode(makeNode('pr1', 'project', { status: 'planning' }), dbSource);
    vi.spyOn(backendAdapter, 'updateProjectNode').mockRejectedValue(new Error('daemon offline'));
    const onPersistError = vi.fn();
    const onPersistSuccess = vi.fn();

    store.updateNode('pr1', { status: 'active' } as unknown as Partial<Node>, viewerSource, {
      onPersistError,
      onPersistSuccess
    });

    await vi.waitFor(() => expect(onPersistError).toHaveBeenCalledTimes(1));
    expect(onPersistSuccess).not.toHaveBeenCalled();
  });

  it('keeps a database-sourced typed field local, with no write', () => {
    store.setNode(makeNode('pr1', 'project', { status: 'planning' }), dbSource);
    const typedSpy = vi.spyOn(backendAdapter, 'updateProjectNode');

    store.updateNode('pr1', { status: 'active' } as unknown as Partial<Node>, dbSource);

    expect(typedSpy).not.toHaveBeenCalled();
    expect((store.getNode('pr1') as unknown as ProjectNode).status).toBe('active');
  });
});
