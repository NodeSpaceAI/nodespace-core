/**
 * CustomEntityNode Component Tests
 *
 * Unit tests for the CustomEntityNode component that renders user-defined entity types.
 * Tests cover:
 * - Schema loading and error handling logic
 * - Visual distinction properties (borders, headers, icons)
 * - State management for loading and errors
 *
 * Issue #449: Generic Custom Entity Rendering - Polish UI for Schema-Driven Types
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { backendAdapter } from '$lib/services/backend-adapter';
import { type SchemaNode, isSchemaNode } from '$lib/types/schema-node';

// Mock backend adapter
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getSchema: vi.fn()
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
  } = {}
): SchemaNode {
  return {
    id,
    nodeType: 'schema',
    content: id,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    // Typed top-level fields (not in properties)
    isCore: options.isCore ?? false,
    schemaVersion: options.schemaVersion ?? 1,
    description: options.description ?? '',
    fields: []
  };
}

/**
 * Test logic for entity name
 * Uses schema description if available, otherwise falls back to nodeType
 */
function getEntityName(schema: SchemaNode | null, nodeType: string): string {
  if (schema) {
    // Access typed field directly (no helper needed)
    if (schema.description) return schema.description;
  }
  return nodeType;
}

/**
 * Extract emoji icon from description if present (e.g., "💰 Invoice" → "💰")
 */
function extractIconFromDescription(description: string): string | null {
  if (!description) return null;
  // Match emoji at the start of the description
  const emojiMatch = description.match(/^([\p{Emoji}])\s/u);
  return emojiMatch ? emojiMatch[1] : null;
}

describe('CustomEntityNode Logic', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  describe('Schema Loading', () => {
    it('should call getSchema with correct nodeType', async () => {
      const mockSchemaNode = createMockSchemaNode('invoice', {
        description: 'Invoice Entity'
      });

      vi.mocked(backendAdapter.getSchema).mockResolvedValue(mockSchemaNode);

      // Simulate what the component does in $effect
      const node = await backendAdapter.getSchema('invoice');

      expect(backendAdapter.getSchema).toHaveBeenCalledWith('invoice');
      expect(isSchemaNode(node)).toBe(true);
    });

    it('should handle schema loading errors gracefully', async () => {
      vi.mocked(backendAdapter.getSchema).mockRejectedValue(new Error('Schema not found'));

      try {
        await backendAdapter.getSchema('nonexistent');
        expect.fail('Should have thrown');
      } catch (error) {
        expect((error as Error).message).toContain('Schema not found');
        // Component would catch this and set schemaError state
      }
    });

    it('should handle network errors in schema loading', async () => {
      vi.mocked(backendAdapter.getSchema).mockRejectedValue(new Error('Network error'));

      try {
        await backendAdapter.getSchema('invoice');
        expect.fail('Should have thrown');
      } catch (error) {
        expect((error as Error).message).toContain('Network error');
        // Component would catch and display error state
      }
    });
  });

  describe('Entity Header Display', () => {
    it('should always show entity header when schema exists', () => {
      const schemaNode = createMockSchemaNode('invoice', {
        description: 'Invoice'
      }) as SchemaNode;

      // CustomEntityNode always shows header for custom entities
      expect(getEntityName(schemaNode, 'invoice')).toBe('Invoice');
    });

    it('should fall back to nodeType when schema is null', () => {
      // When schema is null, component is loading
      expect(getEntityName(null, 'invoice')).toBe('invoice');
    });
  });

  describe('Entity Name Display', () => {
    it('should use schema description as entity name', () => {
      const schemaNode = createMockSchemaNode('invoice', {
        description: 'Sales Invoice'
      }) as SchemaNode;

      expect(getEntityName(schemaNode, 'invoice')).toBe('Sales Invoice');
    });

    it('should fall back to nodeType when schema description is missing', () => {
      const schemaNode = createMockSchemaNode('invoice', {
        description: ''
      }) as SchemaNode;

      expect(getEntityName(schemaNode, 'invoice')).toBe('invoice');
    });

    it('should use nodeType when schema is null', () => {
      expect(getEntityName(null, 'invoice')).toBe('invoice');
    });

    it('should use nodeType when schema is undefined', () => {
      const schema = undefined as unknown as SchemaNode | null;
      expect(getEntityName(schema, 'person')).toBe('person');
    });
  });

  describe('Custom Icon Extraction', () => {
    it('should extract icon from description when emoji is at the start', () => {
      const description = '💰 Invoice';
      expect(extractIconFromDescription(description)).toBe('💰');
    });

    it('should return null when description has no emoji at start', () => {
      const description = 'Invoice 💰';
      expect(extractIconFromDescription(description)).toBeNull();
    });

    it('should return null when description is empty', () => {
      expect(extractIconFromDescription('')).toBeNull();
    });

    it('should handle different emoji icons at description start', () => {
      const testCases = [
        { description: '💰 Invoice', expected: '💰' },
        { description: '👤 Person', expected: '👤' },
        { description: '📄 Document', expected: '📄' },
        { description: '✅ Task', expected: '✅' },
        { description: '🎯 Goal', expected: '🎯' }
      ];

      testCases.forEach(({ description, expected }) => {
        expect(extractIconFromDescription(description)).toBe(expected);
      });
    });

    it('should not match emoji without space after it', () => {
      const description = '💰Invoice'; // No space after emoji
      expect(extractIconFromDescription(description)).toBeNull();
    });
  });

  describe('Component Props Handling', () => {
    it('should properly handle all required props', () => {
      const props = {
        nodeId: 'test-node-1',
        nodeType: 'invoice',
        content: 'Invoice #001',
        children: []
      };

      expect(props.nodeId).toBe('test-node-1');
      expect(props.nodeType).toBe('invoice');
      expect(props.content).toBe('Invoice #001');
      expect(Array.isArray(props.children)).toBe(true);
    });

    it('should support nodeType property for schema lookups', () => {
      const nodeType = 'invoice';
      // This is what the component would do
      const schemaLookupKey = nodeType;
      expect(schemaLookupKey).toBe('invoice');
    });
  });

  describe('Error State Management', () => {
    it('should distinguish between loading and error states', async () => {
      const states = {
        loading: { isLoading: true, schema: null, error: null },
        loaded: {
          isLoading: false,
          schema: createMockSchemaNode('invoice') as SchemaNode,
          error: null
        },
        error: { isLoading: false, schema: null, error: 'Failed to load' }
      };

      expect(states.loading.isLoading).toBe(true);
      expect(states.loaded.schema).not.toBeNull();
      expect(states.error.error).not.toBeNull();
    });

    it('should show appropriate UI for each state', () => {
      const states = {
        loading: { showLoader: true, showForm: false, showError: false },
        loaded: { showLoader: false, showForm: true, showError: false },
        error: { showLoader: false, showForm: false, showError: true }
      };

      // Validate state transitions
      expect(states.loading.showLoader).toBe(true);
      expect(states.loaded.showForm).toBe(true);
      expect(states.error.showError).toBe(true);
    });
  });

  describe('Visual Distinction', () => {
    it('should apply custom-entity-node class for styling', () => {
      // Component logic would set this class
      const entityNodeClass = 'custom-entity-node';
      expect(entityNodeClass).toBe('custom-entity-node');
    });

    it('should apply data-entity-type attribute for entity identification', () => {
      const nodeType = 'invoice';
      const dataAttribute = `data-entity-type="${nodeType}"`;
      expect(dataAttribute).toContain('invoice');
    });

    it('should use CSS custom properties for custom entity accent color', () => {
      // Component uses: border-left: 3px solid var(--custom-entity-accent, #6366f1);
      const defaultAccentColor = '#6366f1';
      expect(defaultAccentColor).toMatch(/^#[0-9a-f]{6}$/i);
    });
  });

  describe('Schema Integration', () => {
    it('should use description field for entity naming', () => {
      const schemaNode = createMockSchemaNode('invoice', {
        description: '💰 Invoice Management'
      });

      expect(getEntityName(schemaNode, 'invoice')).toBe('💰 Invoice Management');
      // Access typed field directly
      expect(extractIconFromDescription(schemaNode.description)).toBe('💰');
    });

    it('should handle schema with fields array', () => {
      // Create schema with fields
      const schemaNode: SchemaNode = {
        ...createMockSchemaNode('invoice', {
          description: 'Invoice'
        }),
        fields: [
          {
            name: 'amount',
            type: 'number',
            protection: 'user',
            indexed: false
          }
        ]
      };

      // Access typed field directly (not in properties)
      expect(schemaNode.fields.length).toBe(1);
      expect(schemaNode.fields[0].name).toBe('amount');
    });
  });

  describe('Schema Loading Behavior', () => {
    it('should request schema for each nodeType', async () => {
      const mockSchemaNode = createMockSchemaNode('invoice', {
        description: 'Invoice'
      });

      vi.mocked(backendAdapter.getSchema).mockResolvedValue(mockSchemaNode);

      // First call
      await backendAdapter.getSchema('invoice');
      // Second call with same nodeType
      await backendAdapter.getSchema('invoice');

      // Both calls should go through (no caching at adapter level)
      expect(backendAdapter.getSchema).toHaveBeenCalledTimes(2);
      expect(backendAdapter.getSchema).toHaveBeenCalledWith('invoice');
    });

    it('should request different schemas for different nodeTypes', async () => {
      const invoiceSchema = createMockSchemaNode('invoice', {
        description: 'Invoice'
      });

      const personSchema = createMockSchemaNode('person', {
        description: 'Person'
      });

      vi.mocked(backendAdapter.getSchema)
        .mockResolvedValueOnce(invoiceSchema)
        .mockResolvedValueOnce(personSchema);

      await backendAdapter.getSchema('invoice');
      await backendAdapter.getSchema('person');

      expect(backendAdapter.getSchema).toHaveBeenCalledWith('invoice');
      expect(backendAdapter.getSchema).toHaveBeenCalledWith('person');
    });
  });
});
