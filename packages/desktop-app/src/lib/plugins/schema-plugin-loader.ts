/**
 * Schema Plugin Auto-Registration System
 *
 * Automatically registers custom entity schemas as plugins, enabling immediate
 * slash command availability without manual plugin registration or app restart.
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
 * Schema Creation → Plugin Auto-Registration → Slash Command Available
 *      ↓                    ↓                           ↓
 * SchemaService    createPluginFromSchema()    SlashCommandService
 * ```
 *
 * The plugin registry already supports runtime registration without restart.
 * SlashCommandService queries the registry fresh on every `/` trigger, so
 * no caching layer blocks dynamic updates.
 *
 * ## Usage
 *
 * ```typescript
 * // Initialize on app startup
 * await initializeSchemaPluginSystem();
 *
 * // Custom entities automatically become available
 * // User creates "invoice" schema → "/invoice" appears in slash menu
 * ```
 *
 * @see packages/desktop-app/src/lib/plugins/plugin-registry.ts - Plugin registration
 * @see packages/desktop-app/src/lib/services/schema-service.ts - Schema management
 */

import type { PluginDefinition } from './types';
import type { SchemaDefinition } from '$lib/types/schema';
import { pluginRegistry } from './plugin-registry';
import { schemaService } from '$lib/services/schema-service';

/**
 * Plugin priority constants
 *
 * Defines the priority order for plugin slash commands in the dropdown.
 * Higher priority commands appear first.
 */
export const PLUGIN_PRIORITIES = {
  CORE: 100, // Core types (text, task, date, etc.)
  CUSTOM_ENTITY: 50, // User-defined custom entity schemas
  USER_COMMAND: 0 // User-created custom commands
} as const;

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
function humanizeSchemaId(id: string): string {
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
 * Convert a schema definition into a plugin definition
 *
 * Creates a minimal plugin that registers the custom entity as a slash command
 * and uses the generic CustomEntityNode component for rendering.
 *
 * @param schema - Schema definition to convert
 * @param schemaId - ID of the schema (used as plugin ID)
 * @returns Plugin definition ready for registration
 *
 * @example
 * ```typescript
 * const schema = await schemaService.getSchema('invoice');
 * const plugin = createPluginFromSchema(schema, 'invoice');
 * // plugin.id === 'invoice'
 * // plugin.config.slashCommands[0].name === 'Invoice'
 * ```
 */
export function createPluginFromSchema(
  schema: SchemaDefinition,
  schemaId: string
): PluginDefinition {
  // Extract display name from schema description or humanize schema ID as fallback
  const displayName = schema.description || humanizeSchemaId(schemaId);

  return {
    id: schemaId,
    name: displayName,
    description: schema.description || `Create ${displayName}`,
    version: `${schema.version}.0.0`, // Use schema version as plugin version
    config: {
      slashCommands: [
        {
          id: schemaId,
          name: displayName,
          description: schema.description || `Create ${displayName}`,
          contentTemplate: '',
          nodeType: schemaId,
          priority: PLUGIN_PRIORITIES.CUSTOM_ENTITY
        }
      ],
      canHaveChildren: true,
      canBeChild: true
    },
    node: {
      // Use generic custom entity component for all custom types
      lazyLoad: () => import('../design/components/custom-entity-node.svelte')
    }
    // No custom viewer - falls back to BaseNodeViewer
    // No custom reference component - falls back to BaseNodeReference
  };
}

/**
 * Register a schema as a plugin immediately
 *
 * Fetches the schema definition and registers it as a plugin. Core types
 * are skipped since they're already registered in core-plugins.ts.
 *
 * @param schemaId - ID of the schema to register
 * @throws {Error} If schema cannot be fetched or registration fails
 *
 * @example
 * ```typescript
 * // Register an invoice schema
 * await registerSchemaPlugin('invoice');
 * // "/invoice" now appears in slash command dropdown
 * ```
 */
export async function registerSchemaPlugin(schemaId: string): Promise<void> {
  try {
    const schema = await schemaService.getSchema(schemaId);

    // Don't register core types (already registered in core-plugins.ts)
    // Defensive: Handle both camelCase and snake_case for type safety across boundaries
    const isCoreType = schema.isCore || (schema as { is_core?: boolean }).is_core;
    if (isCoreType) {
      console.debug(
        `[SchemaPluginLoader] Skipping core type registration: ${schemaId}`
      );
      return;
    }

    // Check if already registered (idempotent)
    if (pluginRegistry.hasPlugin(schemaId)) {
      console.debug(
        `[SchemaPluginLoader] Plugin already registered: ${schemaId}`
      );
      return;
    }

    const plugin = createPluginFromSchema(schema, schemaId);
    pluginRegistry.register(plugin);

    console.log(
      `[SchemaPluginLoader] Registered plugin for custom entity: ${schemaId}`
    );
  } catch (error) {
    console.error(
      `[SchemaPluginLoader] Failed to register schema plugin: ${schemaId}`,
      error
    );
    throw error;
  }
}

/**
 * Unregister a schema plugin
 *
 * Removes the plugin from the registry. The slash command will no longer
 * appear in the dropdown.
 *
 * @param schemaId - ID of the schema to unregister
 *
 * @example
 * ```typescript
 * // Remove invoice plugin
 * unregisterSchemaPlugin('invoice');
 * // "/invoice" no longer appears in slash menu
 * ```
 */
export function unregisterSchemaPlugin(schemaId: string): void {
  if (!pluginRegistry.hasPlugin(schemaId)) {
    console.warn(
      `[SchemaPluginLoader] Attempted to unregister non-existent plugin: ${schemaId}`
    );
    return;
  }

  pluginRegistry.unregister(schemaId);
  console.log(`[SchemaPluginLoader] Unregistered plugin: ${schemaId}`);
}

/**
 * Register all existing custom entity schemas on app startup
 *
 * Queries the schema service for all schemas and registers non-core types
 * as plugins. This ensures existing custom entities are available on launch.
 *
 * Note: This function is not directly exported, but is used internally by
 * initializeSchemaPluginSystem() during app startup.
 *
 * @throws {Error} If schema fetching or registration fails
 *
 * @example
 * ```typescript
 * // Called internally during app initialization
 * await registerExistingSchemas();
 * // All custom entities now available in slash menu
 * ```
 */
async function _registerExistingSchemas(): Promise<void> {
  try {
    console.log('[SchemaPluginLoader] Registering existing custom entity schemas...');

    const schemas = await schemaService.getAllSchemas();

    // Defensive: Handle both camelCase and snake_case for type safety across boundaries
    const customSchemas = schemas.filter(
      (schema) => !schema.isCore && !(schema as { is_core?: boolean }).is_core
    );

    // Parallelize registration for better performance
    await Promise.all(
      customSchemas.map((schema) => registerSchemaPlugin(schema.id))
    );

    console.log(
      `[SchemaPluginLoader] Registered ${customSchemas.length} custom entity schemas`
    );
  } catch (error) {
    console.error(
      '[SchemaPluginLoader] Failed to register existing schemas:',
      error
    );
    throw error;
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
 * Sets up the complete auto-registration workflow:
 * 1. Registers all existing custom entity schemas on startup
 * 2. Listens for schema creation events (future: Tauri events)
 * 3. Listens for schema deletion events (future: Tauri events)
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
    console.log('[SchemaPluginLoader] Initializing schema plugin system...');

    // Register existing custom entity schemas on startup
    const schemas = await schemaService.getAllSchemas();
    const customSchemas = schemas.filter(
      (schema) => !schema.isCore && !(schema as { is_core?: boolean }).is_core
    );

    await Promise.all(
      customSchemas.map((schema) => registerSchemaPlugin(schema.id))
    );

    // TODO: Add Tauri event listeners for schema:created and schema:deleted
    // This will be implemented in a future iteration when Tauri events are added
    //
    // await listen<SchemaCreatedEvent>('schema:created', async (event) => {
    //   const { schema_id, is_core } = event.payload;
    //   if (!is_core) {
    //     await registerSchemaPlugin(schema_id);
    //   }
    // });
    //
    // await listen<SchemaDeletedEvent>('schema:deleted', (event) => {
    //   unregisterSchemaPlugin(event.payload.schema_id);
    // });

    console.log(
      `[SchemaPluginLoader] Schema plugin system initialized (${customSchemas.length} custom entities registered)`
    );

    return {
      success: true,
      registeredCount: customSchemas.length
    };
  } catch (error) {
    const errorMessage = error instanceof Error ? error.message : String(error);
    console.error('[SchemaPluginLoader] Failed to initialize:', error);

    return {
      success: false,
      registeredCount: 0,
      error: errorMessage
    };
  }
}
