/**
 * Unit tests for collections store - Collection browser state management
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  collectionsState,
  collectionsData,
  findCollectionById,
  buildCollectionsTree,
  NON_CONTENT_NODE_TYPES,
  type CollectionsState,
  type CollectionItem,
  type CollectionMember
} from '$lib/stores/collections.svelte';
import type { CollectionInfo } from '$lib/services/collection-service';
import type { Node } from '$lib/types';
import { mockCollections, mockMembers } from '../fixtures/collections-fixtures';
import { pluginRegistry } from '$lib/plugins/index';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags
} from '../fixtures/test-extension';

// The id of the old shared workspace root. Core gives it no special meaning, so
// the tests use it to check that a collection with this id is an ordinary one.
const LEGACY_WORKSPACE_ROOT_ID = 'c0000000-0000-0000-0000-000000000001';

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

    it('hides no container collection when no extension is registered', () => {
      // No extension contributes roots, so a container collection, even one with
      // the legacy workspace-root id, is an ordinary top-level row holding its
      // member collections.
      expect(uiExtensionRegistry.collectionTreeRoots().size).toBe(0);
      const collections: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({
            id: LEGACY_WORKSPACE_ROOT_ID,
            name: 'Default',
            memberCount: 2
          }),
          parentCollectionIds: []
        },
        {
          ...createTestCollectionInfo({ id: 'engineering', name: 'Engineering', memberCount: 3 }),
          parentCollectionIds: [LEGACY_WORKSPACE_ROOT_ID]
        },
        {
          ...createTestCollectionInfo({ id: 'design', name: 'Design', memberCount: 2 }),
          parentCollectionIds: [LEGACY_WORKSPACE_ROOT_ID]
        }
      ];
      collectionsData._setTestData(collections, new Map());

      const tree = collectionsData.collectionsTree;
      expect(tree.map((c) => c.id)).toEqual([LEGACY_WORKSPACE_ROOT_ID]);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['design', 'engineering']);
    });
  });

  describe('buildCollectionsTree root filtering', () => {
    // A root collection: a container the tree does not show. Collections whose
    // only parent is a root show at the top level instead of nesting under it.
    const ROOT = 'a1b2c3d4-1111-2222-3333-444455556666';

    // Two collections whose only parent is the root.
    const underRoot: CollectionInfo[] = [
      {
        ...createTestCollectionInfo({ id: 'engineering', name: 'Engineering', memberCount: 3 }),
        parentCollectionIds: [ROOT]
      },
      {
        ...createTestCollectionInfo({ id: 'design', name: 'Design', memberCount: 2 }),
        parentCollectionIds: [ROOT]
      }
    ];

    // The root collection itself, with content members of its own.
    const withRootNode: CollectionInfo[] = [
      {
        ...createTestCollectionInfo({ id: ROOT, name: 'Container', memberCount: 5 }),
        parentCollectionIds: []
      },
      ...underRoot
    ];

    it('renders collections member_of the root as top-level peers when that root is passed', () => {
      const tree = buildCollectionsTree(underRoot, new Set(), new Set(), new Set([ROOT]));

      // Peers, not nested: neither has children, and both are top-level.
      expect(tree.map((c) => c.id)).toEqual(['design', 'engineering']); // sorted by name
      expect(tree.every((c) => (c.children?.length ?? 0) === 0)).toBe(true);
    });

    it('excludes the root collection from the top level even when it has content members', () => {
      // A root with content members (memberCount > 0) survives pruning, so
      // filtering it only as a parent would still leave its own row visible.
      const tree = buildCollectionsTree(withRootNode, new Set(), new Set(), new Set([ROOT]));

      // The root is gone; its members are the top-level peers.
      expect(tree.find((c) => c.id === ROOT)).toBeUndefined();
      expect(tree.map((c) => c.id)).toEqual(['design', 'engineering']);
    });

    it('with no roots, a container collection is an ordinary top-level collection with its members nested under it', () => {
      // No roots argument: the default hides nothing.
      const tree = buildCollectionsTree(withRootNode);

      expect(tree.map((c) => c.id)).toEqual([ROOT]);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['design', 'engineering']);
    });

    it('does not special-case the legacy workspace-root id', () => {
      const underLegacyRoot: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({
            id: LEGACY_WORKSPACE_ROOT_ID,
            name: 'Default',
            memberCount: 2
          }),
          parentCollectionIds: []
        },
        {
          ...createTestCollectionInfo({ id: 'hr', name: 'HR', memberCount: 1 }),
          parentCollectionIds: [LEGACY_WORKSPACE_ROOT_ID]
        },
        {
          ...createTestCollectionInfo({ id: 'finance', name: 'Finance', memberCount: 1 }),
          parentCollectionIds: [LEGACY_WORKSPACE_ROOT_ID]
        }
      ];

      // With no roots, it is an ordinary top-level collection holding its members.
      const tree = buildCollectionsTree(underLegacyRoot);
      expect(tree.map((c) => c.id)).toEqual([LEGACY_WORKSPACE_ROOT_ID]);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['finance', 'hr']); // sorted by name

      // A set of roots that does not name it leaves it the same.
      expect(buildCollectionsTree(underLegacyRoot, new Set(), new Set(), new Set([ROOT]))).toEqual(
        tree
      );
    });

    it('still nests genuine sub-collections under their real (non-root) parent', () => {
      const nested: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: 'engineering', name: 'Engineering', memberCount: 2 }),
          parentCollectionIds: [ROOT]
        },
        {
          ...createTestCollectionInfo({ id: 'backend', name: 'Backend', memberCount: 1 }),
          // Real parent (a normal sub-collection edge), not the root.
          parentCollectionIds: ['engineering']
        }
      ];

      const tree = buildCollectionsTree(nested, new Set(), new Set(), new Set([ROOT]));

      // engineering is a top-level peer (its root edge is filtered); backend nests.
      expect(tree.map((c) => c.id)).toEqual(['engineering']);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['backend']);
    });

    it('hides every id in a set of two roots, as parents and as rows', () => {
      const SECOND_ROOT = 'b9b8b7b6-1111-2222-3333-444455556666';
      const twoRoots: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: ROOT, name: 'First root', memberCount: 4 }),
          parentCollectionIds: []
        },
        {
          ...createTestCollectionInfo({ id: SECOND_ROOT, name: 'Second root', memberCount: 4 }),
          parentCollectionIds: []
        },
        {
          ...createTestCollectionInfo({ id: 'engineering', name: 'Engineering', memberCount: 3 }),
          parentCollectionIds: [ROOT]
        },
        {
          ...createTestCollectionInfo({ id: 'design', name: 'Design', memberCount: 2 }),
          parentCollectionIds: [SECOND_ROOT]
        }
      ];

      const tree = buildCollectionsTree(
        twoRoots,
        new Set(),
        new Set(),
        new Set([ROOT, SECOND_ROOT])
      );

      // Neither root is a row, and the collections under each are top-level peers.
      expect(tree.map((c) => c.id)).toEqual(['design', 'engineering']);
      expect(tree.every((c) => (c.children?.length ?? 0) === 0)).toBe(true);
    });

    it('treats a collection as ordinary when the set is empty, nesting its member collections', () => {
      const withContainer: CollectionInfo[] = [
        {
          ...createTestCollectionInfo({ id: 'container', name: 'Container', memberCount: 1 }),
          parentCollectionIds: []
        },
        {
          ...createTestCollectionInfo({ id: 'engineering', name: 'Engineering', memberCount: 3 }),
          parentCollectionIds: ['container']
        },
        {
          ...createTestCollectionInfo({ id: 'design', name: 'Design', memberCount: 2 }),
          parentCollectionIds: ['container']
        }
      ];

      const tree = buildCollectionsTree(withContainer, new Set(), new Set(), new Set());

      // Nothing is hidden, so the container is a normal row holding both collections.
      expect(tree.map((c) => c.id)).toEqual(['container']);
      expect(tree[0].children?.map((c) => c.id)).toEqual(['design', 'engineering']);
    });
  });

  describe('collectionsTree unions extension collection-tree roots', () => {
    const EXTENSION_ROOT = 'ext-root';

    // The extension root's node, a collection under it, a top-level collection,
    // and a collection nested in a regular parent.
    const collections: CollectionInfo[] = [
      {
        ...createTestCollectionInfo({ id: EXTENSION_ROOT, name: 'Extension root', memberCount: 5 }),
        parentCollectionIds: []
      },
      {
        ...createTestCollectionInfo({ id: 'engineering', name: 'Engineering', memberCount: 3 }),
        parentCollectionIds: [EXTENSION_ROOT]
      },
      {
        ...createTestCollectionInfo({ id: 'hr', name: 'HR', memberCount: 1 }),
        parentCollectionIds: []
      },
      {
        ...createTestCollectionInfo({ id: 'backend', name: 'Backend', memberCount: 1 }),
        parentCollectionIds: ['engineering']
      }
    ];

    const treeIds = () => collectionsData.collectionsTree.map((c) => c.id);

    beforeEach(() => {
      collectionsData._setTestData(collections, new Map());
    });

    afterEach(() => {
      uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
      resetTestExtension();
    });

    it('matches the unextended tree when no extension is registered', () => {
      // Nothing is hidden: the extension root is an ordinary row.
      expect(collectionsData.collectionsTree).toEqual(buildCollectionsTree(collections));
      expect(treeIds()).toEqual(['ext-root', 'hr']);
      expect(collectionsData.collectionsTree[0].children?.map((c) => c.id)).toEqual([
        'engineering'
      ]);
    });

    it('hides the extension root and shows its members at the top level', () => {
      testExtensionFlags.collectionTreeRoots = [EXTENSION_ROOT];
      uiExtensionRegistry.register(createTestExtension());

      expect(treeIds()).toEqual(['engineering', 'hr']);
      const engineering = collectionsData.collectionsTree.find((c) => c.id === 'engineering');
      expect(engineering?.children?.map((c) => c.id)).toEqual(['backend']);
    });

    it('follows the reactive state the extension reads on each read', () => {
      uiExtensionRegistry.register(createTestExtension());
      expect(treeIds()).toEqual(['ext-root', 'hr']);

      testExtensionFlags.collectionTreeRoots = [EXTENSION_ROOT];
      expect(treeIds()).toEqual(['engineering', 'hr']);

      testExtensionFlags.collectionTreeRoots = [];
      expect(treeIds()).toEqual(['ext-root', 'hr']);
    });

    it('leaves the tree intact when a contributor throws', () => {
      uiExtensionRegistry.register(
        createTestExtension({
          collectionTreeRoots: () => {
            throw new Error('collectionTreeRoots failed');
          }
        })
      );

      expect(collectionsData.collectionsTree).toEqual(buildCollectionsTree(collections));
      expect(treeIds()).toEqual(['ext-root', 'hr']);
    });

    it('restores the previous tree when the extension is unregistered', () => {
      const before = collectionsData.collectionsTree;
      testExtensionFlags.collectionTreeRoots = [EXTENSION_ROOT];
      uiExtensionRegistry.register(createTestExtension());
      expect(collectionsData.collectionsTree).not.toEqual(before);

      uiExtensionRegistry.unregister(TEST_EXTENSION_ID);

      expect(collectionsData.collectionsTree).toEqual(before);
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
