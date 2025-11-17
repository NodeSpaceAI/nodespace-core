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
import type { SchemaDefinition } from '$lib/types/schema';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { schemaService } from '$lib/services/schema-service';

// Mock schema service
vi.mock('$lib/services/schema-service', () => ({
  schemaService: {
    getSchema: vi.fn(),
    getAllSchemas: vi.fn()
  }
}));

describe('Schema Plugin Loader - createPluginFromSchema()', () => {
  it('should convert schema to plugin with correct structure', () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Sales Invoice',
      fields: []
    };

    const plugin = createPluginFromSchema(schema, 'invoice');

    expect(plugin).toMatchObject({
      id: 'invoice',
      name: 'Sales Invoice',
      description: 'Sales Invoice',
      version: '1.0.0',
      config: {
        slashCommands: [
          {
            id: 'invoice',
            name: 'Sales Invoice',
            description: 'Sales Invoice',
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

  it('should use schema description as display name', () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Customer Invoice',
      fields: []
    };

    const plugin = createPluginFromSchema(schema, 'invoice');

    expect(plugin.name).toBe('Customer Invoice');
    expect(plugin.config.slashCommands[0].name).toBe('Customer Invoice');
  });

  it('should humanize schema ID when description is missing', () => {
    const testCases = [
      { id: 'invoice', expected: 'Invoice' },
      { id: 'salesInvoice', expected: 'Sales Invoice' },
      { id: 'sales_invoice', expected: 'Sales Invoice' },
      { id: 'sales-invoice', expected: 'Sales Invoice' },
      { id: 'INVOICE', expected: 'I N V O I C E' }, // Edge case: all caps
      { id: 'invoice123', expected: 'Invoice123' }
    ];

    testCases.forEach(({ id, expected }) => {
      const schema: SchemaDefinition = {
        isCore: false,
        version: 1,
        description: '',
        fields: []
      };

      const plugin = createPluginFromSchema(schema, id);

      expect(plugin.name).toBe(expected);
    });
  });

  it('should set correct priority for custom entities', () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Invoice',
      fields: []
    };

    const plugin = createPluginFromSchema(schema, 'invoice');

    expect(plugin.config.slashCommands[0].priority).toBe(PLUGIN_PRIORITIES.CUSTOM_ENTITY);
    expect(PLUGIN_PRIORITIES.CUSTOM_ENTITY).toBe(50);
  });

  it('should use schema version as plugin version', () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 5,
      description: 'Invoice',
      fields: []
    };

    const plugin = createPluginFromSchema(schema, 'invoice');

    expect(plugin.version).toBe('5.0.0');
  });

  it('should include lazy-loaded CustomEntityNode component', () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Invoice',
      fields: []
    };

    const plugin = createPluginFromSchema(schema, 'invoice');

    expect(plugin.node).toBeDefined();
    expect(plugin.node?.lazyLoad).toBeInstanceOf(Function);
  });

  it('should set nodeType to schema ID for slash command creation', () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Custom Entity',
      fields: []
    };

    const plugin = createPluginFromSchema(schema, 'customEntity');

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
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Invoice',
      fields: []
    };

    vi.mocked(schemaService.getSchema).mockResolvedValue(schema);

    await registerSchemaPlugin('invoice');

    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);
    const plugin = pluginRegistry.getPlugin('invoice');
    expect(plugin?.id).toBe('invoice');
  });

  it('should skip core schemas (isCore: true)', async () => {
    const coreSchema: SchemaDefinition = {
      isCore: true,
      version: 1,
      description: 'Text Node',
      fields: []
    };

    vi.mocked(schemaService.getSchema).mockResolvedValue(coreSchema);

    await registerSchemaPlugin('text');

    expect(pluginRegistry.hasPlugin('text')).toBe(false);
  });

  it('should handle both camelCase and snake_case for isCore check', async () => {
    // Test camelCase (TypeScript convention)
    const camelCaseSchema: SchemaDefinition = {
      isCore: true,
      version: 1,
      description: 'Text',
      fields: []
    };

    vi.mocked(schemaService.getSchema).mockResolvedValue(camelCaseSchema);
    await registerSchemaPlugin('text1');
    expect(pluginRegistry.hasPlugin('text1')).toBe(false);

    // Test snake_case (Rust convention) - defensive programming
    const snakeCaseSchema = {
      isCore: false,
      is_core: true, // snake_case from Rust
      version: 1,
      description: 'Text',
      fields: []
    } as SchemaDefinition & { is_core: boolean };

    vi.mocked(schemaService.getSchema).mockResolvedValue(snakeCaseSchema);
    await registerSchemaPlugin('text2');
    expect(pluginRegistry.hasPlugin('text2')).toBe(false);
  });

  it('should be idempotent - no duplicate registrations', async () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Invoice',
      fields: []
    };

    vi.mocked(schemaService.getSchema).mockResolvedValue(schema);

    await registerSchemaPlugin('invoice');
    await registerSchemaPlugin('invoice');
    await registerSchemaPlugin('invoice');

    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);
    const plugins = pluginRegistry.getAllPlugins();
    const invoicePlugins = plugins.filter((p) => p.id === 'invoice');
    expect(invoicePlugins).toHaveLength(1);
  });

  it('should throw error if schema cannot be fetched', async () => {
    vi.mocked(schemaService.getSchema).mockRejectedValue(
      new Error('Schema not found')
    );

    await expect(registerSchemaPlugin('nonexistent')).rejects.toThrow();
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

  it('should unregister plugin by schema ID', async () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Invoice',
      fields: []
    };

    vi.mocked(schemaService.getSchema).mockResolvedValue(schema);

    await registerSchemaPlugin('invoice');
    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);

    unregisterSchemaPlugin('invoice');
    expect(pluginRegistry.hasPlugin('invoice')).toBe(false);
  });

  it('should handle unregistering non-existent plugin gracefully', () => {
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

  it('should register all non-core schemas on startup', async () => {
    const schemas: Array<SchemaDefinition & { id: string }> = [
      {
        id: 'text',
        isCore: true,
        version: 1,
        description: 'Text',
        fields: []
      },
      {
        id: 'task',
        isCore: true,
        version: 1,
        description: 'Task',
        fields: []
      },
      {
        id: 'invoice',
        isCore: false,
        version: 1,
        description: 'Invoice',
        fields: []
      },
      {
        id: 'customer',
        isCore: false,
        version: 1,
        description: 'Customer',
        fields: []
      }
    ];

    vi.mocked(schemaService.getAllSchemas).mockResolvedValue(schemas);
    vi.mocked(schemaService.getSchema).mockImplementation(async (id) => {
      const schema = schemas.find((s) => s.id === id);
      if (!schema) throw new Error('Schema not found');
      const { id: _id, ...schemaWithoutId } = schema;
      return schemaWithoutId;
    });

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(true);
    expect(result.registeredCount).toBe(2); // Only invoice and customer
    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);
    expect(pluginRegistry.hasPlugin('customer')).toBe(true);
    expect(pluginRegistry.hasPlugin('text')).toBe(false); // Core type excluded
    expect(pluginRegistry.hasPlugin('task')).toBe(false); // Core type excluded
  });

  it('should return success=false on initialization failure', async () => {
    vi.mocked(schemaService.getAllSchemas).mockRejectedValue(
      new Error('Database connection failed')
    );

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(false);
    expect(result.registeredCount).toBe(0);
    expect(result.error).toBe('Database connection failed');
  });

  it('should handle empty schema list', async () => {
    vi.mocked(schemaService.getAllSchemas).mockResolvedValue([]);

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(true);
    expect(result.registeredCount).toBe(0);
  });

  it('should parallelize schema registration for performance', async () => {
    const schemas: Array<SchemaDefinition & { id: string }> = Array.from({ length: 10 }, (_, i) => ({
      id: `entity${i}`,
      isCore: false,
      version: 1,
      description: `Entity ${i}`,
      fields: []
    }));

    vi.mocked(schemaService.getAllSchemas).mockResolvedValue(schemas);
    vi.mocked(schemaService.getSchema).mockImplementation(async (id) => {
      // Simulate async delay
      await new Promise((resolve) => setTimeout(resolve, 10));
      const schema = schemas.find((s) => s.id === id);
      if (!schema) throw new Error('Schema not found');
      const { id: _id, ...schemaWithoutId } = schema;
      return schemaWithoutId;
    });

    const startTime = Date.now();
    const result = await initializeSchemaPluginSystem();
    const duration = Date.now() - startTime;

    expect(result.success).toBe(true);
    expect(result.registeredCount).toBe(10);

    // With parallel execution, should complete in ~10-20ms, not 100ms (10 * 10ms)
    // Allow generous margin for CI/CD environments
    expect(duration).toBeLessThan(100);
  });

  it('should handle both camelCase and snake_case core type filtering', async () => {
    const schemas: Array<(SchemaDefinition & { id: string }) | (SchemaDefinition & { id: string; is_core: boolean })> = [
      {
        id: 'text',
        isCore: true, // camelCase
        version: 1,
        description: 'Text',
        fields: []
      },
      {
        id: 'task',
        is_core: true, // snake_case from Rust
        isCore: false,
        version: 1,
        description: 'Task',
        fields: []
      },
      {
        id: 'invoice',
        isCore: false,
        version: 1,
        description: 'Invoice',
        fields: []
      }
    ];

    vi.mocked(schemaService.getAllSchemas).mockResolvedValue(schemas as Array<SchemaDefinition & { id: string }>);
    vi.mocked(schemaService.getSchema).mockImplementation(async (id) => {
      const schema = schemas.find((s) => s.id === id);
      if (!schema) throw new Error('Schema not found');
      const { id: _id, ...schemaWithoutId } = schema;
      return schemaWithoutId as SchemaDefinition;
    });

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(true);
    expect(result.registeredCount).toBe(1); // Only invoice
    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);
    expect(pluginRegistry.hasPlugin('text')).toBe(false);
    expect(pluginRegistry.hasPlugin('task')).toBe(false);
  });
});

describe('Schema Plugin Loader - PLUGIN_PRIORITIES', () => {
  it('should define correct priority hierarchy', () => {
    expect(PLUGIN_PRIORITIES.CORE).toBe(100);
    expect(PLUGIN_PRIORITIES.CUSTOM_ENTITY).toBe(50);
    expect(PLUGIN_PRIORITIES.USER_COMMAND).toBe(0);

    // Verify ordering
    expect(PLUGIN_PRIORITIES.CORE).toBeGreaterThan(PLUGIN_PRIORITIES.CUSTOM_ENTITY);
    expect(PLUGIN_PRIORITIES.CUSTOM_ENTITY).toBeGreaterThan(
      PLUGIN_PRIORITIES.USER_COMMAND
    );
  });
});

describe('Schema Plugin Loader - Integration Tests', () => {
  beforeEach(() => {
    pluginRegistry.clear();
    vi.clearAllMocks();
  });

  afterEach(() => {
    pluginRegistry.clear();
  });

  it('should complete full registration flow: schema → plugin → slash command', async () => {
    const schema: SchemaDefinition = {
      isCore: false,
      version: 1,
      description: 'Sales Invoice',
      fields: []
    };

    vi.mocked(schemaService.getSchema).mockResolvedValue(schema);

    // Register schema as plugin
    await registerSchemaPlugin('invoice');

    // Verify plugin exists
    expect(pluginRegistry.hasPlugin('invoice')).toBe(true);

    // Verify slash command configuration
    const plugin = pluginRegistry.getPlugin('invoice');
    expect(plugin?.config.slashCommands).toHaveLength(1);
    expect(plugin?.config.slashCommands[0]).toMatchObject({
      id: 'invoice',
      name: 'Sales Invoice',
      nodeType: 'invoice',
      priority: PLUGIN_PRIORITIES.CUSTOM_ENTITY
    });
  });

  it('should handle full startup initialization flow', async () => {
    const schemas: Array<SchemaDefinition & { id: string }> = [
      { id: 'text', isCore: true, version: 1, description: 'Text', fields: [] },
      { id: 'task', isCore: true, version: 1, description: 'Task', fields: [] },
      { id: 'invoice', isCore: false, version: 1, description: 'Invoice', fields: [] },
      { id: 'customer', isCore: false, version: 1, description: 'Customer', fields: [] }
    ];

    vi.mocked(schemaService.getAllSchemas).mockResolvedValue(schemas);
    vi.mocked(schemaService.getSchema).mockImplementation(async (id) => {
      const schema = schemas.find((s) => s.id === id);
      if (!schema) throw new Error('Schema not found');
      const { id: _id, ...schemaWithoutId } = schema;
      return schemaWithoutId;
    });

    const result = await initializeSchemaPluginSystem();

    expect(result.success).toBe(true);
    expect(result.registeredCount).toBe(2);

    // Verify only custom entities are registered
    const allPlugins = pluginRegistry.getAllPlugins();
    const customEntityPlugins = allPlugins.filter((p) =>
      ['invoice', 'customer'].includes(p.id)
    );

    expect(customEntityPlugins).toHaveLength(2);
  });
});
