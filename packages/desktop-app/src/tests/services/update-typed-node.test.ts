/**
 * `SharedNodeStore.updateTypedNode()` — the one write path for a core type's
 * typed fields, and `updateNode()`'s routing into it.
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
import type {
  Node,
  PersonNode,
  PlayNode,
  ProjectNode,
  QueryNode,
  TaskNode
} from '../../lib/types';
import { hasTypedUpdate, TYPED_CORE_FIELDS } from '../../lib/types/typed-core-fields';

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

  it('routes a typed query field (e.g. a view change) to updateQueryNode', async () => {
    store.setNode(makeNode('q1', 'query', { targetType: 'task', filters: [] }), dbSource);
    const typedSpy = vi.spyOn(backendAdapter, 'updateQueryNode').mockImplementation(
      async (id, version, update) =>
        ({
          ...makeNode(id, 'query', { targetType: 'task', filters: [], ...update }),
          version: version + 1
        }) as unknown as QueryNode
    );
    const genericSpy = vi.spyOn(backendAdapter, 'updateNode');
    const viewConfig = { lastView: 'kanban', kanban: { groupBy: 'status' } };

    store.updateNode('q1', { viewConfig } as unknown as Partial<Node>, viewerSource);

    await vi.waitFor(() => expect(typedSpy).toHaveBeenCalledWith('q1', 1, { viewConfig }));
    expect(genericSpy).not.toHaveBeenCalled();
    expect((store.getNode('q1') as unknown as QueryNode).viewConfig).toEqual(viewConfig);
  });

  it('routes a typed play field (e.g. a description edit) to updatePlayNode', async () => {
    store.setNode(makeNode('pl1', 'play', { rules: [] }), dbSource);
    const typedSpy = vi.spyOn(backendAdapter, 'updatePlayNode').mockImplementation(
      async (id, version, update) =>
        ({
          ...makeNode(id, 'play', { rules: [], ...update }),
          version: version + 1
        }) as unknown as PlayNode
    );
    const genericSpy = vi.spyOn(backendAdapter, 'updateNode');

    store.updateNode(
      'pl1',
      { description: 'Greets new tasks' } as unknown as Partial<Node>,
      viewerSource
    );

    await vi.waitFor(() =>
      expect(typedSpy).toHaveBeenCalledWith('pl1', 1, { description: 'Greets new tasks' })
    );
    expect(genericSpy).not.toHaveBeenCalled();
    expect((store.getNode('pl1') as unknown as PlayNode).description).toBe('Greets new tasks');
  });

  // The store routes every type the registry gives a typed update to that
  // update, so a type the backend adds one for without a route here has no
  // write path.
  it.each(Object.keys(TYPED_CORE_FIELDS).filter(hasTypedUpdate))(
    'sends a typed %s field through a typed update, never the generic one',
    async (nodeType) => {
      const field = TYPED_CORE_FIELDS[nodeType].find((f) => !f.readOnly)!;
      store.setNode(makeNode('n1', nodeType), dbSource);
      const respond = async (id: string, version: number) =>
        ({ ...makeNode(id, nodeType), version: version + 1 }) as never;
      const typedSpies = {
        task: vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(respond),
        person: vi.spyOn(backendAdapter, 'updatePersonNode').mockImplementation(respond),
        project: vi.spyOn(backendAdapter, 'updateProjectNode').mockImplementation(respond),
        query: vi.spyOn(backendAdapter, 'updateQueryNode').mockImplementation(respond),
        play: vi.spyOn(backendAdapter, 'updatePlayNode').mockImplementation(respond),
        collection: vi.spyOn(backendAdapter, 'updateCollectionNode').mockImplementation(respond),
        skill: vi.spyOn(backendAdapter, 'updateSkillNode').mockImplementation(respond),
        'database-settings': vi
          .spyOn(backendAdapter, 'updateDatabaseSettingsNode')
          .mockImplementation(respond)
      };
      const genericSpy = vi.spyOn(backendAdapter, 'updateNode');

      store.updateNode('n1', { [field.wire]: 'x' } as unknown as Partial<Node>, viewerSource);

      // The update for this type is the one called, and no other.
      const called = () =>
        Object.entries(typedSpies)
          .filter(([, spy]) => spy.mock.calls.length > 0)
          .map(([type]) => type);
      await vi.waitFor(() => expect(called()).toEqual([nodeType]));
      expect(genericSpy).not.toHaveBeenCalled();
    }
  );

  it('never sends a read-only system field of a query to updateQueryNode', async () => {
    store.setNode(makeNode('q1', 'query', { targetType: 'task', executionCount: 0 }), dbSource);
    const typedSpy = vi.spyOn(backendAdapter, 'updateQueryNode');
    const genericSpy = vi.spyOn(backendAdapter, 'updateNode');

    store.updateNode('q1', { executionCount: 7 } as unknown as Partial<Node>, viewerSource);

    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(typedSpy).not.toHaveBeenCalled();
    expect(genericSpy).not.toHaveBeenCalled();
    expect((store.getNode('q1') as unknown as QueryNode).executionCount).toBe(0);
  });

  it('keeps a subtype of a typed core type on the generic update, status included', async () => {
    // A user-defined `issue extends task` is the generic node: its fields live in
    // `properties`, and the typed task update is not its write path.
    store.setNode(makeNode('i1', 'issue', { properties: { status: 'open' } }), dbSource);
    const genericSpy = vi.spyOn(backendAdapter, 'updateNode').mockImplementation(
      async (id, version) =>
        ({
          ...makeNode(id, 'issue', { properties: { status: 'done' } }),
          version: version + 1
        }) as Node
    );
    const typedSpy = vi.spyOn(backendAdapter, 'updateTaskNode');

    store.updateNode('i1', { properties: { status: 'done' } }, viewerSource);

    await vi.waitFor(() => expect(genericSpy).toHaveBeenCalledTimes(1));
    expect(genericSpy.mock.calls[0][2]).toEqual({ properties: { status: 'done' } });
    expect(typedSpy).not.toHaveBeenCalled();
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
      async (id, version, update) =>
        ({ ...makeNode(id, 'project', { ...update }), version: version + 1 }) as unknown as ProjectNode
    );
    const genericSpy = vi.spyOn(backendAdapter, 'updateNode').mockImplementation(
      async (id, version) =>
        ({ ...makeNode(id, 'project', { status: 'active' }), content: 'Renamed', version: version + 1 }) as Node
    );
    const onPersistSuccess = vi.fn();

    store.updateNode(
      'pr1',
      { status: 'active', content: 'Renamed' } as unknown as Partial<Node>,
      viewerSource,
      { onPersistSuccess }
    );

    await vi.waitFor(() => expect(typedSpy).toHaveBeenCalledWith('pr1', 1, { status: 'active' }));
    await vi.waitFor(() => expect(genericSpy).toHaveBeenCalledTimes(1), { timeout: 3000 });
    // The generic half carries content only — the typed field has one write path.
    expect(genericSpy.mock.calls[0][2]).toEqual({ content: 'Renamed' });
    expect(store.getNode('pr1')?.content).toBe('Renamed');
    // Callbacks belong to the typed half: fired once, not once per half.
    await vi.waitFor(() => expect(store.getNode('pr1')?.version).toBe(3), { timeout: 3000 });
    expect(onPersistSuccess).toHaveBeenCalledTimes(1);
  });

  it('sends a typed write superseded in the queue by a generic write, ahead of it', async () => {
    // A write is in flight; a typed write queues; a generic write replaces it
    // in the coordinator's single queued slot. The typed field must still
    // reach the server — sent first by the generic write's closure.
    store.setNode(makeNode('p1', 'person', { firstName: 'Ada' }), dbSource);
    const calls: string[] = [];
    let releaseFirst!: () => void;
    vi.spyOn(backendAdapter, 'updatePersonNode').mockImplementation(async (id, version, update) => {
      calls.push(`typed:${JSON.stringify(update)}@v${version}`);
      if (calls.length === 1) {
        await new Promise<void>((resolve) => {
          releaseFirst = resolve;
        });
      }
      return { ...makeNode(id, 'person', { ...update }), version: version + 1 } as unknown as PersonNode;
    });
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (id, version, update) => {
      calls.push(`generic:${JSON.stringify(update)}@v${version}`);
      return { ...makeNode(id, 'person'), properties: {}, version: version + 1 } as Node;
    });

    store.updatePersonNode('p1', { firstName: 'Grace' }, viewerSource);
    await vi.waitFor(() => expect(calls).toHaveLength(1));
    store.updatePersonNode('p1', { email: 'grace@example.com' }, viewerSource);
    store.updateNode('p1', { properties: { 'custom:team': 'Core' } }, viewerSource, {
      persist: 'immediate'
    });
    releaseFirst();

    await vi.waitFor(() => expect(calls).toHaveLength(3), { timeout: 3000 });
    expect(calls).toEqual([
      'typed:{"firstName":"Grace"}@v1',
      'typed:{"email":"grace@example.com"}@v2',
      'generic:{"properties":{"custom:team":"Core"}}@v3'
    ]);
  });

  it('stages typed fields on a node awaiting its create, and sends them once it lands', async () => {
    // Not persisted yet: there is no server-side row for a typed update, and
    // an immediate typed write would cancel the node's debounced create. The
    // field is staged, then sent right after the create.
    // A viewer-sourced setNode with skipPersistence leaves it un-persisted.
    store.setNode(makeNode('pr1', 'project', { status: 'planning' }), viewerSource, true);
    const order: string[] = [];
    vi.spyOn(backendAdapter, 'createNode').mockImplementation(async () => {
      order.push('create');
      return { id: 'pr1', placement: null };
    });
    vi.spyOn(backendAdapter, 'getNode').mockImplementation(
      async (id) => makeNode(id, 'project', { status: 'planning' })
    );
    const typedSpy = vi.spyOn(backendAdapter, 'updateProjectNode').mockImplementation(
      async (id, version, update) => {
        order.push(`typed:${JSON.stringify(update)}`);
        return { ...makeNode(id, 'project', { ...update }), version: version + 1 } as unknown as ProjectNode;
      }
    );
    const onPersistSuccess = vi.fn();

    store.updateNode('pr1', { status: 'active' } as unknown as Partial<Node>, viewerSource, {
      onPersistSuccess
    });
    expect(typedSpy).not.toHaveBeenCalled();
    expect((store.getNode('pr1') as unknown as ProjectNode).status).toBe('active');

    // The node's create goes out through the generic path (here, a content edit).
    store.updateNode('pr1', { content: 'Launch' }, viewerSource, { persist: 'immediate' });

    await vi.waitFor(() => expect(order).toEqual(['create', 'typed:{"status":"active"}']), {
      timeout: 3000
    });
    await vi.waitFor(() => expect(onPersistSuccess).toHaveBeenCalledTimes(1));
  });

  it('a create carries every typed field the node holds, staged or not', async () => {
    // `priority` is on the node from the start and no typed write sends it, so
    // the create is its only way to the backend. `status` is staged by a typed
    // write, which still follows the create and settles its callers; the
    // create carries the staged value too, since that write only runs if the
    // create succeeds.
    store.setNode(
      makeNode('pr2', 'project', { status: 'planning', priority: 'high' }),
      viewerSource,
      true
    );
    const createSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockImplementation(async () => ({ id: 'pr2', placement: null }));
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(null);
    const typedSpy = vi.spyOn(backendAdapter, 'updateProjectNode').mockImplementation(
      async (id, version, update) =>
        ({ ...makeNode(id, 'project', { ...update }), version: version + 1 }) as unknown as ProjectNode
    );

    store.updateNode('pr2', { status: 'active' } as unknown as Partial<Node>, viewerSource);
    store.updateNode('pr2', { content: 'Launch' }, viewerSource, { persist: 'immediate' });

    await vi.waitFor(() => expect(typedSpy).toHaveBeenCalledTimes(1), { timeout: 3000 });
    expect(createSpy).toHaveBeenCalledTimes(1);
    expect(createSpy.mock.calls[0][0]).toEqual(
      expect.objectContaining({ id: 'pr2', properties: { status: 'active', priority: 'high' } })
    );
    expect(typedSpy.mock.calls[0][2]).toEqual({ status: 'active' });
  });

  it('an immediate typed write does not discard a still-debounced content edit', async () => {
    // Typing in a task, then changing its status before the debounce fires:
    // the pending content write must still go out. It is started at once, and
    // — like every write — first flushes the typed field already staged.
    store.setNode(makeNode('t1', 'task', { status: 'open' }), dbSource);
    const calls: string[] = [];
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (id, version, update) => {
      calls.push(`generic:${JSON.stringify(update)}@v${version}`);
      return {
        ...makeNode(id, 'task', { status: 'open' }),
        content: 'Buy milk',
        version: version + 1
      } as Node;
    });
    vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(async (id, version, update) => {
      calls.push(`typed:${JSON.stringify(update)}@v${version}`);
      return { ...makeNode(id, 'task', { ...update }), version: version + 1 } as unknown as TaskNode;
    });

    store.updateNode('t1', { content: 'Buy milk' }, viewerSource); // debounced
    store.updateTaskNode('t1', { status: 'done' }, viewerSource); // immediate

    await vi.waitFor(() => expect(calls).toHaveLength(2), { timeout: 3000 });
    expect(calls).toEqual(['typed:{"status":"done"}@v1', 'generic:{"content":"Buy milk"}@v2']);
  });

  it('a failed typed flush neither blocks the generic write nor reports to its caller', async () => {
    // A queued typed write superseded by a generic write is flushed by the
    // generic closure. If that typed update fails, the generic write still
    // goes out, and the failure is reported to the TYPED caller only.
    store.setNode(makeNode('pr1', 'project', { status: 'planning' }), dbSource);
    let releaseFirst!: () => void;
    let typedCalls = 0;
    vi.spyOn(backendAdapter, 'updateProjectNode').mockImplementation(async (id, version, update) => {
      typedCalls++;
      if (typedCalls === 1) {
        await new Promise<void>((resolve) => {
          releaseFirst = resolve;
        });
        return { ...makeNode(id, 'project', { ...update }), version: version + 1 } as unknown as ProjectNode;
      }
      throw new Error('daemon offline');
    });
    const genericSpy = vi.spyOn(backendAdapter, 'updateNode').mockImplementation(
      async (id, version) => ({ ...makeNode(id, 'project'), version: version + 1 }) as Node
    );
    const typedError = vi.fn();
    const genericError = vi.fn();

    store.updateNode('pr1', { status: 'active' } as unknown as Partial<Node>, viewerSource);
    await vi.waitFor(() => expect(typedCalls).toBe(1));
    store.updateNode('pr1', { priority: 'high' } as unknown as Partial<Node>, viewerSource, {
      onPersistError: typedError
    });
    store.updateNode(
      'pr1',
      { properties: { 'custom:budget': 5 } },
      viewerSource,
      { persist: 'immediate', onPersistError: genericError }
    );
    releaseFirst();

    await vi.waitFor(() => expect(genericSpy).toHaveBeenCalledTimes(1), { timeout: 3000 });
    expect(genericSpy.mock.calls[0][2]).toEqual({ properties: { 'custom:budget': 5 } });
    await vi.waitFor(() => expect(typedError).toHaveBeenCalledTimes(1));
    expect(genericError).not.toHaveBeenCalled();
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
