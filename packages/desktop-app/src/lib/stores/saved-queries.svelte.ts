/**
 * Saved Queries Store
 *
 * Global reactive list of saved query nodes, grouped by the node type they
 * target, for the "Node Types" section of the navigation sidebar.
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

  /** See `SchemasStore.#generation`. */
  #generation = 0;

  /** Saved queries targeting `typeId`, ordered by name. */
  forType(typeId: string): SavedQueryListItem[] {
    return this.queries.filter((q) => q.targetType === typeId).sort(compareQueries);
  }

  /** True when `nodeId` is a query currently listed (used to react to deletes). */
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
        .filter((q) => q.targetType !== ALL_TYPES);
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
