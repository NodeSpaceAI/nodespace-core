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
import { nearestCoreType, setTypeResolver } from '$lib/types/core-node-types';

const log = createLogger('SchemasStore');

// Core schema IDs that are user-queryable and should appear in the sidenav.
// Structural/inline types (text, date, header, code-block, etc.) are excluded —
// they are node content primitives, not entity types users browse or filter.
const SIDENAV_CORE_TYPES = new Set([
  'task',
  'issue',
  'project',
  'software-project',
  'spec',
  'plan',
  'decision',
  'skill',
  'person',
  'agent-guidance'
]);

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

  /** User-created custom schemas, minus subtypes of non-browsable core types. */
  get customSchemas(): SchemaNode[] {
    return this.schemas.filter((s) => !s.isCore && !this.#extendsHiddenBase(s));
  }

  /**
   * Whether a non-core schema is an extension-owned system subtype that the
   * types list must not show.
   *
   * HOW THE RULE WORKS: the types list shows only types a user can create and
   * browse. A schema is hidden BY WHAT IT EXTENDS: walk its `extends` chain to
   * the nearest core type (`nearestCoreType`); if that core type is not in
   * `SIDENAV_CORE_TYPES` (the browsable allowlist above), the schema is hidden.
   * Example: a subtype of `database-settings` that an extension registers to
   * hold per-database state. The singleton is retyped to it in place, and a
   * user never creates or browses it. A type with no `extends`, or one whose
   * chain reaches a browsable core type (`task`, `person`, ...), is shown.
   *
   * WHY THIS AND NOT A DECLARED FLAG: a `hidden`/`system` field on the schema
   * envelope would be more explicit, but a schema-field change follows the
   * node-type sequence (generated types, storage, MCP/CLI params, probably an
   * ADR) and every extension would have to remember to set it. Deriving it from
   * `extends` needs no new field, keeps core free of any extension's naming,
   * and also covers other extension-owned subtypes of system types.
   *
   * KNOWN RISK: a type a USER creates that extends a non-browsable core type
   * (for example a custom subtype of `database-settings`, `collection` or
   * `text`) is hidden from the list too, because the rule cannot tell it from
   * an extension's. It still exists and its nodes still work; it just has no
   * sidebar row.
   *
   * TO MAKE A HIDDEN TYPE VISIBLE AGAIN: add its nearest core base type's id to
   * `SIDENAV_CORE_TYPES` (this also lists that core type itself in
   * `builtInSchemas`), or, to show one subtype only, replace this rule with an
   * explicit allow/deny list or a declared schema flag.
   */
  #extendsHiddenBase(schema: SchemaNode): boolean {
    const base = nearestCoreType(schema.id);
    return base !== undefined && !SIDENAV_CORE_TYPES.has(base);
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
