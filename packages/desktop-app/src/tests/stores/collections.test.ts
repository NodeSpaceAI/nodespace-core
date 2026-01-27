/**
 * Unit tests for collections store - Collection browser state management
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  collectionsState,
  collectionsData,
  selectedCollection,
  selectedCollectionMembers,
  findCollectionById,
  type CollectionsState,
  type CollectionItem,
  type CollectionMember
} from '$lib/stores/collections';
import type { CollectionInfo } from '$lib/services/collection-service';
import type { Node } from '$lib/types';
import { mockCollections, mockMembers } from '../fixtures/collections-fixtures';

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
      const state = get(collectionsState);

      expect(state.selectedCollectionId).toBeNull();
      expect(state.subPanelOpen).toBe(false);
      expect(state.expandedCollectionIds).toBeInstanceOf(Set);
      expect(state.expandedCollectionIds.size).toBe(0);
    });

    it('selectedCollection derived store returns undefined initially', () => {
      const selected = get(selectedCollection);
      expect(selected).toBeUndefined();
    });

    it('selectedCollectionMembers derived store returns empty array initially', () => {
      const members = get(selectedCollectionMembers);
      expect(members).toEqual([]);
    });
  });

  describe('selectCollection', () => {
    it('selects a collection and opens the sub-panel', () => {
      collectionsState.selectCollection('col-1');

      const state = get(collectionsState);
      expect(state.selectedCollectionId).toBe('col-1');
      expect(state.subPanelOpen).toBe(true);
    });

    it('updates selectedCollection derived store', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1');

      const selected = get(selectedCollection);
      expect(selected).toBeDefined();
      expect(selected?.id).toBe('col-1');
      expect(selected?.name).toBe('Project Ideas');
    });

    it('updates selectedCollectionMembers derived store', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1');

      const members = get(selectedCollectionMembers);
      expect(members).toHaveLength(3);
      expect(members[0].name).toBe('AI-Powered Note Taking');
    });

    it('selecting a different collection replaces the selection', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1');
      collectionsState.selectCollection('col-2');

      const state = get(collectionsState);
      expect(state.selectedCollectionId).toBe('col-2');
      expect(state.subPanelOpen).toBe(true);

      const selected = get(selectedCollection);
      expect(selected?.name).toBe('Meeting Notes');
    });

    it('selecting a nested collection works correctly', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1-1');

      const selected = get(selectedCollection);
      expect(selected).toBeDefined();
      expect(selected?.id).toBe('col-1-1');
      expect(selected?.name).toBe('AI Features and Machine Learning Integration');
    });

    it('selecting a deeply nested collection works correctly', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      collectionsState.selectCollection('col-1-1-1');

      const selected = get(selectedCollection);
      expect(selected).toBeDefined();
      expect(selected?.id).toBe('col-1-1-1');
      expect(selected?.name).toBe('Natural Language Processing Research');
    });
  });

  describe('closeSubPanel', () => {
    it('closes the sub-panel but keeps selection', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.closeSubPanel();

      const state = get(collectionsState);
      expect(state.selectedCollectionId).toBe('col-1'); // Keeps selection for visual context
      expect(state.subPanelOpen).toBe(false);
    });

    it('does nothing when called without prior selection', () => {
      collectionsState.closeSubPanel();

      const state = get(collectionsState);
      expect(state.selectedCollectionId).toBeNull();
      expect(state.subPanelOpen).toBe(false);
    });
  });

  describe('clearSelection', () => {
    it('clears selection and closes sub-panel', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.clearSelection();

      const state = get(collectionsState);
      expect(state.selectedCollectionId).toBeNull();
      expect(state.subPanelOpen).toBe(false);
    });

    it('selectedCollection derived store returns undefined after clearing', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.clearSelection();

      const selected = get(selectedCollection);
      expect(selected).toBeUndefined();
    });

    it('selectedCollectionMembers returns empty array after clearing', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.clearSelection();

      const members = get(selectedCollectionMembers);
      expect(members).toEqual([]);
    });
  });

  describe('toggleCollectionExpanded', () => {
    it('expands a collection when collapsed', () => {
      collectionsState.toggleCollectionExpanded('col-1');

      const state = get(collectionsState);
      expect(state.expandedCollectionIds.has('col-1')).toBe(true);
    });

    it('collapses a collection when expanded', () => {
      collectionsState.toggleCollectionExpanded('col-1');
      collectionsState.toggleCollectionExpanded('col-1');

      const state = get(collectionsState);
      expect(state.expandedCollectionIds.has('col-1')).toBe(false);
    });

    it('can expand multiple collections', () => {
      collectionsState.toggleCollectionExpanded('col-1');
      collectionsState.toggleCollectionExpanded('col-2');

      const state = get(collectionsState);
      expect(state.expandedCollectionIds.has('col-1')).toBe(true);
      expect(state.expandedCollectionIds.has('col-2')).toBe(true);
      expect(state.expandedCollectionIds.size).toBe(2);
    });

    it('expanding does not affect selection state', () => {
      collectionsState.selectCollection('col-1');
      collectionsState.toggleCollectionExpanded('col-2');

      const state = get(collectionsState);
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
      let state = get(collectionsState);
      expect(state.selectedCollectionId).toBe('col-1');
      expect(state.subPanelOpen).toBe(true);
      expect(state.expandedCollectionIds.size).toBe(2);

      // Reset
      collectionsState.reset();

      // Verify state is initial
      state = get(collectionsState);
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

      expect(get(selectedCollection)).toBeUndefined();

      collectionsState.selectCollection('col-1');
      expect(get(selectedCollection)?.id).toBe('col-1');

      collectionsState.selectCollection('col-2');
      expect(get(selectedCollection)?.id).toBe('col-2');

      collectionsState.clearSelection();
      expect(get(selectedCollection)).toBeUndefined();
    });

    it('selectedCollectionMembers updates reactively when selection changes', () => {
      // Set up test data
      collectionsData._setTestData(flattenCollections(mockCollections), createTestMembers());

      expect(get(selectedCollectionMembers)).toEqual([]);

      collectionsState.selectCollection('col-1');
      expect(get(selectedCollectionMembers)).toHaveLength(3);

      collectionsState.selectCollection('col-3'); // Empty collection
      expect(get(selectedCollectionMembers)).toEqual([]);

      collectionsState.selectCollection('col-4');
      expect(get(selectedCollectionMembers)).toHaveLength(4);
    });

    it('selectedCollectionMembers returns empty for invalid selection', () => {
      collectionsState.selectCollection('non-existent');

      const members = get(selectedCollectionMembers);
      expect(members).toEqual([]);
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
