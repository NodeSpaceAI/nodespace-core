/**
 * Schema Plugin Auto-Registration System
 *
 * Automatically registers custom entity schemas as plugins, enabling reference
 * components and viewer fallback without manual plugin registration or app restart.
 * Entity types are not slash-creatable (core or user-defined alike) — creation goes
 * through the sidenav's type view instead.
 *
 * ## Features
 *
 * - Converts schema definitions into plugin definitions
 * - Auto-registers plugins when schemas are created
 * - Auto-unregisters when schemas are deleted
 * - Registers existing schemas on app startup
 * - Hot-reload support (no restart required)
 *
 * ## Architecture
 *
 * ```
 * Schema Creation → Plugin Auto-Registration → Reference/Viewer Available
 *      ↓                    ↓                           ↓
 * SchemaService    createPluginFromSchema()      PluginRegistry
 * ```
 *
 * The plugin registry already supports runtime registration without restart.
 *
 * ## Usage
 *
 * ```typescript
 * // Initialize on app startup
 * await initializeSchemaPluginSystem();
 *
 * // Custom entities automatically become available for reference/viewer resolution
 * // User creates "invoice" schema → invoice nodes render via BaseNode fallback
 * ```
 *
 * @see packages/desktop-app/src/lib/plugins/plugin-registry.ts - Plugin registration
 * @see packages/desktop-app/src/lib/services/schema-service.ts - Schema management
 */

import type { PluginDefinition } from './types';
import { pluginRegistry } from './plugin-registry';
import { backendAdapter } from '$lib/services/backend-adapter';
import { type SchemaNode, isSchemaNode } from '$lib/types/schema-node';
import { createLogger } from '$lib/utils/logger';
import { toError } from '$lib/types/errors';

const log = createLogger('SchemaPluginLoader');

/**
 * Ids this module has registered into `pluginRegistry` (custom, non-core
 * schemas only). Tracked separately from the registry itself so
 * {@link resyncSchemaPluginsForDatabaseSwitch} can tell a schema-derived
 * entry apart from a hardcoded core-type plugin (registered elsewhere, e.g.
 * `core-plugins.ts`) that happens to share the same `Map` — resyncing must
 * never unregister those. Kept in sync by {@link registerSchemaPlugin} and
 * {@link unregisterSchemaPlugin}, the only two writers of `pluginRegistry`
 * entries this module owns.
 */
const registeredSchemaIds = new Set<string>();

/**
 * Bumped on every {@link resyncSchemaPluginsForDatabaseSwitch} call. A second
 * database switch that starts while an earlier resync's fetch is still in
 * flight supersedes it — the earlier call detects the mismatch after its
 * await and stops before writing schema data for a database that is no
 * longer active into the (database-agnostic, singleton) plugin registry.
 */
let resyncGeneration = 0;

/**
 * Humanize a schema ID into a readable display name
 *
 * Converts technical IDs into user-friendly names:
 * - camelCase → Camel Case
 * - snake_case → Snake Case
 * - kebab-case → Kebab Case
 * - Capitalizes each word
 *
 * @param id - Schema ID to humanize
 * @returns Humanized display name
 *
 * @example
 * ```typescript
 * humanizeSchemaId('invoice') // 'Invoice'
 * humanizeSchemaId('salesInvoice') // 'Sales Invoice'
 * humanizeSchemaId('sales_invoice') // 'Sales Invoice'
 * humanizeSchemaId('sales-invoice') // 'Sales Invoice'
 * ```
 */
export function humanizeSchemaId(id: string): string {
  return (
    id
      // Insert space before uppercase letters (camelCase → camel Case)
      .replace(/([A-Z])/g, ' $1')
      // Replace underscores and hyphens with spaces
      .replace(/[_-]/g, ' ')
      // Trim leading/trailing spaces
      .trim()
      // Capitalize first letter of each word
      .replace(/\b\w/g, (char) => char.toUpperCase())
  );
}

/**
 * Convert a schema node into a plugin definition
 *
 * Creates a minimal plugin that registers the custom entity for reference/viewer
 * resolution via the BaseNode fallback. No slash command is generated — entity types
 * are not slash-creatable; instances are created via the sidenav's type view.
 *
 * @param schema - Schema node to convert
 * @returns Plugin definition ready for registration
 *
 * @example
 * ```typescript
 * const schemaNode = await backendAdapter.getSchema('invoice');
 * if (isSchemaNode(schemaNode)) {
 *   const plugin = createPluginFromSchema(schemaNode);
 * }
 * ```
 */
export function createPluginFromSchema(schema: SchemaNode): PluginDefinition {
  const schemaId = schema.id;
  const version = schema.schemaVersion;

  // Display name comes from schema content (the schema's name, e.g. "Customer").
  // description is for human-readable purpose/context, not the display name.
  const displayName = schema.content || humanizeSchemaId(schemaId);
  const description = schema.description ?? '';

  return {
    id: schemaId,
    name: displayName,
    description: description || `Create ${displayName}`,
    version: `${version}.0.0`, // Use schema version as plugin version
    config: {
      // No slash command — entity types (core and user-defined alike) are not
      // slash-creatable. User-defined types are created via the sidenav's type view
      // (customSchemas → handleSchemaClick → create instance).
      slashCommands: [],
      canHaveChildren: true,
      canBeChild: true
    },
    // Carried at the plugin level (not on a slash command, since there isn't one) so
    // row-rendering surfaces can still tell this type's `title` apart from `content` via
    // PluginRegistry.hasTitleTemplate() — see node-row.svelte and resolveDisplayTitle().
    hasTitleTemplate: !!schema.titleTemplate,
    titleTemplate: schema.titleTemplate
    // No node component — custom entities render via BaseNode fallback.
    // No custom viewer — falls back to BaseNodeViewer.
    // No custom reference — falls back to BaseNodeReference.
  };
}

/**
 * Register a schema as a plugin, or refresh it if already registered
 *
 * Fetches the schema node and (re-)registers it as a plugin. Core types
 * are skipped since they're already registered in core-plugins.ts.
 *
 * Always upserts rather than skipping an already-registered id: an existing
 * plugin's `hasTitleTemplate`/`titleTemplate` (which `resolveDisplayTitle`
 * depends on) must stay refreshable — a schema whose `title_template` was
 * added/changed via `update_schema` after the initial registration needs
 * this call to actually pick up the change, and a database switch relies on
 * the same upsert to correct a same-id type registered from a *different*
 * database's schema (see {@link resyncSchemaPluginsForDatabaseSwitch}).
 * `createPluginFromSchema` never attaches real node/viewer/reference
 * components (custom entities always fall back to BaseNode et al.), so
 * overwriting an existing entry can never clobber a component-bearing
 * plugin — those only ever come from `core-plugins.ts`, and core types are
 * filtered out above.
 *
 * @param schemaId - ID of the schema to register
 * @throws {Error} If schema cannot be fetched or registration fails
 *
 * @example
 * ```typescript
 * // Register an invoice schema
 * await registerSchemaPlugin('invoice');
 * // Invoice nodes now resolve to a reference/viewer via the plugin registry
 * ```
 */
export async function registerSchemaPlugin(schemaId: string): Promise<void> {
  try {
    const node = await backendAdapter.getSchema(schemaId);

    // Verify it's a schema node
    if (!isSchemaNode(node)) {
      log.warn(`Node ${schemaId} is not a schema node`);
      return;
    }

    // Don't register core types (already registered in core-plugins.ts)
    // Access typed field directly (no helper needed)
    if (node.isCore) {
      log.debug(`Skipping core type registration: ${schemaId}`);
      return;
    }

    const plugin = createPluginFromSchema(node);
    pluginRegistry.register(plugin);
    registeredSchemaIds.add(schemaId);

    log.info(`Registered plugin for custom entity: ${schemaId}`);
  } catch (error) {
    log.error(`Failed to register schema plugin: ${schemaId}`, error);
    throw error;
  }
}

/**
 * Unregister a schema plugin
 *
 * Removes the plugin from the registry. Reference/viewer resolution for the
 * type will no longer be available.
 *
 * @param schemaId - ID of the schema to unregister
 *
 * @example
 * ```typescript
 * // Remove invoice plugin
 * unregisterSchemaPlugin('invoice');
 * ```
 */
export function unregisterSchemaPlugin(schemaId: string): void {
  // Drop tracking unconditionally (even if the registry never actually held
  // it, e.g. a stale id from a database already left) so `registeredSchemaIds`
  // never grows unboundedly stale entries that a later resync would have to
  // reason about.
  registeredSchemaIds.delete(schemaId);

  if (!pluginRegistry.hasPlugin(schemaId)) {
    log.debug(`Skipping unregister, plugin not found: ${schemaId}`);
    return;
  }

  pluginRegistry.unregister(schemaId);
  log.info(`Unregistered plugin: ${schemaId}`);
}

/**
 * Re-sync the schema plugin registry for a database switch.
 *
 * The plugin registry is a database-agnostic singleton, but its
 * `hasTitleTemplate`/`titleTemplate` entries are per-database schema data:
 * without this, a custom type from the database just left keeps resolving
 * titles with that database's (now wrong, possibly nonexistent-in-the-new-
 * database) template, and a custom type unique to the newly-active database
 * has no plugin at all until the next full app restart.
 *
 * Drops every plugin this module registered for the previous database whose
 * type isn't in the newly-active database's current schema list (leaves
 * hardcoded core-type plugins from `core-plugins.ts` untouched — those are
 * tracked separately via {@link registeredSchemaIds}), then (re-)registers
 * every one of the new database's custom schemas — including same-id types
 * that were already registered, so a differing `title_template` between the
 * two databases is corrected rather than left at whichever database
 * registered first.
 *
 * Mirrors {@link initializeSchemaPluginSystem}'s startup registration, but
 * for a switch instead of first boot. Best-effort: a failure here leaves
 * some titles stale until the next successful switch or app restart, which
 * is a strict improvement over never attempting the resync at all.
 */
export async function resyncSchemaPluginsForDatabaseSwitch(): Promise<void> {
  const generation = ++resyncGeneration;
  try {
    const nodes = await backendAdapter.getAllSchemas();

    // A second switch started while this fetch was in flight and will run
    // its own resync against the now-active database — applying this stale
    // result would fight it, so stop here rather than write.
    if (generation !== resyncGeneration) return;

    const customSchemas = nodes.filter((node) => isSchemaNode(node) && !node.isCore);
    const nextIds = new Set(customSchemas.map((node) => node.id));

    for (const id of [...registeredSchemaIds]) {
      if (!nextIds.has(id)) unregisterSchemaPlugin(id);
    }

    await Promise.all(customSchemas.map((node) => registerSchemaPlugin(node.id)));

    log.info(
      `Re-synced schema plugins for database switch (${customSchemas.length} custom entities)`
    );
  } catch (error) {
    log.error('Failed to re-sync schema plugins for database switch', error);
  }
}

/**
 * Result of schema plugin system initialization
 */
export interface InitializationResult {
  success: boolean;
  registeredCount: number;
  error?: string;
}

/**
 * Initialize schema plugin auto-registration system
 *
 * Registers all existing custom entity schemas on startup so their
 * reference/viewer resolution is available immediately on launch.
 *
 * Dynamic registration for schemas created/deleted at runtime is handled
 * by BrowserSyncService and TauriSyncListener, which call
 * registerSchemaPlugin / unregisterSchemaPlugin on domain events.
 *
 * Call this once during app startup in the root layout.
 *
 * @returns Initialization result with success status and details
 *
 * @example
 * ```typescript
 * // In +layout.svelte
 * onMount(async () => {
 *   const result = await initializeSchemaPluginSystem();
 *   if (!result.success) {
 *     // Handle initialization failure
 *     console.warn(`Custom entities unavailable: ${result.error}`);
 *   }
 * });
 * ```
 */
export async function initializeSchemaPluginSystem(): Promise<InitializationResult> {
  try {
    log.debug('Initializing schema plugin system...');

    // Register existing custom entity schemas on startup
    const nodes = await backendAdapter.getAllSchemas();
    // Access typed field directly (no helper needed)
    const customSchemas = nodes.filter(
      (node) => isSchemaNode(node) && !node.isCore
    );

    await Promise.all(
      customSchemas.map((node) => registerSchemaPlugin(node.id))
    );

    log.info(
      `Schema plugin system initialized (${customSchemas.length} custom entities registered)`
    );

    return {
      success: true,
      registeredCount: customSchemas.length
    };
  } catch (error) {
    const errorMessage = toError(error).message;
    log.error('Failed to initialize:', error);

    return {
      success: false,
      registeredCount: 0,
      error: errorMessage
    };
  }
}
