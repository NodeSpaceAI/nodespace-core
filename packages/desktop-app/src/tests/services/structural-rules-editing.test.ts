/**
 * The editor follows the structural rules (ADR-089): indent, outdent and a
 * type change are not offered where a type's `children` or `parent` rule would
 * refuse the result. The database enforces the rules on every write; these
 * tests cover the editor declining to ask.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  createReactiveNodeService,
  type ReactiveNodeService,
  type NodeManagerEvents
} from '$lib/services/reactive-node-service.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import {
  canHaveChild,
  setTypeResolver,
  type TypeDeclaration
} from '$lib/types/core-node-types';
import { waitForPendingMoveOperations } from '$lib/services/pending-operations';
import type { Node } from '$lib/types';

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    moveNode: vi.fn().mockResolvedValue({
      node: {
        id: 'mock-node',
        nodeType: 'text',
        content: '',
        version: 2,
        properties: {},
        createdAt: '2026-01-01T00:00:00.000Z',
        modifiedAt: '2026-01-01T00:00:00.000Z'
      },
      placement: null
    }),
    moveChildrenToParent: vi.fn().mockResolvedValue({ nodes: [], orders: [] }),
    getNode: vi.fn().mockResolvedValue(null),
    createNode: vi.fn().mockResolvedValue({ id: 'mock-id', placement: null }),
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

function node(id: string, nodeType: string): Node {
  return {
    lifecycleStatus: 'active',
    id,
    nodeType,
    content: `Content of ${id}`,
    version: 1,
    properties: {},
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z'
  };
}

describe('the editor follows the structural rules', () => {
  let service: ReactiveNodeService;
  let events: NodeManagerEvents;

  /**
   * Load a tree: `[id, nodeType, parentId]` rows, in sibling order.
   */
  function load(rows: Array<[string, string, string | null]>): void {
    rows.forEach(([id, , parentId], order) => {
      if (parentId) structureTree.addChild({ parentId, childId: id, order: order + 1 });
    });
    service.initializeNodes(rows.map(([id, nodeType]) => node(id, nodeType)));
  }

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    structureTree.clear();
    events = {
      focusRequested: vi.fn(),
      hierarchyChanged: vi.fn(),
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    };
    service = createReactiveNodeService(events);
  });

  afterEach(async () => {
    await waitForPendingMoveOperations();
    service.destroy();
    structureTree.clear();
    setTypeResolver(() => undefined);
  });

  describe('indent', () => {
    it('is refused under a type that takes no children', async () => {
      load([
        ['code', 'code-block', null],
        ['line', 'text', null]
      ]);
      expect(await service.indentNode('line')).toBe(false);
      expect(structureTree.getParent('line')).toBeNull();
    });

    it('is refused for a type that is always a root', async () => {
      load([
        ['page', 'text', null],
        ['work', 'collection', null]
      ]);
      expect(await service.indentNode('work')).toBe(false);
      expect(structureTree.getParent('work')).toBeNull();
    });

    it('is not offered under a chat, though the rules allow it', async () => {
      // An editor choice: a chat may have children, but the outline does not
      // put a node under one while the chat viewer has no way to show it.
      expect(canHaveChild('ai-chat-native', 'text')).toBe(true);
      load([
        ['chat', 'ai-chat-native', null],
        ['line', 'text', null]
      ]);
      expect(await service.indentNode('line')).toBe(false);
      expect(structureTree.getParent('line')).toBeNull();
    });

    it('follows the rules a user-defined type declares, and those it inherits', async () => {
      const schemas: Record<string, TypeDeclaration> = {
        journal: { children: { rule: 'any_except', types: ['task'] } },
        team: { extends: 'collection' }
      };
      setTypeResolver((id) => schemas[id]);
      load([
        ['journal', 'journal', null],
        ['todo', 'task', null],
        ['team', 'team', null]
      ]);
      // A journal takes no task, and a team is a collection: always a root.
      expect(await service.indentNode('todo')).toBe(false);
      expect(await service.indentNode('team')).toBe(false);
    });
  });

  describe('outdent', () => {
    it('is refused when the siblings below cannot become children of the node', async () => {
      load([
        ['page', 'text', null],
        ['section', 'text', 'page'],
        ['code', 'code-block', 'section'],
        ['below', 'text', 'section']
      ]);
      // `below` would become a child of the code block, which takes none.
      expect(await service.outdentNode('code')).toBe(false);
      expect(structureTree.getParent('code')).toBe('section');
      expect(structureTree.getParent('below')).toBe('section');
    });

    it('is offered when the rules allow where every node lands', async () => {
      load([
        ['page', 'text', null],
        ['section', 'text', 'page'],
        ['line', 'text', 'section'],
        ['below', 'text', 'section']
      ]);
      expect(await service.outdentNode('line')).toBe(true);
      expect(structureTree.getParent('line')).toBe('page');
      expect(structureTree.getParent('below')).toBe('line');
    });

    it('is refused for a type that needs the parent it would leave', async () => {
      const schemas: Record<string, TypeDeclaration> = {
        thread: {},
        reply: { parent: { rule: 'must_have_parent_of', types: ['thread'] } }
      };
      setTypeResolver((id) => schemas[id]);
      load([
        ['page', 'text', null],
        ['thread', 'thread', 'page'],
        ['reply', 'reply', 'thread']
      ]);
      expect(await service.outdentNode('reply')).toBe(false);
      expect(structureTree.getParent('reply')).toBe('thread');
    });
  });

  describe('type change', () => {
    beforeEach(() => {
      load([
        ['page', 'text', null],
        ['section', 'text', 'page'],
        ['line', 'text', 'section']
      ]);
    });

    it('is refused when the node has children and the new type takes none', () => {
      expect(service.canTakeType('section', 'code-block')).toBe(false);
      expect(service.canTakeType('section', 'ordered-list')).toBe(false);
      expect(service.canTakeType('section', 'header')).toBe(true);
    });

    it('is refused when the node has a parent and the new type is always a root', () => {
      expect(service.canTakeType('line', 'collection')).toBe(false);
      expect(service.canTakeType('line', 'code-block')).toBe(true);
    });

    it('is refused for a root when the new type needs a parent', () => {
      const schemas: Record<string, TypeDeclaration> = {
        thread: {},
        reply: { parent: { rule: 'must_have_parent_of', types: ['thread'] } }
      };
      setTypeResolver((id) => schemas[id]);
      expect(service.canTakeType('page', 'reply')).toBe(false);
      expect(service.canTakeType('page', 'thread')).toBe(true);
    });

    it('is refused when the parent does not take the new type', () => {
      const schemas: Record<string, TypeDeclaration> = {
        journal: { children: { rule: 'any_except', types: ['task'] } }
      };
      setTypeResolver((id) => schemas[id]);
      SharedNodeStore.getInstance().updateNode(
        'section',
        { nodeType: 'journal' },
        { type: 'database', reason: 'test' },
        { skipPersistence: true }
      );
      expect(service.canTakeType('line', 'task')).toBe(false);
      expect(service.canTakeType('line', 'header')).toBe(true);
    });
  });
});
