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
  PLUGIN_PRIORITIES
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
    description?: string;
    content?: string;
  } = {}
): SchemaNode {
  return {
    id,
    nodeType: 'schema',
    // content is the schema display name (e.g. "Invoice", "Customer")
    content: options.content ?? id,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    isCore: options.isCore ?? false,
    schemaVersion: options.schemaVersion ?? 1,
    description: options.description ?? '',
    fields: []
  };
}

describe('Schema Plugin Loader - createPluginFromSchema()', () => {
  it('should convert schema node to plugin with correct structure', () => {
    const schemaNode = createMockSchemaNode('invoice', {
      content: 'Sales Invoice',
      description: 'Schema for invoices',
      schemaVersion: 1
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin).toMatchObject({
      id: 'invoice',
      name: 'Sales Invoice',
      description: 'Schema for invoices',
      version: '1.0.0',
      config: {
        slashCommands: [
          {
            id: 'invoice',
            name: 'Sales Invoice',
            description: 'Schema for invoices',
            contentTemplate: '',
            nodeType: 'invoice',
            priority: PLUGIN_PRIORITIES.CUSTOM_ENTITY
          }
        ],
        canHaveChildren: true,
        canBeChild: true
      }
    });
  });

  it('should use schema content as display name', () => {
    const schemaNode = createMockSchemaNode('invoice', {
      content: 'Customer Invoice'
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.name).toBe('Customer Invoice');
    expect(plugin.config.slashCommands[0].name).toBe('Customer Invoice');
  });

  it('should use schema content as display name, falling back to humanized ID when content is empty', () => {
    // When content is set (normal case), it IS the display name
    const withContent = createMockSchemaNode('invoice', { content: 'Invoice' });
    expect(createPluginFromSchema(withContent).name).toBe('Invoice');

    // When content is empty (edge case), humanize the schema ID as last resort
    const emptyContent: SchemaNode = { ...createMockSchemaNode('sales-invoice'), content: '' };
    expect(createPluginFromSchema(emptyContent).name).toBe('Sales Invoice');
  });

  it('should set correct priority for custom entities', () => {
    const schemaNode = createMockSchemaNode('invoice', {
      description: 'Invoice'
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.config.slashCommands[0].priority).toBe(PLUGIN_PRIORITIES.CUSTOM_ENTITY);
    expect(PLUGIN_PRIORITIES.CUSTOM_ENTITY).toBe(50);
  });

  it('should use schema version as plugin version', () => {
    const schemaNode = createMockSchemaNode('invoice', {
      schemaVersion: 5,
      description: 'Invoice'
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.version).toBe('5.0.0');
  });

  it('should include lazy-loaded CustomEntityNode component', () => {
    const schemaNode = createMockSchemaNode('invoice', {
      description: 'Invoice'
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.node).toBeDefined();
    expect(plugin.node?.lazyLoad).toBeInstanceOf(Function);
  });

  it('should set contentTemplate to empty string when no titleTemplate (node name is editable)', () => {
    const schemaNode = createMockSchemaNode('customer', { description: 'Customer' });
    const plugin = createPluginFromSchema(schemaNode);
    expect(plugin.config.slashCommands[0].contentTemplate).toBe('');
  });

  it('should set contentTemplate to Untitled when titleTemplate is set (content is not the name)', () => {
    const schemaNode: SchemaNode = {
      ...createMockSchemaNode('customer', { description: 'Customer' }),
      titleTemplate: '{first_name} {last_name}'
    };
    const plugin = createPluginFromSchema(schemaNode);
    expect(plugin.config.slashCommands[0].contentTemplate).toBe('Untitled');
  });

  it('should set nodeType to schema ID for slash command creation', () => {
    const schemaNode = createMockSchemaNode('customEntity', {
      description: 'Custom Entity'
    });

    const plugin = createPluginFromSchema(schemaNode);

    expect(plugin.config.slashCommands[0].nodeType).toBe('customEntity');
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
      isCore: false,
      description: 'Invoice'
    });

    vi.mocked(backendAdapter.getSchema).mockResolvedValue(schemaNode);

    await registerSchemaPlugin('invoice');

    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);
    const plugin = pluginRegistry.getPlugin('invoice');
    expect(plugin?.id).toBe('invoice');
  });

  it('should skip core schemas (isCore: true)', async () => {
    const coreSchema = createMockSchemaNode('text', {
      isCore: true,
      description: 'Text Node'
    });

    vi.mocked(backendAdapter.getSchema).mockResolvedValue(coreSchema);

    await registerSchemaPlugin('text');

    expect(pluginRegistry.hasPlugin('text')).toBe(false);
  });

  it('should be idempotent - no duplicate registrations', async () => {
    const schemaNode = createMockSchemaNode('invoice', {
      isCore: false,
      description: 'Invoice'
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
      isCore: false,
      description: 'Invoice'
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
      createMockSchemaNode('text', { isCore: true, description: 'Text' }),
      createMockSchemaNode('task', { isCore: true, description: 'Task' }),
      createMockSchemaNode('invoice', { isCore: false, description: 'Invoice' }),
      createMockSchemaNode('person', { isCore: false, description: 'Person' })
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
