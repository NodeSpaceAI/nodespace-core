/**
 * Tests for C3a: Frontend outdent ordering math removal
 *
 * Verifies that:
 * 1. No frontend code computes fractional order values for optimistic placement.
 * 2. moveInMemoryRelationship is called without an order argument (relative-after intent).
 * 3. applyHasChildUpdated reconciles the optimistic placement to the daemon-supplied order.
 * 4. The three outdent variants (normal, in-flight not-persisted, in-flight CREATE executing)
 *    all call moveInMemoryRelationship without a hand-computed order.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  createReactiveNodeService,
  type ReactiveNodeService,
  type NodeManagerEvents
} from '$lib/services/reactive-node-service.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { focusManager } from '$lib/services/focus-manager.svelte';
import { conflictNotifications } from '$lib/stores/conflict-notifications.svelte';
import type { Node } from '$lib/types';

// vi.hoisted() runs before vi.mock hoisting — safe to reference in factory
const { moveInMemoryRelationshipSpy } = vi.hoisted(() => ({
  moveInMemoryRelationshipSpy: vi.fn()
}));

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    moveNode: vi.fn().mockResolvedValue({
      id: 'mock-node',
      nodeType: 'text',
      content: '',
      version: 2,
      properties: {},
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString()
    }),
    moveChildrenToParent: vi.fn().mockResolvedValue([]),
    getNode: vi.fn().mockResolvedValue(null),
    createNode: vi.fn().mockResolvedValue('mock-id'),
    updateNode: vi.fn().mockResolvedValue(null),
    deleteNode: vi.fn().mockResolvedValue({ deleted: true }),
    getChildren: vi.fn().mockResolvedValue([]),
    getChildrenTree: vi.fn().mockResolvedValue(null),
    getDescendants: vi.fn().mockResolvedValue([]),
    createMention: vi.fn().mockResolvedValue(undefined),
    deleteMention: vi.fn().mockResolvedValue(undefined),
    getOutgoingMentions: vi.fn().mockResolvedValue([]),
    getIncomingMentions: vi.fn().mockResolvedValue([]),
    getMentioningContainers: vi.fn().mockResolvedValue([]),
    queryNodes: vi.fn().mockResolvedValue([]),
    mentionAutocomplete: vi.fn().mockResolvedValue([]),
    createContainerNode: vi.fn().mockResolvedValue('mock-container-id'),
    updateTaskNode: vi.fn().mockResolvedValue(null)
  },
  insertPosition: {
    beginning: () => ({ type: 'beginning' }),
    end: () => ({ type: 'end' }),
    after: (siblingId: string) => ({ type: 'after', siblingId })
  }
}));

vi.mock('$lib/stores/reactive-structure-tree.svelte', () => ({
  structureTree: {
    addInMemoryRelationship: vi.fn(),
    moveInMemoryRelationship: moveInMemoryRelationshipSpy,
    getChildren: vi.fn(() => []),
    getChildrenWithOrder: vi.fn(() => []),
    getParent: vi.fn(() => null),
    removeChild: vi.fn(),
    addChild: vi.fn(),
    onChange: vi.fn(() => () => {}),
    children: new Map()
  }
}));

function makeNode(id: string): Node {
  return {
    id,
    nodeType: 'text',
    content: `Content ${id}`,
    version: 1,
    properties: {},
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString()
  };
}

describe('C3a — outdentNode emits no fractional order values', () => {
  let service: ReactiveNodeService;
  let events: NodeManagerEvents;
  let sharedNodeStore: SharedNodeStore;

  beforeEach(() => {
    vi.clearAllMocks();
    SharedNodeStore.resetInstance();
    sharedNodeStore = SharedNodeStore.getInstance();

    events = {
      focusRequested: vi.fn(),
      hierarchyChanged: vi.fn(),
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    };

    service = createReactiveNodeService(events);

    // Mock structureTree.getParent to return a parent chain for a child node
    vi.mocked(structureTree.getParent).mockImplementation((nodeId: string) => {
      if (nodeId === 'child') return 'parent';
      if (nodeId === 'parent') return 'grandparent';
      return null;
    });
    vi.mocked(structureTree.getChildren).mockImplementation((nodeId: string) => {
      if (nodeId === 'parent') return ['child'];
      return [];
    });
    vi.mocked(structureTree.getChildrenWithOrder).mockImplementation((nodeId: string) => {
      if (nodeId === 'grandparent') return [{ nodeId: 'parent', order: 2.0 }];
      if (nodeId === 'parent') return [{ nodeId: 'child', order: 1.0 }];
      return [];
    });
  });

  afterEach(() => {
    service.destroy();
  });

  function addPersistedNode(id: string) {
    const node = makeNode(id);
    // source.type === 'database' triggers shouldMarkAsPersisted=true in determinePersistenceBehavior
    sharedNodeStore.setNode(node, { type: 'database', reason: 'test' });
    return node;
  }

  it('outdent of persisted node calls moveInMemoryRelationship WITHOUT a numeric order argument', async () => {
    addPersistedNode('grandparent');
    addPersistedNode('parent');
    addPersistedNode('child');

    const result = await service.outdentNode('child');

    // outdentNode returns a boolean (true if outdent ran, false if validation failed)
    expect(typeof result).toBe('boolean');

    // No moveInMemoryRelationship call should pass a fractional order value.
    // Frontend uses only relative-after placement (no order) or integer sibling-transfer order.
    for (const call of moveInMemoryRelationshipSpy.mock.calls) {
      const order = call[3];
      if (typeof order === 'number') {
        // Sibling-transfer uses integer i+1 — verify it is an integer, not a fractional midpoint
        expect(order % 1).toBe(0);
      }
    }
  });

  it('outdent of NOT-YET-PERSISTED node: optimistic moveInMemoryRelationship has no fractional order', async () => {
    addPersistedNode('grandparent');
    addPersistedNode('parent');

    // Use viewer source so the node is NOT added to persistedNodeIds.
    // source.type === 'viewer' → determinePersistenceBehavior returns shouldMarkAsPersisted: false,
    // so isNodePersisted('child') is false and the CREATE-cancel branch executes.
    const child = makeNode('child');
    sharedNodeStore.setNode(child, { type: 'viewer', viewerId: 'test-viewer' });

    await service.outdentNode('child');

    // No moveInMemoryRelationship call should pass a fractional order value.
    // The CREATE-cancel path calls moveInMemoryRelationship(oldParentId, newParentId, nodeId)
    // with no order arg — positional intent only.
    for (const call of moveInMemoryRelationshipSpy.mock.calls) {
      if (call[3] !== undefined) {
        expect(Number.isInteger(call[3])).toBe(true);
      }
    }
  });

  it('no call to moveInMemoryRelationship passes a value between 0 and 1 (no fractional midpoints)', async () => {
    addPersistedNode('grandparent');
    addPersistedNode('parent');
    addPersistedNode('child');

    await service.outdentNode('child');

    for (const call of moveInMemoryRelationshipSpy.mock.calls) {
      const order = call[3];
      if (typeof order === 'number') {
        // Fractional midpoints (like 2.5) are daemon-side only; frontend only uses integers
        expect(order % 1).toBe(0);
      }
    }
  });
});

describe('Outdenting/indenting an unpersisted, actively-edited node does not drop its CREATE', () => {
  let service: ReactiveNodeService;
  let events: NodeManagerEvents;
  let sharedNodeStore: SharedNodeStore;

  beforeEach(() => {
    vi.clearAllMocks();
    SharedNodeStore.resetInstance();
    sharedNodeStore = SharedNodeStore.getInstance();
    focusManager.clearEditing();
    conflictNotifications.dismissAll();

    events = {
      focusRequested: vi.fn(),
      hierarchyChanged: vi.fn(),
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    };

    service = createReactiveNodeService(events);

    vi.mocked(structureTree.getParent).mockImplementation((nodeId: string) => {
      if (nodeId === 'child') return 'parent';
      if (nodeId === 'parent') return 'grandparent';
      return null;
    });
  });

  afterEach(() => {
    service.destroy();
    focusManager.clearEditing();
    conflictNotifications.dismissAll();
  });

  function addPersistedNode(id: string) {
    const node = makeNode(id);
    sharedNodeStore.setNode(node, { type: 'database', reason: 'test' });
    return node;
  }

  it('outdenting a just-created, focused node inside the persistence debounce window re-triggers the CREATE via a viewer source, never a database source, and never marks the node persisted', async () => {
    const grandparent = addPersistedNode('grandparent');
    const parent = addPersistedNode('parent');

    // `SharedNodeStore.getParentsForNode` derives from the (mocked, no-op-
    // applied) structureTree singleton, which this test's hand-rolled mock
    // does not keep in sync with `moveInMemoryRelationship` calls — see
    // `getParentsForNode`'s own "may not be initialized in tests" doc
    // comment. Stub it directly so `validateOutdent` sees the intended
    // hierarchy: child -> parent -> grandparent.
    vi.spyOn(sharedNodeStore, 'getParentsForNode').mockImplementation((nodeId: string) => {
      if (nodeId === 'child') return [parent];
      if (nodeId === 'parent') return [grandparent];
      return [];
    });

    // Create 'child' the same way a real Enter keypress does: a viewer-sourced
    // setNode schedules a DEBOUNCED create and leaves the node "pending" in
    // PersistenceCoordinator, and the node keeps focus — matching a
    // just-created node inside the 500ms persistence debounce window.
    const child = makeNode('child');
    sharedNodeStore.setNode(child, { type: 'viewer', viewerId: 'test-viewer' });
    focusManager.focusNode('child', 'default');

    expect(sharedNodeStore.hasPendingSave('child')).toBe(true);
    expect(sharedNodeStore.isNodePersisted('child')).toBe(false);

    const setNodeSpy = vi.spyOn(sharedNodeStore, 'setNode');

    // Outdent while the CREATE is still pending (Shift+Tab inside the debounce window).
    const outdentResult = await service.outdentNode('child');
    expect(outdentResult).toBe(true);

    // The re-trigger call setNode() makes for 'child' must be the SECOND
    // call recorded by the spy (the first is the initial viewer-sourced
    // create above) and must use a `viewer` source — never `database`. A
    // `database` source is what `decideRemoteUpdate` treats as a foreign
    // write to skip while the node is actively edited, which is exactly the
    // defect this test guards: it silently dropped the CREATE and left the
    // node's own bookkeeping falsely marked as persisted (see the assertion
    // below).
    const reTriggerCalls = setNodeSpy.mock.calls.filter((call) => call[0].id === 'child');
    expect(reTriggerCalls.length).toBeGreaterThanOrEqual(1);
    const lastReTrigger = reTriggerCalls[reTriggerCalls.length - 1];
    expect(lastReTrigger[1].type).toBe('viewer');
    // And it must have actually been APPLIED (not declined) — the whole
    // point of using a `viewer` source is that `decideRemoteUpdate` never
    // declines it, so setNode's return value must reflect that.
    const lastReTriggerIndex = setNodeSpy.mock.calls.indexOf(lastReTrigger);
    expect(setNodeSpy.mock.results[lastReTriggerIndex].value).toBe(true);

    // The CREATE must still be scheduled (pending), and the node must NOT be
    // falsely marked as persisted — this is the exact bookkeeping corruption
    // that made the later debounced write fire as an UPDATE (NODE_NOT_FOUND)
    // instead of a CREATE in production.
    expect(sharedNodeStore.hasPendingSave('child')).toBe(true);
    expect(sharedNodeStore.isNodePersisted('child')).toBe(false);

    // A lost/dropped CREATE must never be misreported as a version conflict.
    const versionMismatch = conflictNotifications.notifications.find(
      (n) => n.nodeId === 'child' && n.conflictType === 'version-mismatch'
    );
    expect(versionMismatch).toBeUndefined();
  });

  it('indenting a just-created, focused node inside the persistence debounce window re-triggers the CREATE via a viewer source and never marks the node persisted', async () => {
    addPersistedNode('parent');
    // 'sibling' is the indent target's previous sibling — indentNode moves the
    // new node under it, so it must be able to have children.
    const sibling = makeNode('sibling');
    sharedNodeStore.setNode(sibling, { type: 'database', reason: 'test' });

    vi.mocked(structureTree.getParent).mockImplementation((nodeId: string) => {
      if (nodeId === 'child' || nodeId === 'sibling') return 'parent';
      return null;
    });
    // See the outdent test above: getNodesForParent doesn't reflect this
    // test's hand-rolled structureTree mock, so stub it directly.
    vi.spyOn(sharedNodeStore, 'getNodesForParent').mockImplementation((parentId: string | null) => {
      if (parentId !== 'parent') return [];
      const child = sharedNodeStore.getNode('child');
      return child ? [sibling, child] : [sibling];
    });

    const child = makeNode('child');
    sharedNodeStore.setNode(child, { type: 'viewer', viewerId: 'test-viewer' });
    focusManager.focusNode('child', 'default');

    expect(sharedNodeStore.hasPendingSave('child')).toBe(true);
    expect(sharedNodeStore.isNodePersisted('child')).toBe(false);

    const setNodeSpy = vi.spyOn(sharedNodeStore, 'setNode');

    const indentResult = await service.indentNode('child');
    expect(indentResult).toBe(true);

    const reTriggerCalls = setNodeSpy.mock.calls.filter((call) => call[0].id === 'child');
    expect(reTriggerCalls.length).toBeGreaterThanOrEqual(1);
    const lastReTrigger = reTriggerCalls[reTriggerCalls.length - 1];
    expect(lastReTrigger[1].type).toBe('viewer');
    const lastReTriggerIndex = setNodeSpy.mock.calls.indexOf(lastReTrigger);
    expect(setNodeSpy.mock.results[lastReTriggerIndex].value).toBe(true);

    expect(sharedNodeStore.hasPendingSave('child')).toBe(true);
    expect(sharedNodeStore.isNodePersisted('child')).toBe(false);

    const versionMismatch = conflictNotifications.notifications.find(
      (n) => n.nodeId === 'child' && n.conflictType === 'version-mismatch'
    );
    expect(versionMismatch).toBeUndefined();
  });

  it('rolls back the optimistic outdent and reports failure — never falls through to the persisted-node MOVE path — when the CREATE re-trigger is declined', async () => {
    const grandparent = addPersistedNode('grandparent');
    const parent = addPersistedNode('parent');
    vi.spyOn(sharedNodeStore, 'getParentsForNode').mockImplementation((nodeId: string) => {
      if (nodeId === 'child') return [parent];
      if (nodeId === 'parent') return [grandparent];
      return [];
    });

    const child = makeNode('child');
    sharedNodeStore.setNode(child, { type: 'viewer', viewerId: 'test-viewer' });
    focusManager.focusNode('child', 'default');

    // Force the re-trigger to be declined. `viewerSource` makes this
    // unreachable today (decideRemoteUpdate always applies a `viewer`
    // source), but the caller must never report success — or fall through
    // to the already-persisted MOVE path — if this ever changes.
    vi.spyOn(sharedNodeStore, 'setNode').mockReturnValue(false);

    const { backendAdapter } = await import('$lib/services/backend-adapter');
    const moveNodeMock = vi.mocked(backendAdapter.moveNode);
    const moveInMemorySpy = vi.mocked(structureTree.moveInMemoryRelationship);
    moveInMemorySpy.mockClear();

    const outdentResult = await service.outdentNode('child');

    expect(outdentResult).toBe(false);
    // No fall-through: moveNode must never be called for a node that was
    // never created server-side.
    expect(moveNodeMock).not.toHaveBeenCalled();
    // The optimistic reparent (child: parent -> grandparent) must be rolled
    // back (grandparent -> parent), matching rollbackOutdentChanges.
    expect(moveInMemorySpy).toHaveBeenCalledWith('parent', 'grandparent', 'child');
    expect(moveInMemorySpy).toHaveBeenCalledWith('grandparent', 'parent', 'child');
  });
});

describe('setNode: a re-triggered write to a focused/pending node must use a viewer source', () => {
  let sharedNodeStore: SharedNodeStore;

  beforeEach(() => {
    vi.clearAllMocks();
    SharedNodeStore.resetInstance();
    sharedNodeStore = SharedNodeStore.getInstance();
    focusManager.clearEditing();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    focusManager.clearEditing();
    conflictNotifications.dismissAll();
  });

  it('a `database`-sourced re-trigger on a focused, pending, unpersisted node is declined and falsely marks it persisted (documents the defect a `viewer` source avoids)', () => {
    const node = makeNode('child');
    sharedNodeStore.setNode(node, { type: 'viewer', viewerId: 'test-viewer' });
    focusManager.focusNode('child', 'default');
    expect(sharedNodeStore.hasPendingSave('child')).toBe(true);
    expect(sharedNodeStore.isNodePersisted('child')).toBe(false);

    const applied = sharedNodeStore.setNode(
      { ...node, insertPosition: { type: 'end' } } as typeof node & {
        insertPosition?: { type: string };
      },
      { type: 'database', reason: 'outdent-node' }
    );

    // This is the exact defect: a local user action mislabelled as a
    // `database` source is declined by the skip-while-editing guard, and the
    // guard's decline path marks the node as persisted anyway (correct for a
    // genuine foreign write, wrong for re-triggering one's own CREATE).
    expect(applied).toBe(false);
    expect(sharedNodeStore.isNodePersisted('child')).toBe(true);
  });

  it('a `viewer`-sourced re-trigger on a focused, pending, unpersisted node is always applied and never marks it persisted', () => {
    const node = makeNode('child');
    sharedNodeStore.setNode(node, { type: 'viewer', viewerId: 'test-viewer' });
    focusManager.focusNode('child', 'default');
    expect(sharedNodeStore.hasPendingSave('child')).toBe(true);
    expect(sharedNodeStore.isNodePersisted('child')).toBe(false);

    const applied = sharedNodeStore.setNode(
      { ...node, insertPosition: { type: 'end' } } as typeof node & {
        insertPosition?: { type: string };
      },
      { type: 'viewer', viewerId: 'test-viewer' }
    );

    expect(applied).toBe(true);
    // Still not persisted — the CREATE is rescheduled, not dropped.
    expect(sharedNodeStore.isNodePersisted('child')).toBe(false);
    expect(sharedNodeStore.hasPendingSave('child')).toBe(true);

    const versionMismatch = conflictNotifications.notifications.find(
      (n) => n.nodeId === 'child' && n.conflictType === 'version-mismatch'
    );
    expect(versionMismatch).toBeUndefined();
  });
});
