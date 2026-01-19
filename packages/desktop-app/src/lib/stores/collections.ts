import { writable, derived } from 'svelte/store';

/**
 * Collections Store
 *
 * Manages the state for the Collections browser in the navigation sidebar.
 * Collections are AI-managed groupings of related nodes that span across the hierarchy.
 */

// Types
export interface CollectionItem {
  id: string;
  name: string;
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

// Initial state
// Note: `expanded` (whether Collections section is open) is managed by layoutState for persistence
const initialState: CollectionsState = {
  selectedCollectionId: null,
  subPanelOpen: false,
  expandedCollectionIds: new Set()
};

// Create the main store
function createCollectionsStore() {
  const { subscribe, set, update } = writable<CollectionsState>(initialState);

  return {
    subscribe,
    set,
    update,

    // Note: toggleExpanded/setExpanded moved to layoutState for persistence

    /** Select a collection and open the sub-panel */
    selectCollection: (collectionId: string) => {
      update((state) => ({
        ...state,
        selectedCollectionId: collectionId,
        subPanelOpen: true
      }));
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

// Mock data for visual prototype - 3 levels deep
// This will be replaced with real data from the backend
export const mockCollections: CollectionItem[] = [
  {
    id: 'col-1',
    name: 'Project Ideas',
    children: [
      {
        id: 'col-1-1',
        name: 'AI Features and Machine Learning Integration',
        children: [
          { id: 'col-1-1-1', name: 'Natural Language Processing Research' },
          { id: 'col-1-1-2', name: 'Vector Embeddings and Semantic Search' }
        ]
      },
      { id: 'col-1-2', name: 'UI Improvements' }
    ]
  },
  {
    id: 'col-2',
    name: 'Meeting Notes',
    children: [
      { id: 'col-2-1', name: '2025 Q1' },
      {
        id: 'col-2-2',
        name: '2024 Q4',
        children: [
          { id: 'col-2-2-1', name: 'Sprint Reviews' },
          { id: 'col-2-2-2', name: 'Retrospectives' }
        ]
      }
    ]
  },
  { id: 'col-3', name: 'Research Papers' },
  { id: 'col-4', name: 'Reading List' }
];

// Mock member data for all collections (including nested)
// This will be replaced with real data from the backend
export const mockMembers: Record<string, CollectionMember[]> = {
  'col-1': [
    { id: 'node-1', name: 'AI-Powered Note Taking', nodeType: 'text' },
    { id: 'node-2', name: 'Voice Interface Design', nodeType: 'text' },
    { id: 'node-3', name: 'Graph Visualization', nodeType: 'text' }
  ],
  'col-1-1': [
    { id: 'node-10', name: 'GPT Integration Ideas', nodeType: 'text' },
    { id: 'node-11', name: 'Local LLM Research', nodeType: 'text' }
  ],
  'col-1-1-1': [{ id: 'node-12', name: 'Prompt Engineering Notes', nodeType: 'text' }],
  'col-1-1-2': [{ id: 'node-13', name: 'Vector DB Comparison', nodeType: 'text' }],
  'col-1-2': [{ id: 'node-14', name: 'Dark Mode Implementation', nodeType: 'task' }],
  'col-2': [
    { id: 'node-4', name: 'Team Standup 2025-01-15', nodeType: 'date' },
    { id: 'node-5', name: 'Sprint Planning', nodeType: 'text' }
  ],
  'col-2-1': [{ id: 'node-15', name: 'January Kickoff', nodeType: 'date' }],
  'col-2-2': [{ id: 'node-16', name: 'Q4 Summary', nodeType: 'text' }],
  'col-2-2-1': [{ id: 'node-17', name: 'Sprint 24 Review', nodeType: 'text' }],
  'col-2-2-2': [{ id: 'node-18', name: 'Team Improvements', nodeType: 'text' }],
  'col-3': [], // Empty collection
  'col-4': [
    { id: 'node-6', name: 'Designing Data-Intensive Apps', nodeType: 'text' },
    { id: 'node-7', name: 'Clean Architecture', nodeType: 'text' },
    { id: 'node-8', name: 'Review chapter 5', nodeType: 'task' },
    { id: 'node-9', name: 'Domain-Driven Design', nodeType: 'text' }
  ]
};

// Helper function to find a collection by ID in the tree
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

// Derived store for the currently selected collection
export const selectedCollection = derived(collectionsState, ($state) =>
  $state.selectedCollectionId ? findCollectionById(mockCollections, $state.selectedCollectionId) : undefined
);

// Derived store for the members of the currently selected collection
export const selectedCollectionMembers = derived(collectionsState, ($state) =>
  $state.selectedCollectionId ? (mockMembers[$state.selectedCollectionId] ?? []) : []
);
