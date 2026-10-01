/**
 * Schemas Store
 *
 * Global reactive store for schema definitions.
 * Used by navigation-sidebar and any other UI that needs the schema list.
 *
 * Mirrors the collectionsData pattern so schema changes from MCP/external
 * sources are reflected in the sidebar without requiring a page refresh.
 *
 * Svelte 5 rune store (ADR-049): the raw list lives on the class as `$state`;
 * `builtInSchemas` / `customSchemas` are computed getters, not `derived` stores.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import { createLogger } from '$lib/utils/logger';
import { onDaemonReconnect } from '$lib/services/daemon-status';
import type { SchemaNode } from '$lib/types/schema-node';
import { setTypeResolver } from '$lib/types/core-node-types';

const log = createLogger('SchemasStore');

// Core schema IDs that are user-queryable and should appear in the sidenav.
// Structural/inline types (text, date, header, code-block, etc.) are excluded —
// they are node content primitives, not entity types users browse or filter.
const SIDENAV_CORE_TYPES = new Set(['task', 'project', 'skill', 'person', 'agent-guidance']);

class SchemasStore {
  /** Raw schema list */
  schemas = $state<SchemaNode[]>([]);

  /**
   * Bumped whenever the store stops representing the database it did —
   * `invalidateForDatabaseSwitch()`. An in-flight `loadSchemas` captures this
   * before awaiting and drops its result if the value changed, so a load
   * issued against the previous database cannot write its rows into the
   * store representing the newly-active one (mirrors
   * `collectionsData.forgetLocallyCreated()` / `aiChatsData`'s
   * `#generation` guard around `loadCollections`/`loadAiChats`).
   *
   * Deliberately a store-local counter rather than reading
   * `sharedNodeStore.currentEpoch()`: this store must stay import-light (it
   * is loaded in lightweight, non-Tauri test contexts — e.g. the daemon
   * readiness e2e harness — that never construct the full shared-node-store/
   * reactive-structure-tree machinery).
   */
  #generation = 0;

  /** Core schemas shown in sidenav (only user-queryable ones like "task") */
  get builtInSchemas(): SchemaNode[] {
    return this.schemas.filter((s) => s.isCore && SIDENAV_CORE_TYPES.has(s.id));
  }

  /** User-created custom schemas */
  get customSchemas(): SchemaNode[] {
    return this.schemas.filter((s) => !s.isCore);
  }

  /** Load all schemas from the backend and update the store. */
  async loadSchemas(): Promise<void> {
    // A database switch landed while this load was in flight: the fetched
    // rows belong to the database we just left, so drop them rather than
    // writing them into a store that now represents a different database.
    const generation = this.#generation;
    try {
      const schemas = await backendAdapter.getAllSchemas();
      if (generation !== this.#generation) {
        log.debug('Discarding schemas load that resolved after the store moved on');
        return;
      }
      this.schemas = schemas;
      log.debug('Schemas loaded', { count: schemas.length });
    } catch (err) {
      log.error('Failed to load schemas', err);
    }
  }

  /**
   * Invalidate any load issued against a database this store no longer
   * represents. Call before reloading for a newly-active database (mirrors
   * `collectionsData.forgetLocallyCreated()` / `aiChatsData
   * .invalidateForDatabaseSwitch()`); `loadSchemas` itself overwrites
   * `schemas` wholesale so no separate list reset is needed.
   */
  invalidateForDatabaseSwitch(): void {
    this.#generation++;
  }
}

export const schemasStore = new SchemasStore();

// The loaded schemas are where a user-defined type's `extends` target is known.
// Reading `schemas` inside the resolver makes a derived value that asks `isA`
// re-evaluate when the schemas load or change.
setTypeResolver((nodeType) => schemasStore.schemas.find((s) => s.id === nodeType));

/** Load all schemas from the backend and update the store. */
export const loadSchemas = (): Promise<void> => schemasStore.loadSchemas();

export const schemasData = {
  loadSchemas,
  invalidateForDatabaseSwitch: (): void => schemasStore.invalidateForDatabaseSwitch()
};

// Registered once at module load (this file is a singleton — ES modules only
// evaluate once), not per component mount. Retries loadSchemas whenever the
// daemon becomes reachable, so a load that failed while the daemon was still
// starting up recovers automatically without a manual reload.
onDaemonReconnect(() => schemasStore.loadSchemas());
