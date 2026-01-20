import { writable, derived } from 'svelte/store';
import { collectionService, type CollectionInfo } from '$lib/services/collection-service';
import type { Node } from '$lib/types';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('CollectionsStore');

/**
 * Collections Store
 *
 * Manages the state for the Collections browser in the navigation sidebar.
 * Collections are AI-managed groupings of related nodes that span across the hierarchy.
 *
 * ## Architecture
 *
 * - Collections are flat (globally unique names, no parent-child hierarchy)
 * - Paths like "hr:policy:vacation" are navigation conventions, not structure
 * - Nodes can belong to multiple collections (many-to-many via member_of edges)
 *
 * ## Data Flow
 *
 * 1. `collectionsData` - Reactive store with all collections from backend
 * 2. `collectionsState` - UI state (selection, sub-panel, expanded items)
 * 3. Derived stores for selected collection and its members
 */

// ============================================================================
// Types
// ============================================================================

export interface CollectionItem {
  id: string;
  name: string;
  memberCount: number;
  children?: CollectionItem[];
}

export interface CollectionMember {
  id: string;
  name: string;
  nodeType: string;
}

export interface CollectionsState {
  /** Currently selected collection ID (for sub-panel display) */
  selectedCollectionId: string | null;
  /** Whether the sub-panel is open */
  subPanelOpen: boolean;
  /** Set of expanded collection IDs (for nested tree state) */
  expandedCollectionIds: Set<string>;
}

// ============================================================================
// Collections Data Store (from backend)
// ============================================================================

interface CollectionsDataState {
  collections: CollectionInfo[];
  members: Map<string, Node[]>;
  loading: boolean;
  error: string | null;
}

const initialDataState: CollectionsDataState = {
  collections: [],
  members: new Map(),
  loading: false,
  error: null
};

function createCollectionsDataStore() {
  const { subscribe, set, update } = writable<CollectionsDataState>(initialDataState);

  return {
    subscribe,

    /** Load all collections from backend */
    loadCollections: async () => {
      update((state) => ({ ...state, loading: true, error: null }));

      try {
        const collections = await collectionService.getAllCollections();
        log.debug('Loaded collections', { count: collections.length });

        update((state) => ({
          ...state,
          collections,
          loading: false
        }));
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to load collections';
        log.error('Failed to load collections', { error: message });
        update((state) => ({
          ...state,
          loading: false,
          error: message
        }));
      }
    },

    /** Load members for a specific collection */
    loadMembers: async (collectionId: string) => {
      try {
        const members = await collectionService.getCollectionMembers(collectionId);
        log.debug('Loaded collection members', { collectionId, count: members.length });

        update((state) => {
          const newMembers = new Map(state.members);
          newMembers.set(collectionId, members);
          return { ...state, members: newMembers };
        });
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to load members';
        log.error('Failed to load collection members', { collectionId, error: message });
      }
    },

    /** Get cached members for a collection */
    getMembers: (collectionId: string): Node[] => {
      let result: Node[] = [];
      const unsubscribe = subscribe((state) => {
        result = state.members.get(collectionId) ?? [];
      });
      unsubscribe();
      return result;
    },

    /** Get a collection by ID from cached data */
    getCollectionById: (collectionId: string): CollectionInfo | undefined => {
      let result: CollectionInfo | undefined;
      const unsubscribe = subscribe((state) => {
        result = state.collections.find((c) => c.id === collectionId);
      });
      unsubscribe();
      return result;
    },

    /** Clear all cached data */
    reset: () => {
      set(initialDataState);
    },

    /** Set test data directly (for testing purposes only) */
    _setTestData: (collections: CollectionInfo[], members: Map<string, Node[]>) => {
      set({
        collections,
        members,
        loading: false,
        error: null
      });
    }
  };
}

export const collectionsData = createCollectionsDataStore();

// ============================================================================
// Collections UI State Store
// ============================================================================

const initialState: CollectionsState = {
  selectedCollectionId: null,
  subPanelOpen: false,
  expandedCollectionIds: new Set()
};

function createCollectionsStore() {
  const { subscribe, set, update } = writable<CollectionsState>(initialState);

  return {
    subscribe,
    set,
    update,

    /** Select a collection and open the sub-panel */
    selectCollection: async (collectionId: string) => {
      update((state) => ({
        ...state,
        selectedCollectionId: collectionId,
        subPanelOpen: true
      }));

      // Load members when selecting
      await collectionsData.loadMembers(collectionId);
    },

    /** Close the sub-panel */
    closeSubPanel: () => {
      update((state) => ({
        ...state,
        subPanelOpen: false
        // Keep selectedCollectionId for visual context in the list
      }));
    },

    /** Clear selection and close sub-panel */
    clearSelection: () => {
      update((state) => ({
        ...state,
        selectedCollectionId: null,
        subPanelOpen: false
      }));
    },

    /** Toggle a collection's expanded state in the tree */
    toggleCollectionExpanded: (collectionId: string) => {
      update((state) => {
        const newExpanded = new Set(state.expandedCollectionIds);
        if (newExpanded.has(collectionId)) {
          newExpanded.delete(collectionId);
        } else {
          newExpanded.add(collectionId);
        }
        return {
          ...state,
          expandedCollectionIds: newExpanded
        };
      });
    },

    /** Reset to initial state */
    reset: () => {
      set(initialState);
    }
  };
}

export const collectionsState = createCollectionsStore();

// ============================================================================
// Derived Stores
// ============================================================================

/**
 * Transform flat collections into tree structure for UI display
 * Collections are flat in the database, but we display them in a tree
 * organized by path segments (e.g., "hr:policy" creates hr -> policy tree)
 */
export const collectionsTree = derived(collectionsData, ($data): CollectionItem[] => {
  // For now, display as a flat list until path-based hierarchy is needed
  // The backend stores collections as flat, so this matches the data model
  return $data.collections.map((c) => ({
    id: c.id,
    name: c.content, // Collection name is stored in content field
    memberCount: c.memberCount
  }));
});

/**
 * Collection with unified name field for UI display
 * Normalizes both CollectionInfo (with content) and CollectionItem (with name)
 */
export interface SelectedCollectionInfo {
  id: string;
  name: string;
  memberCount?: number;
  content?: string;
}

/** Get the currently selected collection */
export const selectedCollection = derived(
  [collectionsState, collectionsData],
  ([$state, $data]): SelectedCollectionInfo | undefined => {
    if (!$state.selectedCollectionId) return undefined;

    const collection = $data.collections.find((c) => c.id === $state.selectedCollectionId);
    if (collection) {
      return {
        id: collection.id,
        name: collection.content, // CollectionInfo uses content for name
        content: collection.content,
        memberCount: collection.memberCount
      };
    }

    return undefined;
  }
);

/** Get members of the currently selected collection as CollectionMember format */
export const selectedCollectionMembers = derived(
  [collectionsState, collectionsData],
  ([$state, $data]): CollectionMember[] => {
    if (!$state.selectedCollectionId) return [];

    const members = $data.members.get($state.selectedCollectionId);
    if (members && members.length > 0) {
      return members.map((node) => ({
        id: node.id,
        name: node.content,
        nodeType: node.nodeType
      }));
    }

    return [];
  }
);

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Find a collection by ID in the tree structure
 */
export function findCollectionById(
  collections: CollectionItem[],
  id: string
): CollectionItem | undefined {
  for (const col of collections) {
    if (col.id === id) return col;
    if (col.children) {
      const found = findCollectionById(col.children, id);
      if (found) return found;
    }
  }
  return undefined;
}

