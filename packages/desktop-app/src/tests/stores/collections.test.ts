/**
 * Unit tests for collections store - Collection browser state management
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  collectionsState,
  collectionsData,
  findCollectionById,
  buildCollectionsTree,
  ROOT_COLLECTION_ID,
  NON_CONTENT_NODE_TYPES,
  type CollectionsState,
  type CollectionItem,
  type CollectionMember
} from '$lib/stores/collections.svelte';
import type { CollectionInfo } from '$lib/services/collection-service';
import type { Node } from '$lib/types';
import { mockCollections, mockMembers } from '../fixtures/collections-fixtures';
import { pluginRegistry } from '$lib/plugins/index';

// Convert mock data to CollectionInfo format for testing
function createTestCollectionInfo(item: CollectionItem, parentId?: string): CollectionInfo {
  return {
    id: item.id,
    content: item.name,
    memberCount: item.memberCount,
    nodeType: 'collection',
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    properties: {},
    parentCollectionIds: parentId ? [parentId] : []
  };
}

// Flatten collections tree to list for the data store
function flattenCollections(items: CollectionItem[], parentId?: string): CollectionInfo[] {
  const result: CollectionInfo[] = [];
  for (const item of items) {
    result.push(createTestCollectionInfo(item, parentId));
    if (item.children) {
      result.push(...flattenCollections(item.children, item.id));
    }
  }
  return result;
}

// Convert mock members to Node format
function createTestMembers(): Map<string, Node[]> {
  const result = new Map<string, Node[]>();
  for (const [collectionId, members] of Object.entries(mockMembers)) {
    result.set(
      collectionId,
      members.map((m) => ({
        id: m.id,
        content: m.name,
        title: m.name,
        nodeType: m.nodeType,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      }))
    );
  }
  return result;
}

describe('Collections Store', () => {
  beforeEach(() => {
    // Reset both stores to initial state before each test
    collectionsState.reset();
    collectionsData.reset();
  });

  describe('Initial State', () => {
    it('has correct initial state', () => {
      const state = collectionsState.state;

      expect(state.selectedCollectionId).toBeNull();
      expect(state.subPanelOpen).toBe(false);
      expect(state.expandedCollectionIds).toBeInstanceOf(Set);
      expect(state.expandedCollectionIds.size).toBe(0);
    });

    it('selectedCollection derived store returns undefined initially', () => {
      const selected = collectionsState.selectedCollection;
      expect(selected).toBeUndefined();
    });

    it('selectedCollectionMembers derived store returns empty array initially', () => {
      const members = collectionsState.selectedCollectionMembers;
      expect(members).toEqual([]);
    });
  });

  describe('selectCollection', () => {
    it('selects a collection and opens the sub-panel', () => {
      collectionsState.selectCollection('col-1');

      const state = collectionsState.state;
      expect(state.selectedCollectionId).toBe('col-1');
      expect(state.subPanelOpen).toBe(true);
    });

    it('updates selectedCollection derived store', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1');

      const selected = collectionsState.selectedCollection;
      expect(selected).toBeDefined();
      expect(selected?.id).toBe('col-1');
      expect(selected?.name).toBe('Project Ideas');
    });

    it('updates selectedCollectionMembers derived store', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1');

      const members = collectionsState.selectedCollectionMembers;
      expect(members).toHaveLength(3);
      expect(members[0].name).toBe('AI-Powered Note Taking');
    });

    it('strips the markdown heading marker from an untitled member (imported doc root)', () => {
      // Imported header roots carry their heading in `content` ("# ACP...") and
      // have no separate `title`, so the member list must strip the marker.
      const untitledMembers = new Map<string, Node[]>([
        [
          'col-1',
          [
            {
              id: 'imported-root',
              content: '# ACP Integration Architecture',
              title: '',
              nodeType: 'header',
              createdAt: new Date().toISOString(),
              modifiedAt: new Date().toISOString(),
              version: 1,
              properties: {}
            }
          ]
        ]
      ]);
      collectionsData._setTestData(flattenCollections(mockCollections), untitledMembers);

      collectionsState.selectCollection('col-1');

      const members = collectionsState.selectedCollectionMembers;
      expect(members).toHaveLength(1);
      expect(members[0].name).toBe('ACP Integration Architecture');
    });

    it('selecting a different collection replaces the selection', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1');
      collectionsState.selectCollection('col-2');

      const state = collectionsState.state;
      expect(state.selectedCollectionId).toBe('col-2');
      expect(state.subPanelOpen).toBe(true);

      const selected = collectionsState.selectedCollection;
      expect(selected?.name).toBe('Meeting Notes');
    });

    it('selecting a nested collection works correctly', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1-1');

      const selected = collectionsState.selectedCollection;
      expect(selected).toBeDefined();
      expect(selected?.id).toBe('col-1-1');
      expect(selected?.name).toBe('AI Features and Machine Learning Integration');
    });

    it('selecting a deeply nested collection works correctly', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1-1-1');

      const selected = collectionsState.selectedCollection;
      expect(selected).toBeDefined();
      expect(selected?.id).toBe('col-1-1-1');
      expect(selected?.name).toBe('Natural Language Processing Research');
    });
  });

  describe('closeSubPanel', () => {
    it('closes the sub-panel but keeps selection', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.closeSubPanel();

      const state = collectionsState.state;
      expect(state.selectedCollectionId).toBe('col-1'); // Keeps selection for visual context
      expect(state.subPanelOpen).toBe(false);
    });

    it('does nothing when called without prior selection', () => {
      collectionsState.closeSubPanel();

      const state = collectionsState.state;
      expect(state.selectedCollectionId).toBeNull();
      expect(state.subPanelOpen).toBe(false);
    });
  });

  describe('clearSelection', () => {
    it('clears selection and closes sub-panel', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.clearSelection();

      const state = collectionsState.state;
      expect(state.selectedCollectionId).toBeNull();
      expect(state.subPanelOpen).toBe(false);
    });

    it('selectedCollection derived store returns undefined after clearing', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.clearSelection();

      const selected = collectionsState.selectedCollection;
      expect(selected).toBeUndefined();
    });

    it('selectedCollectionMembers returns empty array after clearing', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.clearSelection();

      const members = collectionsState.selectedCollectionMembers;
      expect(members).toEqual([]);
    });
  });

  describe('toggleCollectionExpanded', () => {
    it('expands a collection when collapsed', () => {
      collectionsState.toggleCollectionExpanded('col-1');

      const state = collectionsState.state;
      expect(state.expandedCollectionIds.has('col-1')).toBe(true);
    });

    it('collapses a collection when expanded', () => {
      collectionsState.toggleCollectionExpanded('col-1');
      collectionsState.toggleCollectionExpanded('col-1');

      const state = collectionsState.state;
      expect(state.expandedCollectionIds.has('col-1')).toBe(false);
    });

    it('can expand multiple collections', () => {
      collectionsState.toggleCollectionExpanded('col-1');
      collectionsState.toggleCollectionExpanded('col-2');

      const state = collectionsState.state;
      expect(state.expandedCollectionIds.has('col-1')).toBe(true);
      expect(state.expandedCollectionIds.has('col-2')).toBe(true);
      expect(state.expandedCollectionIds.size).toBe(2);
    });

    it('expanding does not affect selection state', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.toggleCollectionExpanded('col-2');

      const state = collectionsState.state;
      expect(state.selectedCollectionId).toBe('col-1');
      expect(state.subPanelOpen).toBe(true);
    });
  });

  describe('reset', () => {
    it('resets all state to initial values', () => {
      // Set up some state
      collectionsState.selectCollection('col-1');
      collectionsState.toggleCollectionExpanded('col-2');
      collectionsState.toggleCollectionExpanded('col-3');

      // Verify state is modified
      let state = collectionsState.state;
      expect(state.selectedCollectionId).toBe('col-1');
      expect(state.subPanelOpen).toBe(true);
      expect(state.expandedCollectionIds.size).toBe(2);

      // Reset
      collectionsState.reset();

      // Verify state is initial
      state = collectionsState.state;
      expect(state.selectedCollectionId).toBeNull();
      expect(state.subPanelOpen).toBe(false);
      expect(state.expandedCollectionIds.size).toBe(0);
    });
  });

  describe('findCollectionById helper', () => {
    it('finds a top-level collection', () => {
      const result = findCollectionById(mockCollections, 'col-1');

      expect(result).toBeDefined();
      expect(result?.id).toBe('col-1');
      expect(result?.name).toBe('Project Ideas');
    });

    it('finds a nested collection (level 2)', () => {
      const result = findCollectionById(mockCollections, 'col-1-1');

      expect(result).toBeDefined();
      expect(result?.id).toBe('col-1-1');
      expect(result?.name).toBe('AI Features and Machine Learning Integration');
    });

    it('finds a deeply nested collection (level 3)', () => {
      const result = findCollectionById(mockCollections, 'col-1-1-1');

      expect(result).toBeDefined();
      expect(result?.id).toBe('col-1-1-1');
      expect(result?.name).toBe('Natural Language Processing Research');
    });

    it('returns undefined for non-existent collection', () => {
      const result = findCollectionById(mockCollections, 'non-existent');
      expect(result).toBeUndefined();
    });

    it('returns undefined for empty collections array', () => {
      const result = findCollectionById([], 'col-1');
      expect(result).toBeUndefined();
    });

    it('finds collections in different branches of the tree', () => {
      // Test finding collections in the second top-level branch
      const result = findCollectionById(mockCollections, 'col-2-2-1');

      expect(result).toBeDefined();
      expect(result?.id).toBe('col-2-2-1');
      expect(result?.name).toBe('Sprint Reviews');
    });
  });

  describe('Mock Data', () => {
    it('mockCollections has expected structure', () => {
      expect(mockCollections).toHaveLength(4);
      expect(mockCollections[0].id).toBe('col-1');
      expect(mockCollections[0].children).toBeDefined();
      expect(mockCollections[0].children).toHaveLength(2);
    });

    it('mockCollections has 3 levels of nesting', () => {
      // Level 1: col-1
      const level1 = mockCollections[0];
      expect(level1.id).toBe('col-1');

      // Level 2: col-1-1
      const level2 = level1.children?.[0];
      expect(level2?.id).toBe('col-1-1');

      // Level 3: col-1-1-1
      const level3 = level2?.children?.[0];
      expect(level3?.id).toBe('col-1-1-1');
    });

    it('mockMembers has members for all collections', () => {
      // Check that each collection in the tree has an entry in mockMembers
      const allCollectionIds = [
        'col-1',
        'col-1-1',
        'col-1-1-1',
        'col-1-1-2',
        'col-1-2',
        'col-2',
        'col-2-1',
        'col-2-2',
        'col-2-2-1',
        'col-2-2-2',
        'col-3',
        'col-4'
      ];

      allCollectionIds.forEach((id) => {
        expect(mockMembers).toHaveProperty(id);
      });
    });

    it('mockMembers includes an empty collection', () => {
      expect(mockMembers['col-3']).toEqual([]);
    });

    it('mockMembers has correct member structure', () => {
      const members = mockMembers['col-1'];

      expect(members).toHaveLength(3);
      members.forEach((member) => {
        expect(member).toHaveProperty('id');
        expect(member).toHaveProperty('name');
        expect(member).toHaveProperty('nodeType');
        expect(typeof member.id).toBe('string');
        expect(typeof member.name).toBe('string');
        expect(typeof member.nodeType).toBe('string');
      });
    });
  });

  describe('Derived Stores', () => {
    it('selectedCollection updates reactively when selection changes', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      expect(collectionsState.selectedCollection).toBeUndefined();

      collectionsState.selectCollection('col-1');
      expect(collectionsState.selectedCollection?.id).toBe('col-1');

      collectionsState.selectCollection('col-2');
      expect(collectionsState.selectedCollection?.id).toBe('col-2');

      collectionsState.clearSelection();
      expect(collectionsState.selectedCollection).toBeUndefined();
    });

    it('selectedCollectionMembers updates reactively when selection changes', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      expect(collectionsState.selectedCollectionMembers).toEqual([]);

      collectionsState.selectCollection('col-1');
      expect(collectionsState.selectedCollectionMembers).toHaveLength(3);

      collectionsState.selectCollection('col-3'); // Empty collection
      expect(collectionsState.selectedCollectionMembers).toEqual([]);

      collectionsState.selectCollection('col-4');
      expect(collectionsState.selectedCollectionMembers).toHaveLength(4);
    });

    it('selectedCollectionMembers returns empty for invalid selection', () => {
      collectionsState.selectCollection('non-existent');

      const members = collectionsState.selectedCollectionMembers;
      expect(members).toEqual([]);
    });
  });

  describe('selectedCollectionMembers title-vs-content', () => {
    afterEach(() => {
      pluginRegistry.unregister('widget-entity');
    });

    it('shows current content, not a stale cached title, for a non-template type', () => {
      // sharedNodeStore's cached `title` only refreshes via a backend round-trip; optimistic
      // content edits patch `content` directly. A member row must reflect current content,
      // not a title computed for an earlier state.
      const members = new Map<string, Node[]>([
        [
          'col-1',
          [
            {
              id: 'node-1',
              content: 'Another Task',
              title: '/',
              nodeType: 'task',
              createdAt: new Date().toISOString(),
              modifiedAt: new Date().toISOString(),
              version: 1,
              properties: {}
            }
          ]
        ]
      ]);
      collectionsData._setTestData(flattenCollections(mockCollections), members);

      collectionsState.selectCollection('col-1');

      expect(collectionsState.selectedCollectionMembers).toEqual([
        { id: 'node-1', name: 'Another Task', nodeType: 'task' }
      ]);
    });

    it('still shows the computed title for a title_template-driven custom entity', () => {
      pluginRegistry.register({
        id: 'widget-entity',
        name: 'Widget',
        description: 'Custom entity with a title template',
        version: '1.0.0',
        config: { slashCommands: [] },
        hasTitleTemplate: true,
        titleTemplate: '{first_name} {last_name}'
      });

      const members = new Map<string, Node[]>([
        [
          'col-1',
          [
            {
              id: 'node-1',
              content: 'raw',
              title: 'Jane Doe',
              nodeType: 'widget-entity',
              createdAt: new Date().toISOString(),
              modifiedAt: new Date().toISOString(),
              version: 1,
              properties: {}
            }
          ]
        ]
      ]);
      collectionsData._setTestData(flattenCollections(mockCollections), members);

      collectionsState.selectCollection('col-1');

      expect(collectionsState.selectedCollectionMembers).toEqual([
        { id: 'node-1', name: 'Jane Doe', nodeType: 'widget-entity' }
      ]);
    });

    it('does not run a template-computed title through markdown stripping', () => {
      // Regression: stripMarkdown must apply only to a content-sourced value, never to a
      // title_template-computed title — that's a property value, not markdown, and
      // stripMarkdown's bold/italic/code regexes would otherwise mangle any paired `_`/`*`/
      // backtick in it (e.g. a name or slug containing an underscore).
      pluginRegistry.register({
        id: 'widget-entity',
        name: 'Widget',
        description: 'Custom entity with a title template',
        version: '1.0.0',
        config: { slashCommands: [] },
        hasTitleTemplate: true,
        titleTemplate: '{first_name}_{last_name}'
      });

      const members = new Map<string, Node[]>([
        [
          'col-1',
          [
            {
              id: 'node-1',
              content: 'raw',
              title: 'Jane_Doe',
              nodeType: 'widget-entity',
              createdAt: new Date().toISOString(),
              modifiedAt: new Date().toISOString(),
              version: 1,
              properties: {}
            }
          ]
        ]
      ]);
      collectionsData._setTestData(flattenCollections(mockCollections), members);

      collectionsState.selectCollection('col-1');

      expect(collectionsState.selectedCollectionMembers).toEqual([
        { id: 'node-1', name: 'Jane_Doe', nodeType: 'widget-entity' }
      ]);
    });
  });

  describe('collectionsTree hide-empty filter', () => {
    it('hides top-level collections with no visible members', () => {
      // Fixtures include col-3 "Research Papers" with memberCount: 0 (leaf).
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      const tree = collectionsData.collectionsTree;
      const ids = tree.map((c) => c.id);

      expect(ids).not.toContain('col-3');
      // Populated top-level collections remain.
      expect(ids).toEqual(expect.arrayContaining(['col-1', 'col-2', 'col-4']));
    });

    it('keeps an empty parent when a descendant has visible members', () => {
      const collections: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: 'parent', name: 'Empty Parent', memberCount: 0 }),
          parentCollectionIds: []
        },
        {
          ...createTestCollectionInfo({ id: 'child', name: 'Populated Child', memberCount: 2 }),
          parentCollectionIds: ['parent']
        }
      ];
      collectionsData._setTestData(collections, new Map());

      const tree = collectionsData.collectionsTree;
      expect(tree.map((c) => c.id)).toEqual(['parent']);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['child']);
    });

    it('prunes empty children while keeping populated siblings', () => {
      const collections: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: 'parent', name: 'Parent', memberCount: 1 }),
          parentCollectionIds: []
        },
        {
          ...createTestCollectionInfo({ id: 'empty-child', name: 'Empty', memberCount: 0 }),
          parentCollectionIds: ['parent']
        },
        {
          ...createTestCollectionInfo({ id: 'full-child', name: 'Full', memberCount: 3 }),
          parentCollectionIds: ['parent']
        }
      ];
      collectionsData._setTestData(collections, new Map());

      const tree = collectionsData.collectionsTree;
      expect(tree).toHaveLength(1);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['full-child']);
    });

    it('drops an empty parent whose descendants are all empty', () => {
      const collections: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: 'parent', name: 'Parent', memberCount: 0 }),
          parentCollectionIds: []
        },
        {
          ...createTestCollectionInfo({ id: 'child', name: 'Child', memberCount: 0 }),
          parentCollectionIds: ['parent']
        }
      ];
      collectionsData._setTestData(collections, new Map());

      expect(collectionsData.collectionsTree).toEqual([]);
    });
  });

  describe('buildCollectionsTree dynamic bound-root filtering', () => {
    // A per-install workspace root: a random uuid minted per install,
    // NOT the well-known legacy id. Its member_of edge must be hidden the same
    // way the legacy root's is, or the user's top-level collections wrongly nest
    // under it in the sidebar.
    const PER_INSTALL_ROOT = 'a1b2c3d4-1111-2222-3333-444455556666';

    // Two user collections whose only parent is the per-install root.
    const underPerInstallRoot: CollectionInfo[] = [
      {
        ...createTestCollectionInfo({ id: 'engineering', name: 'Engineering', memberCount: 3 }),
        parentCollectionIds: [PER_INSTALL_ROOT]
      },
      {
        ...createTestCollectionInfo({ id: 'design', name: 'Design', memberCount: 2 }),
        parentCollectionIds: [PER_INSTALL_ROOT]
      }
    ];

    it('renders collections member_of the per-install root as top-level peers when that root is passed', () => {
      const tree = buildCollectionsTree(underPerInstallRoot, new Set(), new Set(), PER_INSTALL_ROOT);

      // Peers, not nested: neither has children, and both are top-level.
      expect(tree.map((c) => c.id)).toEqual(['design', 'engineering']); // sorted by name
      expect(tree.every((c) => (c.children?.length ?? 0) === 0)).toBe(true);
    });

    it('excludes the workspace root NODE from top-level even when it has content members', () => {
      // get_all_collections returns the root node itself; with content member_of
      // edges (memberCount > 0) it survives pruning, so filtering it only as a
      // parent would still leave it visible as a top-level peer — the exact bug.
      const withRootNode: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: PER_INSTALL_ROOT, name: 'My Workspace', memberCount: 5 }),
          parentCollectionIds: []
        },
        ...underPerInstallRoot
      ];

      const tree = buildCollectionsTree(withRootNode, new Set(), new Set(), PER_INSTALL_ROOT);

      // The root node is gone; its children are the top-level peers.
      expect(tree.find((c) => c.id === PER_INSTALL_ROOT)).toBeUndefined();
      expect(tree.map((c) => c.id)).toEqual(['design', 'engineering']);
    });

    it('would WRONGLY nest them under the root when the stale legacy constant is used', () => {
      // get_all_collections returns the per-install root node itself, so with the
      // wrong root id it is treated as a real display parent that swallows the
      // user's collections — exactly the regression this issue fixes.
      const withRootNode: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({
            id: PER_INSTALL_ROOT,
            name: 'My Workspace',
            memberCount: 5
          }),
          parentCollectionIds: []
        },
        ...underPerInstallRoot
      ];

      const tree = buildCollectionsTree(withRootNode, new Set(), new Set(), ROOT_COLLECTION_ID);

      // Legacy constant does not match the per-install root → root nests everything.
      expect(tree.map((c) => c.id)).toEqual([PER_INSTALL_ROOT]);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['design', 'engineering']);
    });

    it('falls back to the legacy ROOT_COLLECTION_ID when no root id is given (public/legacy tenant)', () => {
      const underLegacyRoot: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: 'hr', name: 'HR', memberCount: 1 }),
          parentCollectionIds: [ROOT_COLLECTION_ID]
        },
        {
          ...createTestCollectionInfo({ id: 'finance', name: 'Finance', memberCount: 1 }),
          parentCollectionIds: [ROOT_COLLECTION_ID]
        }
      ];

      // No 4th arg → the default (ROOT_COLLECTION_ID) is applied, so legacy-rooted
      // collections still render as peers, unchanged from before the fix.
      const tree = buildCollectionsTree(underLegacyRoot);

      expect(tree.map((c) => c.id)).toEqual(['finance', 'hr']); // sorted by name
      expect(tree.every((c) => (c.children?.length ?? 0) === 0)).toBe(true);
    });

    it('still nests genuine sub-collections under their real (non-root) parent', () => {
      const nested: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: 'engineering', name: 'Engineering', memberCount: 2 }),
          parentCollectionIds: [PER_INSTALL_ROOT]
        },
        {
          ...createTestCollectionInfo({ id: 'backend', name: 'Backend', memberCount: 1 }),
          // Real parent (a normal sub-collection edge), not the workspace root.
          parentCollectionIds: ['engineering']
        }
      ];

      const tree = buildCollectionsTree(nested, new Set(), new Set(), PER_INSTALL_ROOT);

      // engineering is a top-level peer (its root edge is filtered); backend nests.
      expect(tree.map((c) => c.id)).toEqual(['engineering']);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['backend']);
    });
  });

  describe('NON_CONTENT_NODE_TYPES member filter', () => {
    // Build a full Node for a given type (only the fields the filter/mapper read).
    function mkNode(id: string, nodeType: string, name: string): Node {
      return {
        id,
        content: name,
        title: name,
        nodeType,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };
    }

    it('exports the expected non-content node types', () => {
      for (const t of ['schema', 'person', 'database-settings', 'collection', 'horizontal-line']) {
        expect(NON_CONTENT_NODE_TYPES.has(t)).toBe(true);
      }
      // Genuine user-authored content types are NOT in the set.
      for (const t of ['text', 'task', 'header', 'code-block', 'date']) {
        expect(NON_CONTENT_NODE_TYPES.has(t)).toBe(false);
      }
    });

    it('drops non-content members (creator person, system, sub-collection, divider) from Contents', () => {
      const mixed = new Map<string, Node[]>([
        [
          'col-1',
          [
            mkNode('text-1', 'text', 'A note'),
            mkNode('creator', 'person', 'Alice'), // stamped creator — must drop
            mkNode('task-1', 'task', 'Do the thing'),
            mkNode('schema-1', 'schema', 'Schema'), // system definition — must drop
            mkNode('sub-col', 'collection', 'Sub'), // shown in the tree — must drop
            mkNode('divider', 'horizontal-line', ''), // decorative — must drop
            mkNode('code-1', 'code-block', 'console.log()')
          ]
        ]
      ]);
      collectionsData._setTestData(flattenCollections(mockCollections), mixed);

      collectionsState.selectCollection('col-1');

      const members = collectionsState.selectedCollectionMembers;
      // Only genuine content survives, in original order.
      expect(members.map((m) => m.id)).toEqual(['text-1', 'task-1', 'code-1']);
      expect(members.every((m) => !NON_CONTENT_NODE_TYPES.has(m.nodeType))).toBe(true);
    });
  });

  describe('Type Definitions', () => {
    it('CollectionItem interface is correctly structured', () => {
      const item: CollectionItem = {
        id: 'test-id',
        name: 'Test Name',
        memberCount: 5,
        children: [{ id: 'child-id', name: 'Child Name', memberCount: 2 }]
      };

      expect(item.id).toBe('test-id');
      expect(item.name).toBe('Test Name');
      expect(item.memberCount).toBe(5);
      expect(item.children).toHaveLength(1);
    });

    it('CollectionMember interface is correctly structured', () => {
      const member: CollectionMember = {
        id: 'node-id',
        name: 'Node Name',
        nodeType: 'text'
      };

      expect(member.id).toBe('node-id');
      expect(member.name).toBe('Node Name');
      expect(member.nodeType).toBe('text');
    });

    it('CollectionsState interface is correctly structured', () => {
      const state: CollectionsState = {
        selectedCollectionId: 'col-1',
        subPanelOpen: true,
        expandedCollectionIds: new Set(['col-1', 'col-2'])
      };

      expect(state.selectedCollectionId).toBe('col-1');
      expect(state.subPanelOpen).toBe(true);
      expect(state.expandedCollectionIds.size).toBe(2);
    });
  });
});
