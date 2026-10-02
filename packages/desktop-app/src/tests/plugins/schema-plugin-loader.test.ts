/**
 * Schema Plugin Auto-Registration Tests
 *
 * Comprehensive test suite for the schema plugin loader system that automatically
 * converts custom entity schemas into plugins with slash commands.
 *
 * Tests follow the official NodeSpace testing guide patterns.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  createPluginFromSchema,
  registerSchemaPlugin,
  unregisterSchemaPlugin,
  initializeSchemaPluginSystem,
  resyncSchemaPluginsForDatabaseSwitch
} from '$lib/plugins/schema-plugin-loader';
import type { SchemaNode } from '$lib/types/schema-node';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { backendAdapter } from '$lib/services/backend-adapter';

// Mock backend adapter
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getSchema: vi.fn(),
    getAllSchemas: vi.fn()
  }
}));

/**
 * Helper to create a mock schema node with typed top-level fields
 * Matches the backend SchemaNode serialization format
 */
function createMockSchemaNode(
  id: string,
  options: {
    isCore?: boolean;
    schemaVersion?: number;
    content?: string;
  } = {}
): SchemaNode {
  return {
    lifecycleStatus: 'active' as const,
    properties: {},
    id,
    nodeType: 'schema',
    // content is the schema display name (e.g. "Invoice", "Customer")
    content: options.content ?? id,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    isCore: options.isCore ?? false,
    schemaVersion: options.schemaVersion ?? 1,
    relationships: [],
    fields: []
  };
}

describe('Schema Plugin Loader - createPluginFromSchema()', () => {
  it('should convert schema node to plugin with correct structure', () => {
    const schemaNode = createMockSchemaNode('invoice', {
      content: 'Sales Invoice',
      schemaVersion: 1
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin).toMatchObject({
      id: 'invoice',
      name: 'Sales Invoice',
      description: 'Create Sales Invoice',
      version: '1.0.0',
      config: {
        // Entity types are not slash-creatable — no slash command is generated for
        // user-defined schema types. Instances are created via the sidenav's type view.
        slashCommands: [],
      }
    });
  });

  it('should use schema content as display name', () => {
    const schemaNode = createMockSchemaNode('invoice', {
      content: 'Customer Invoice'
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.name).toBe('Customer Invoice');
    // No slash command is generated for the display name to propagate onto.
    expect(plugin.config.slashCommands).toHaveLength(0);
  });

  it('should use schema content as display name, falling back to humanized ID when content is empty', () => {
    // When content is set (normal case), it IS the display name
    const withContent = createMockSchemaNode('invoice', { content: 'Invoice' });
    expect(createPluginFromSchema(withContent).name).toBe('Invoice');

    // When content is empty (edge case), humanize the schema ID as last resort
    const emptyContent: SchemaNode = { ...createMockSchemaNode('sales-invoice'), content: '' };
    expect(createPluginFromSchema(emptyContent).name).toBe('Sales Invoice');
  });

  it('should not generate a slash command for custom entities', () => {
    const schemaNode = createMockSchemaNode('invoice');

    const plugin = createPluginFromSchema(schemaNode);

    // Entity types (core and user-defined alike) are not slash-creatable.
    expect(plugin.config.slashCommands).toEqual([]);
  });

  it('should use schema version as plugin version', () => {
    const schemaNode = createMockSchemaNode('invoice', {
      schemaVersion: 5
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.version).toBe('5.0.0');
  });

  it('should not include a node component (custom entities use BaseNode fallback)', () => {
    const schemaNode = createMockSchemaNode('invoice');

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.node).toBeUndefined();
  });

  it('should not generate a slash command, and reports hasTitleTemplate: false, when no titleTemplate is set', () => {
    const schemaNode = createMockSchemaNode('customer');
    const plugin = createPluginFromSchema(schemaNode);
    expect(plugin.config.slashCommands).toHaveLength(0);
    expect(plugin.hasTitleTemplate).toBe(false);
    expect(plugin.titleTemplate).toBeUndefined();
  });

  it('should not generate a slash command, but still carries hasTitleTemplate/titleTemplate at the plugin level, when titleTemplate is set', () => {
    // No slash command exists to carry this signal anymore (entity types aren't
    // slash-creatable) — row-rendering surfaces (node-row.svelte, resolveDisplayTitle) read
    // it via PluginRegistry.hasTitleTemplate()/getTitleTemplate() instead, off the plugin
    // definition itself.
    const schemaNode: SchemaNode = {
      ...createMockSchemaNode('customer'),
      titleTemplate: '{first_name} {last_name}'
    };
    const plugin = createPluginFromSchema(schemaNode);
    expect(plugin.config.slashCommands).toHaveLength(0);
    expect(plugin.hasTitleTemplate).toBe(true);
    expect(plugin.titleTemplate).toBe('{first_name} {last_name}');
  });

  it('should not generate a slash command regardless of schema ID', () => {
    const schemaNode = createMockSchemaNode('customEntity');

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.config.slashCommands).toHaveLength(0);
  });
});

describe('Schema Plugin Loader - registerSchemaPlugin()', () => {
  beforeEach(() => {
    pluginRegistry.clear();
    vi.clearAllMocks();
  });

  afterEach(() => {
    pluginRegistry.clear();
  });

  it('should register non-core schema as plugin', async () => {
    const schemaNode = createMockSchemaNode('invoice', {
      isCore: false
    });

    vi.mocked(backendAdapter.getSchema).mockResolvedValue(schemaNode);

    await registerSchemaPlugin('invoice');

    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);
    const plugin = pluginRegistry.getPlugin('invoice');
    expect(plugin?.id).toBe('invoice');
  });

  it('should skip core schemas (isCore: true)', async () => {
    const coreSchema = createMockSchemaNode('text', {
      isCore: true
    });

    vi.mocked(backendAdapter.getSchema).mockResolvedValue(coreSchema);

    await registerSchemaPlugin('text');

    expect(pluginRegistry.hasPlugin('text')).toBe(false);
  });

  it('should be idempotent - no duplicate registrations', async () => {
    const schemaNode = createMockSchemaNode('invoice', {
      isCore: false
    });

    vi.mocked(backendAdapter.getSchema).mockResolvedValue(schemaNode);

    await registerSchemaPlugin('invoice');
    await registerSchemaPlugin('invoice');
    await registerSchemaPlugin('invoice');

    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);
    const plugins = pluginRegistry.getAllPlugins();
    const invoicePlugins = plugins.filter((p) => p.id === 'invoice');
    expect(invoicePlugins).toHaveLength(1);
  });

  it('should throw error if schema cannot be fetched', async () => {
    vi.mocked(backendAdapter.getSchema).mockRejectedValue(new Error('Schema not found'));

    await expect(registerSchemaPlugin('nonexistent')).rejects.toThrow('Schema not found');
  });

  it('should skip non-schema nodes gracefully', async () => {
    // Mock returns a value that fails isSchemaNode() check
    // (missing required typed fields like isCore, schemaVersion, fields)
    const nonSchemaNode = {
      id: 'task-123',
      nodeType: 'task', // Not a schema node - isSchemaNode will return false
      content: 'Some task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1
      // Missing: isCore, schemaVersion, description, fields
    } as unknown as SchemaNode;

    vi.mocked(backendAdapter.getSchema).mockResolvedValue(nonSchemaNode);

    await registerSchemaPlugin('task-123');

    expect(pluginRegistry.hasPlugin('task-123')).toBe(false);
  });

  it('refreshes an already-registered plugin instead of leaving it stale', async () => {
    // First registration: no title_template yet.
    vi.mocked(backendAdapter.getSchema).mockResolvedValue(
      createMockSchemaNode('customer')
    );
    await registerSchemaPlugin('customer');
    expect(pluginRegistry.hasTitleTemplate('customer')).toBe(false);

    // update_schema adds a title_template mid-session — a second call for the
    // SAME already-registered id must pick it up. Before the fix,
    // registerSchemaPlugin's `hasPlugin` early-return made this a no-op and
    // hasTitleTemplate stayed stuck at `false`.
    vi.mocked(backendAdapter.getSchema).mockResolvedValue({
      ...createMockSchemaNode('customer'),
      titleTemplate: '{first_name} {last_name}'
    });
    await registerSchemaPlugin('customer');

    expect(pluginRegistry.hasTitleTemplate('customer')).toBe(true);
    expect(pluginRegistry.getTitleTemplate('customer')).toBe('{first_name} {last_name}');
  });
});

describe('Schema Plugin Loader - unregisterSchemaPlugin()', () => {
  beforeEach(() => {
    pluginRegistry.clear();
    vi.clearAllMocks();
  });

  afterEach(() => {
    pluginRegistry.clear();
  });

  it('should unregister an existing plugin', async () => {
    const schemaNode = createMockSchemaNode('invoice', {
      isCore: false
    });

    vi.mocked(backendAdapter.getSchema).mockResolvedValue(schemaNode);

    await registerSchemaPlugin('invoice');
    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);

    unregisterSchemaPlugin('invoice');
    expect(pluginRegistry.hasPlugin('invoice')).toBe(false);
  });

  it('should handle unregistering non-existent plugin gracefully', () => {
    // Should not throw
    expect(() => unregisterSchemaPlugin('nonexistent')).not.toThrow();
  });
});

describe('Schema Plugin Loader - initializeSchemaPluginSystem()', () => {
  beforeEach(() => {
    pluginRegistry.clear();
    vi.clearAllMocks();
  });

  afterEach(() => {
    pluginRegistry.clear();
  });

  it('should register all custom (non-core) schemas', async () => {
    const schemas = [
      createMockSchemaNode('text', { isCore: true }),
      createMockSchemaNode('task', { isCore: true }),
      createMockSchemaNode('invoice', { isCore: false }),
      createMockSchemaNode('person', { isCore: false })
    ];

    vi.mocked(backendAdapter.getAllSchemas).mockResolvedValue(schemas);
    vi.mocked(backendAdapter.getSchema).mockImplementation(async (id) => {
      return schemas.find((s) => s.id === id)!;
    });

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(true);
    expect(result.registeredCount).toBe(2); // Only custom schemas

    // Custom schemas registered
    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);
    expect(pluginRegistry.hasPlugin('person')).toBe(true);

    // Core schemas NOT registered
    expect(pluginRegistry.hasPlugin('text')).toBe(false);
    expect(pluginRegistry.hasPlugin('task')).toBe(false);
  });

  it('should return success: false on error', async () => {
    vi.mocked(backendAdapter.getAllSchemas).mockRejectedValue(new Error('Connection failed'));

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(false);
    expect(result.error).toContain('Connection failed');
  });

  it('should handle empty schema list', async () => {
    vi.mocked(backendAdapter.getAllSchemas).mockResolvedValue([]);

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(true);
    expect(result.registeredCount).toBe(0);
  });

  it('should handle all-core schemas (nothing to register)', async () => {
    const schemas = [
      createMockSchemaNode('text', { isCore: true }),
      createMockSchemaNode('task', { isCore: true })
    ];

    vi.mocked(backendAdapter.getAllSchemas).mockResolvedValue(schemas);

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(true);
    expect(result.registeredCount).toBe(0);
  });
});

describe('Schema Plugin Loader - resyncSchemaPluginsForDatabaseSwitch()', () => {
  beforeEach(() => {
    pluginRegistry.clear();
    vi.clearAllMocks();
  });

  afterEach(() => {
    pluginRegistry.clear();
  });

  it('unregisters a custom type absent from the newly-active database and registers a new one', async () => {
    // Simulate database A: 'invoice' was registered while A was active.
    vi.mocked(backendAdapter.getSchema).mockResolvedValue(
      createMockSchemaNode('invoice')
    );
    await registerSchemaPlugin('invoice');
    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);

    // Switch to database B, whose custom schemas are just 'customer'.
    const dbBSchemas = [createMockSchemaNode('customer')];
    vi.mocked(backendAdapter.getAllSchemas).mockResolvedValue(dbBSchemas);
    vi.mocked(backendAdapter.getSchema).mockImplementation(
      async (id) => dbBSchemas.find((s) => s.id === id)!
    );

    await resyncSchemaPluginsForDatabaseSwitch();

    // 'invoice' doesn't exist in B — must not keep resolving with A's metadata.
    expect(pluginRegistry.hasPlugin('invoice')).toBe(false);
    // 'customer' is unique to B and must now be registered.
    expect(pluginRegistry.hasPlugin('customer')).toBe(true);
  });

  it('refreshes a same-id type whose title template differs between databases', async () => {
    vi.mocked(backendAdapter.getSchema).mockResolvedValue({
      ...createMockSchemaNode('customer'),
      titleTemplate: '{name_a}'
    });
    await registerSchemaPlugin('customer');
    expect(pluginRegistry.getTitleTemplate('customer')).toBe('{name_a}');

    const dbBSchemas = [
      {
        ...createMockSchemaNode('customer'),
        titleTemplate: '{name_b}'
      }
    ];
    vi.mocked(backendAdapter.getAllSchemas).mockResolvedValue(dbBSchemas);
    vi.mocked(backendAdapter.getSchema).mockImplementation(
      async (id) => dbBSchemas.find((s) => s.id === id)!
    );

    await resyncSchemaPluginsForDatabaseSwitch();

    // Same id in both databases, but the new database's template must win —
    // a stale-but-present registration is exactly what the removed
    // `hasPlugin` early-return in `registerSchemaPlugin` used to block.
    expect(pluginRegistry.getTitleTemplate('customer')).toBe('{name_b}');
  });

  it('never unregisters a hardcoded core-type plugin it did not itself register', async () => {
    // core-plugins.ts registers core types directly against the same
    // singleton registry — simulate that here without going through
    // registerSchemaPlugin (which is what keeps `task` out of this module's
    // own bookkeeping in the first place).
    pluginRegistry.register({
      id: 'task',
      name: 'Task',
      description: 'Core task type',
      version: '1.0.0',
      config: { slashCommands: [] }
    });
    expect(pluginRegistry.hasPlugin('task')).toBe(true);

    // The new database's custom schema list doesn't include 'task' (core
    // types never appear there), which is exactly the condition a naive
    // "unregister anything not in the new list" resync would misuse to drop it.
    vi.mocked(backendAdapter.getAllSchemas).mockResolvedValue([]);

    await resyncSchemaPluginsForDatabaseSwitch();

    expect(pluginRegistry.hasPlugin('task')).toBe(true);
  });
});
