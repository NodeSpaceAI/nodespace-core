import { collectionService, type CollectionInfo } from '$lib/services/collection-service';
import type { Node } from '$lib/types';
import { createLogger } from '$lib/utils/logger';
import { onDaemonReconnect } from '$lib/services/daemon-status';
import { stripMarkdown } from '$lib/services/markdown-utils';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';

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
 * 3. Computed getters for selected collection and its members
 *
 * Svelte 5 rune store (ADR-049): state lives on the classes as `$state`;
 * `collectionsTree` / `selectedCollection` / `selectedCollectionMembers` are
 * computed getters, not `derived` stores.
 */

// ============================================================================
// Types
// ============================================================================

export interface CollectionItem {
  id: string;
  name: string;
  memberCount: number;
  children?: CollectionItem[];
  /**
   * True while this collection is an unconfirmed optimistic insert — the local
   * entry shown immediately on create, before the backend write resolves. The
   * sidebar uses it to render the row in a pending style; it clears when the
   * create call returns and the entry is reconciled with its real id.
   */
  pending?: boolean;
}

export interface CollectionMember {
  id: string;
  name: string;
  nodeType: string;
}

/**
 * Node types that are NOT user-authored content and so should never appear in a
 * collection's contents list. `schema`/`person`/`database-settings` are system
 * definition nodes; `collection` nodes are shown in the collection tree itself;
 * `horizontal-line` is a purely decorative structural divider. Everything else
 * (text, header, task, checkbox, code-block, quote-block, ordered-list,
 * ai-chat, query, prompt, skill, date, …) is genuine content and kept.
 */
export const NON_CONTENT_NODE_TYPES: ReadonlySet<string> = new Set([
  'schema',
  'person',
  'database-settings',
  'collection',
  'horizontal-line',
]);

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
  /** Collection members - full Node data fetched in single query */
  members: Map<string, Node[]>;
  loading: boolean;
  error: string | null;
  /**
   * IDs of collections the user created in this session. A brand-new collection
   * has zero members, which `pruneEmptyCollections` would otherwise hide — so
   * these are exempt from the hide-empty filter for the lifetime of the session.
   * Without this the user's own just-created collection vanishes on the very
   * refresh that was meant to reveal it.
   *
   * The exemption is keyed on an id that can outlive the collection it was
   * granted for: backend collection ids are derived from the name, so deleting
   * "Architecture" and later receiving a same-named collection from sync would
   * reuse the exempt id, and renaming one leaves the old id exempt (harmless —
   * nothing matches it) while the new id is not. Both are accepted session-
   * scoped fuzziness in a visibility heuristic, and a reset clears the set.
   */
  locallyCreatedIds: Set<string>;
  /** Subset of `locallyCreatedIds` whose backend create has not resolved yet */
  pendingIds: Set<string>;
}

const initialDataState: CollectionsDataState = {
  collections: [],
  members: new Map(),
  loading: false,
  error: null,
  locallyCreatedIds: new Set(),
  pendingIds: new Set(),
};

class CollectionsDataStore {
  state = $state<CollectionsDataState>({
    ...initialDataState,
    members: new Map(),
    locallyCreatedIds: new Set(),
    pendingIds: new Set(),
  });

  /**
   * True once `loadCollections` has completed at least one successful fetch.
   * Lets consumers distinguish "not fetched yet" (empty because unloaded) from
   * "fetched, genuinely empty" — e.g. the invitations prompt only concludes a
   * signed-in user has no collection access after a real load has resolved,
   * never during the pre-load window.
   */
  hasLoaded = $state(false);

  /**
   * Bumped whenever the store stops representing the data it did — `reset()`
   * and `forgetLocallyCreated()` (the database switch). An in-flight
   * `createCollection` captures this before awaiting and abandons its reconcile
   * if the value changed, so a create issued against the previous database
   * cannot write its exemption or error into the store afterwards.
   */
  #generation = 0;

  /**
   * Transform flat collections into tree structure for UI display.
   * Uses parentCollectionIds to build proper hierarchy. Hides collections with
   * no member nodes visible to the current user, and the root collections that
   * registered extensions contribute (see `buildCollectionsTree`). With no
   * extension contributing roots, no collection is hidden as a root.
   */
  get collectionsTree(): CollectionItem[] {
    return buildCollectionsTree(
      this.state.collections,
      this.state.locallyCreatedIds,
      this.state.pendingIds,
      uiExtensionRegistry.collectionTreeRoots()
    );
  }

  /** Load all collections from backend */
  async loadCollections(): Promise<void> {
    // A reset or database switch landed while this load was in flight: the
    // fetched rows belong to the database we just left, so drop them rather
    // than writing them into a store that now represents a different one
    // (mirrors the same guard already around `createCollection`).
    const generation = this.#generation;
    this.state = { ...this.state, loading: true, error: null };

    try {
      const fetched = await collectionService.getAllCollections();
      if (generation !== this.#generation) {
        log.debug('Discarding collections load that resolved after the store moved on');
        return;
      }
      log.debug('Loaded collections', { count: fetched.length });
      // Preserve optimistic entries the backend has not confirmed yet: a reload
      // that raced an in-flight create must not make the new collection blink
      // out of the sidebar between the optimistic insert and its confirmation.
      const fetchedIds = new Set(fetched.map((c) => c.id));
      const unconfirmed = this.state.collections.filter(
        (c) => this.state.pendingIds.has(c.id) && !fetchedIds.has(c.id)
      );
      this.state = {
        ...this.state,
        collections: [...fetched, ...unconfirmed],
        loading: false,
      };
      this.hasLoaded = true;
    } catch (err) {
      if (generation !== this.#generation) return;
      const message = err instanceof Error ? err.message : 'Failed to load collections';
      log.error('Failed to load collections', { error: message });
      this.state = { ...this.state, loading: false, error: message };
    }
  }

  /**
   * Create a new collection optimistically: the entry is inserted into local
   * state and visible in the sidebar immediately, before the backend write is
   * confirmed — matching the instant-appearance UX node creation already has.
   * The backend derives the collection's real id from its name, so the
   * optimistic entry carries a temporary id that is swapped for the real one
   * when the create call resolves. On failure the entry is rolled back out of
   * local state and the error surfaced.
   *
   * Returns the new collection's id, or null on failure.
   */
  async createCollection(name: string, description?: string): Promise<string | null> {
    const generation = this.#generation;
    const tempId = `pending-collection-${crypto.randomUUID()}`;
    const now = new Date().toISOString();
    const optimistic: CollectionInfo = {
      id: tempId,
      content: name,
      nodeType: 'collection',
      createdAt: now,
      modifiedAt: now,
      version: 1,
      properties: description ? { description } : {},
      memberCount: 0,
      parentCollectionIds: [],
    };

    this.state = {
      ...this.state,
      collections: [...this.state.collections, optimistic],
      locallyCreatedIds: new Set(this.state.locallyCreatedIds).add(tempId),
      pendingIds: new Set(this.state.pendingIds).add(tempId),
      error: null,
    };

    try {
      const id = await collectionService.createCollection(name, description);
      // A backend that reports success without an id has not created anything
      // we can reconcile against — treat it as a failure rather than seeding a
      // permanently-exempt placeholder keyed on an empty id. The browser dev
      // proxy's unimplemented create resolves '' exactly like this.
      if (!id) {
        throw new Error('Collection create returned no id');
      }
      log.debug('Created collection', { id, name });

      // A reset or database switch landed while this create was in flight: the
      // collection belongs to the database we just left, so drop the result
      // rather than writing its exemption into the current store. Reported as
      // a failure — the collection is not in the store the caller can see, so
      // returning its id would have the sidebar treat it as successfully
      // created and shown when neither is true.
      if (generation !== this.#generation) {
        log.debug('Discarding create that resolved after the store moved on', { id, name });
        return null;
      }

      // Reconcile: swap the temporary id for the backend's real one. If a
      // concurrent reload already brought the real row in, drop the placeholder
      // instead of seeding a duplicate.
      const alreadyPresent = this.state.collections.some((c) => c.id === id);
      const collections = alreadyPresent
        ? this.state.collections.filter((c) => c.id !== tempId)
        : this.state.collections.map((c) => (c.id === tempId ? { ...c, id } : c));

      const locallyCreatedIds = new Set(this.state.locallyCreatedIds);
      locallyCreatedIds.delete(tempId);
      locallyCreatedIds.add(id);

      const pendingIds = new Set(this.state.pendingIds);
      pendingIds.delete(tempId);

      this.state = { ...this.state, collections, locallyCreatedIds, pendingIds };
      return id;
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to create collection';
      log.error('Failed to create collection', { name, error: message });

      // As above: a create that fails after a reset must not leave its error
      // on a store that now represents a different database.
      if (generation !== this.#generation) {
        return null;
      }

      const locallyCreatedIds = new Set(this.state.locallyCreatedIds);
      locallyCreatedIds.delete(tempId);
      const pendingIds = new Set(this.state.pendingIds);
      pendingIds.delete(tempId);

      this.state = {
        ...this.state,
        collections: this.state.collections.filter((c) => c.id !== tempId),
        locallyCreatedIds,
        pendingIds,
        error: message,
      };
      return null;
    }
  }

  /** Load members for a specific collection (single query for full Node data) */
  async loadMembers(collectionId: string): Promise<void> {
    try {
      const members = await collectionService.getCollectionMembers(collectionId);
      log.debug('Loaded collection members', { collectionId, count: members.length });

      const newMembers = new Map(this.state.members);
      newMembers.set(collectionId, members);
      this.state = { ...this.state, members: newMembers };
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to load members';
      log.error('Failed to load collection members', { collectionId, error: message });
    }
  }

  /** Invalidate cached members for a collection (triggers reload on next access) */
  invalidateMembers(collectionId: string): void {
    const newMembers = new Map(this.state.members);
    newMembers.delete(collectionId);
    this.state = { ...this.state, members: newMembers };
  }

  /** Invalidate all cached members (e.g., on node update that might affect any collection) */
  invalidateAllMembers(): void {
    this.state = { ...this.state, members: new Map() };
  }

  /** Get cached members for a collection */
  getMembers(collectionId: string): Node[] {
    return this.state.members.get(collectionId) ?? [];
  }

  /** Get a collection by ID from cached data */
  getCollectionById(collectionId: string): CollectionInfo | undefined {
    return this.state.collections.find((c) => c.id === collectionId);
  }

  /**
   * Drop the hide-empty exemptions without clearing the rest of the cache.
   *
   * Called when switching databases. The exemptions are per-database: backend
   * collection ids are derived from the name, so the id granted to a locally
   * created "Architecture" is the *same* id "Architecture" would have in the
   * newly-selected database. Carrying the set across the switch would wrongly
   * un-hide a same-named empty collection there. Also invalidates any in-flight
   * create, whose result belongs to the database being left.
   */
  forgetLocallyCreated(): void {
    this.#generation++;
    this.state = {
      ...this.state,
      locallyCreatedIds: new Set(),
      pendingIds: new Set(),
    };
  }

  /** Clear all cached data */
  reset(): void {
    // Invalidate any in-flight create so its resolution cannot write into the
    // state this reset is establishing.
    this.#generation++;
    this.state = {
      ...initialDataState,
      members: new Map(),
      locallyCreatedIds: new Set(),
      pendingIds: new Set(),
    };
    this.hasLoaded = false;
  }

  /** Set test data directly (for testing purposes only) */
  _setTestData(
    collections: CollectionInfo[],
    members: Map<string, Node[]>,
    locallyCreatedIds: Set<string> = new Set()
  ): void {
    this.state = {
      collections,
      members,
      loading: false,
      error: null,
      locallyCreatedIds,
      pendingIds: new Set(),
    };
    this.hasLoaded = true;
  }
}

export const collectionsData = new CollectionsDataStore();

// Registered once at module load (this file is a singleton — ES modules only
// evaluate once), not per component mount. Retries loadCollections whenever
// the daemon becomes reachable, so a load that failed while the daemon was
// still starting up recovers automatically without a manual reload.
onDaemonReconnect(() => collectionsData.loadCollections());

// ============================================================================
// Collections UI State Store
// ============================================================================

const initialState: CollectionsState = {
  selectedCollectionId: null,
  subPanelOpen: false,
  expandedCollectionIds: new Set(),
};

class CollectionsStore {
  state = $state<CollectionsState>({
    ...initialState,
    expandedCollectionIds: new Set(),
  });

  /** The currently selected collection, normalized for UI display */
  get selectedCollection(): SelectedCollectionInfo | undefined {
    if (!this.state.selectedCollectionId) return undefined;

    const collection = collectionsData.state.collections.find(
      (c) => c.id === this.state.selectedCollectionId
    );
    if (collection) {
      return {
        id: collection.id,
        name: collection.content, // CollectionInfo uses content for name
        content: collection.content,
        memberCount: collection.memberCount,
      };
    }

    return undefined;
  }

  /** Members of the currently selected collection as CollectionMember format */
  get selectedCollectionMembers(): CollectionMember[] {
    if (!this.state.selectedCollectionId) return [];

    const members = collectionsData.state.members.get(this.state.selectedCollectionId);
    if (members && members.length > 0) {
      return (
        members
          // Keep user-authored content only; drop system/definition/decorative
          // nodes (schema, person, database-settings, collection, horizontal-line).
          .filter((node) => !NON_CONTENT_NODE_TYPES.has(node.nodeType))
          .map((node) => ({
            id: node.id,
            // pluginRegistry.resolveDisplayTitle already picks title vs. content correctly
            // (title only for title_template-driven schemas — a plain node.title fallback is
            // stale for everything else, since it's only refreshed by a backend round-trip).
            // stripMarkdown is passed in as the content formatter, not wrapped around the
            // whole call, so it cleans an imported header root ("# ACP Integration
            // Architecture" -> "ACP Integration Architecture") WITHOUT also running over a
            // title_template-computed name, which is a property value, not markdown — a name
            // containing an underscore or asterisk would otherwise get silently mangled.
            name: pluginRegistry.resolveDisplayTitle(node, stripMarkdown),
            nodeType: node.nodeType,
          }))
      );
    }

    return [];
  }

  /** Select a collection and open the sub-panel */
  async selectCollection(collectionId: string): Promise<void> {
    this.state = {
      ...this.state,
      selectedCollectionId: collectionId,
      subPanelOpen: true,
    };

    // Load members when selecting
    await collectionsData.loadMembers(collectionId);
  }

  /** Close the sub-panel */
  closeSubPanel(): void {
    this.state = {
      ...this.state,
      subPanelOpen: false,
      // Keep selectedCollectionId for visual context in the list
    };
  }

  /** Clear selection and close sub-panel */
  clearSelection(): void {
    this.state = {
      ...this.state,
      selectedCollectionId: null,
      subPanelOpen: false,
    };
  }

  /** Toggle a collection's expanded state in the tree */
  toggleCollectionExpanded(collectionId: string): void {
    const newExpanded = new Set(this.state.expandedCollectionIds);
    if (newExpanded.has(collectionId)) {
      newExpanded.delete(collectionId);
    } else {
      newExpanded.add(collectionId);
    }
    this.state = { ...this.state, expandedCollectionIds: newExpanded };
  }

  /** Reset to initial state */
  reset(): void {
    this.state = { ...initialState, expandedCollectionIds: new Set() };
  }
}

export const collectionsState = new CollectionsStore();

// ============================================================================
// Tree building & helpers
// ============================================================================

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

/**
 * Recursively prune collections that have no member nodes visible to the
 * current user. A collection is kept if it has its own members OR any of its
 * descendants does — so a populated collection nested under an empty parent
 * still surfaces. Returns a new array of new items (pure — the input items are
 * not mutated).
 *
 * `memberCount` is sourced from the local per-user store, so it already
 * reflects RBAC visibility: the local DB only holds member edges the signed-in
 * user can see. This filter therefore hides collections that are empty *for
 * this user*, not just globally empty ones.
 *
 * Collections in `exempt` are always kept regardless of member count: these are
 * the ones the user created in this session, and hiding a just-created
 * collection because it is still empty is the bug this exemption exists to
 * prevent, not the RBAC-emptiness case the filter is for.
 */
function pruneEmptyCollections(
  items: CollectionItem[],
  exempt: ReadonlySet<string>
): CollectionItem[] {
  return items.reduce<CollectionItem[]>((kept, item) => {
    const keptChildren = item.children ? pruneEmptyCollections(item.children, exempt) : [];
    if (item.memberCount > 0 || keptChildren.length > 0 || exempt.has(item.id)) {
      kept.push({ ...item, children: keptChildren });
    }
    return kept;
  }, []);
}

/**
 * Transform flat collections into tree structure for UI display.
 * Uses parentCollectionIds to build proper hierarchy.
 *
 * `locallyCreatedIds` are exempt from the hide-empty filter (see
 * `pruneEmptyCollections`); `pendingIds` additionally mark entries whose
 * backend create is still in flight, so the sidebar can style them as pending.
 *
 * `rootCollectionIds` are root collections: containers the tree does not show.
 * A root's own row is never shown, and it is never a display parent, so a
 * collection whose only parents are roots shows at the top level. A collection
 * with a parent that is not a root still nests under that parent. The set
 * defaults to empty, and with no roots no collection is hidden as a root.
 * Exported for unit testing of the pure tree logic.
 */
export function buildCollectionsTree(
  collections: CollectionInfo[],
  locallyCreatedIds: ReadonlySet<string> = new Set(),
  pendingIds: ReadonlySet<string> = new Set(),
  rootCollectionIds: ReadonlySet<string> = new Set()
): CollectionItem[] {
  // Build a map of id -> CollectionItem for quick lookup
  const itemMap = new Map<string, CollectionItem>();
  for (const c of collections) {
    itemMap.set(c.id, {
      id: c.id,
      name: c.content, // Collection name is stored in content field
      memberCount: c.memberCount,
      children: [],
      pending: pendingIds.has(c.id),
    });
  }

  // Build a set of all IDs that are children of some other collection
  const childIds = new Set<string>();

  // For each collection, add it as a child to its parent(s)
  // Note: A collection can have multiple parents, but for tree display
  // we only show it under the first parent to avoid duplication
  for (const c of collections) {
    // Root collections are never display parents: a collection whose only
    // parents are roots shows at the top level instead of nesting under one.
    const parentIds = (c.parentCollectionIds || []).filter((p) => !rootCollectionIds.has(p));
    if (parentIds.length > 0) {
      // Add to first parent only (to avoid showing same collection multiple times)
      const firstParentId = parentIds[0];
      const parent = itemMap.get(firstParentId);
      const child = itemMap.get(c.id);
      if (parent && child) {
        parent.children = parent.children || [];
        parent.children.push(child);
        childIds.add(c.id);
      }
    }
  }

  // Sort children alphabetically within each parent
  for (const item of itemMap.values()) {
    if (item.children && item.children.length > 0) {
      item.children.sort((a, b) => a.name.localeCompare(b.name));
    }
  }

  // Return only top-level collections (those not nested above), and never a
  // root collection itself. Filtering a root as a parent (above) un-nests its
  // members; dropping it here hides its own row, which would otherwise show at
  // the top level whenever it has direct content members. With no roots, no
  // collection is dropped as a root.
  const topLevel = collections
    .filter((c) => !rootCollectionIds.has(c.id) && !childIds.has(c.id))
    .map((c) => itemMap.get(c.id)!)
    .sort((a, b) => a.name.localeCompare(b.name));

  // Hide collections with no member nodes visible to the current user. Keep a
  // collection whenever it — or any descendant — has members, so populated
  // sub-collections under an empty parent remain reachable, and always keep the
  // ones this user just created even though they start empty.
  return pruneEmptyCollections(topLevel, locallyCreatedIds);
}

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
