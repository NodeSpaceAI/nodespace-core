/**
 * Schemas Store
 *
 * Global reactive store for schema definitions.
 * Used by navigation-sidebar and any other UI that needs the schema list.
 *
 * Mirrors the collectionsData pattern so schema changes from MCP/external
 * sources are reflected in the sidebar without requiring a page refresh.
 */

import { writable, derived } from 'svelte/store';
import { backendAdapter } from '$lib/services/backend-adapter';
import { createLogger } from '$lib/utils/logger';
import type { SchemaNode } from '$lib/types/schema-node';

const log = createLogger('SchemasStore');

// Raw schema list
const _schemas = writable<SchemaNode[]>([]);

/**
 * Load all schemas from the backend and update the store.
 */
async function loadSchemas(): Promise<void> {
  try {
    const schemas = await backendAdapter.getAllSchemas();
    _schemas.set(schemas);
    log.debug('Schemas loaded', { count: schemas.length });
  } catch (err) {
    log.error('Failed to load schemas', err);
  }
}

// Core schema IDs that are user-queryable and should appear in the sidenav.
// Structural/inline types (text, date, header, code-block, etc.) are excluded —
// they are node content primitives, not entity types users browse or filter.
const SIDENAV_CORE_TYPES = new Set(['task']);

// Derived: core schemas shown in sidenav (only user-queryable ones like "task")
export const builtInSchemas = derived(_schemas, ($s) =>
  $s.filter((s) => s.isCore && SIDENAV_CORE_TYPES.has(s.id))
);

// Derived: user-created custom schemas
export const customSchemas = derived(_schemas, ($s) => $s.filter((s) => !s.isCore));

export const schemasData = { loadSchemas };
