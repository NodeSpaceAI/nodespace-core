/**
 * Saved Queries Store
 *
 * Global reactive list of saved query nodes, grouped by the node type they
 * target, for the "Types and Views" section of the navigation sidebar.
 *
 * Follows the `schemasStore` / `aiChatsData` pattern: a `load*` method called
 * from the sidebar's `onMount`, refreshed by the debounced
 * `scheduleSavedQueryRefresh` wired to node domain events, reloaded after a
 * daemon reconnect, and guarded by a database-switch generation counter.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import { createLogger } from '$lib/utils/logger';
import { onDaemonReconnect } from '$lib/services/daemon-status';
import { nodeToQueryNode } from '$lib/types/query';

const log = createLogger('SavedQueriesStore');

/** Target value meaning "every node type" — not attached to any single type. */
const ALL_TYPES = '*';

/**
 * Ids of the saved queries core seeds in every database
 * (`CORE_QUERY_IDS` in packages/core/src/services/query_service/core_queries.rs):
 * Specs/Plans/Decisions by status, Ready tasks, In progress, Awaiting review,
 * Changes requested, Ready to merge, Needs shaping, Waiting on you.
 *
 * They are agent helpers, not views a person browses: rules call them by name
 * (`run_query "Ready tasks"`) and built-in skills are `attached_to` them. They
 * therefore stay ordinary query nodes with these ids and names, and are only
 * left out of the sidebar's list so they do not show up nested under Task,
 * Issue, Spec, Plan and Decision as if the user had saved them. A query the
 * user creates (or duplicates from one of these) has a different id and is
 * listed as usual.
 *
 * TO SHOW THEM AGAIN: empty this set (or drop single ids from it). Nothing else
 * changes; the queries still exist, run and sync as before. A test
 * (saved-queries.test.ts) keeps this list equal to the Rust `CORE_QUERY_IDS`.
 */
export const SEEDED_AGENT_QUERY_IDS: ReadonlySet<string> = new Set([
  '3b9c304f-f480-4f1d-8653-5876bb005f7c', // Specs by status
  'ae79002e-913c-448d-8cc1-95912f2ab2f8', // Plans by status
  'd0200c59-7df9-4bc0-bc78-b06d205b085c', // Decisions by status
  '138b68c6-9a67-4fed-8fa5-993953158f59', // Ready tasks
  'd1e5c9a8-d19c-401b-9277-40fd873fb205', // In progress
  '3eff80e8-9672-4896-ac0b-76a169062511', // Awaiting review
  '6a1f0c52-8b47-4d3e-9a15-c2e07d4b8f13', // Changes requested
  'e47b9d20-35a6-4c81-b0f2-91d8a65c3e74', // Ready to merge
  'bb611c45-2b82-47dd-ad53-560ec9b5447f', // Needs shaping
  '62d6346f-0d71-4f36-bab3-94c2f98ed8ba' // Waiting on you
]);

export interface SavedQueryListItem {
  id: string;
  /** The query's name (node content). */
  name: string;
  /** Node type the query targets. */
  targetType: string;
}

/** Case-insensitive name order, then id, so equal names keep a stable order. */
function compareQueries(a: SavedQueryListItem, b: SavedQueryListItem): number {
  return a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }) || a.id.localeCompare(b.id);
}

class SavedQueriesStore {
  /** Saved queries that target one specific type. */
  queries = $state<SavedQueryListItem[]>([]);

  /** Queries grouped by target type, each group ordered by name. Recomputed only when `queries` changes. */
  #byType = $derived.by(() => {
    const groups = new Map<string, SavedQueryListItem[]>();
    for (const query of [...this.queries].sort(compareQueries)) {
      const group = groups.get(query.targetType);
      if (group) group.push(query);
      else groups.set(query.targetType, [query]);
    }
    return groups;
  });

  /** See `SchemasStore.#generation`. */
  #generation = 0;

  /** Saved queries targeting `typeId`, ordered by name. */
  forType(typeId: string): SavedQueryListItem[] {
    return this.#byType.get(typeId) ?? [];
  }

  /** True when `nodeId` is a query currently listed (used to react to renames and deletes). */
  has(nodeId: string): boolean {
    return this.queries.some((q) => q.id === nodeId);
  }

  /** Load all saved query nodes from the backend. */
  async loadSavedQueries(): Promise<void> {
    const generation = this.#generation;
    try {
      const nodes = await backendAdapter.queryNodes({ nodeType: 'query' });
      if (generation !== this.#generation) {
        log.debug('Discarding saved queries load that resolved after the store moved on');
        return;
      }
      this.queries = nodes
        .map((node) => {
          const query = nodeToQueryNode(node);
          return { id: query.id, name: query.content, targetType: query.targetType };
        })
        .filter((q) => q.targetType !== ALL_TYPES && !SEEDED_AGENT_QUERY_IDS.has(q.id));
      log.debug('Saved queries loaded', { count: this.queries.length });
    } catch (err) {
      log.error('Failed to load saved queries', err);
    }
  }

  /** Invalidate any load issued against a database this store no longer represents. */
  invalidateForDatabaseSwitch(): void {
    this.#generation++;
  }

  /** Reset to empty (test use only). */
  reset(): void {
    this.#generation++;
    this.queries = [];
  }
}

export const savedQueriesData = new SavedQueriesStore();

onDaemonReconnect(() => savedQueriesData.loadSavedQueries());
