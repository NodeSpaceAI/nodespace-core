/**
 * Slash Command Type Persistence Tests
 *
 * Tests that node type conversions via slash commands persist correctly to the database.
 * This is distinct from pattern-based conversions (typing "## " or "[ ] ").
 *
 * THE BUG:
 * - User types /task on placeholder node → ReactiveNodeService.updateNodeType() called
 * - UI shows task node correctly (blue dot icon)
 * - User types "Buy groceries" → ReactiveNodeService.updateNodeContent() called
 * - updateNodeContent() only sends { content }, NOT { content, nodeType }
 * - Database persists as text node with truncated content "[ ] "
 * - Page reload shows text node, user work is lost
 *
 * These tests simulate the ACTUAL flow (separate calls to updateNodeType and updateNodeContent)
 * unlike the existing tests which call updateNode() with both fields together.
 *
 * Part of issue #424: Fix node type conversions persistence
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createReactiveNodeService } from '../../lib/services/reactive-node-service.svelte';
import { SharedNodeStore } from '../../lib/services/shared-node-store';
import { PersistenceCoordinator } from '../../lib/services/persistence-coordinator.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';
import { schemaService } from '../../lib/services/schema-service';
import type { SchemaDefinition } from '../../lib/types/schema';

/**
 * Helper to wait for debounce + persistence
 */
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Mock task schema for tests
 */
const mockTaskSchema: SchemaDefinition = {
  isCore: true,
  version: 1,
  description: 'Task node schema',
  fields: [
    {
      name: 'status',
      type: 'enum',
      protection: 'core',
      default: 'todo',
      indexed: true,
      required: false,
      coreValues: ['todo', 'in-progress', 'done'],
      description: 'Task status'
    },
    {
      name: 'priority',
      type: 'enum',
      protection: 'core',
      default: 'medium',
      indexed: true,
      required: false,
      coreValues: ['low', 'medium', 'high'],
      description: 'Task priority'
    }
  ]
};

// Empty schema definition for node types without schemas
const emptySchema: SchemaDefinition = {
  isCore: false,
  version: 1,
  description: 'Empty schema for testing',
  fields: []
};

// Mock the schema service's methods for testing
vi.spyOn(schemaService, 'getSchema').mockImplementation(async (schemaId: string) => {
  if (schemaId === 'task') {
    return mockTaskSchema;
  }
  // Return empty schema for other types instead of throwing
  // This prevents console warnings during tests
  return emptySchema;
});

// Mock extractDefaults to return the schema defaults immediately (synchronous)
vi.spyOn(schemaService, 'extractDefaults').mockImplementation((schemaId: string) => {
  if (schemaId === 'task') {
    // Return defaults in the same format as the real implementation
    return {
      task: {
        status: 'todo',
        priority: 'medium'
      }
    };
  }
  // Graceful degradation for other schema types (no defaults)
  return {};
});

describe('Slash Command Type Persistence', () => {
  let reactiveService: ReturnType<typeof createReactiveNodeService>;
  let store: SharedNodeStore;
  let coordinator: PersistenceCoordinator;

  const viewerSource: UpdateSource = {
    type: 'viewer',
    viewerId: 'test-viewer-1'
  };

  beforeEach(() => {
    // Reset and initialize services
    SharedNodeStore.resetInstance();
    PersistenceCoordinator.resetInstance();

    store = SharedNodeStore.getInstance();
    coordinator = PersistenceCoordinator.getInstance();
    reactiveService = createReactiveNodeService({
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    });

    // Enable test mode to skip actual database operations
    coordinator.enableTestMode();
    coordinator.resetTestState();
  });

  afterEach(async () => {
    // Clean up
    store.clearAll();
    SharedNodeStore.resetInstance();

    await coordinator.reset();
    PersistenceCoordinator.resetInstance();
  });

  describe('Slash Command: Placeholder → Task', () => {
    it('should persist nodeType when slash command is used BEFORE content is added', async () => {
      // Step 1: Create placeholder node (like pressing Enter)
      const placeholderNode: Node = {
        id: 'slash-test-1',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(placeholderNode, viewerSource);

      // Step 2: User types "/task" → SlashCommandService calls updateNodeType()
      reactiveService.updateNodeType(placeholderNode.id, 'task');

      // Verify UI shows task node immediately
      let node = store.getNode(placeholderNode.id);
      expect(node?.nodeType).toBe('task');
      expect(node?.content).toBe(''); // Still empty

      // Step 3: User types content → TextareaController calls updateNodeContent()
      await sleep(50); // Simulate user thinking/typing delay
      reactiveService.updateNodeContent(placeholderNode.id, 'Buy milk');

      // Verify UI shows full content
      node = store.getNode(placeholderNode.id);
      expect(node?.nodeType).toBe('task');
      expect(node?.content).toBe('Buy milk');

      // Step 4: Wait for debounce + persistence
      await sleep(600);

      // Step 5: Verify persistence (in test mode, check store state)
      const persisted = store.getNode(placeholderNode.id);
      expect(persisted?.nodeType).toBe('task');
      expect(persisted?.content).toBe('Buy milk');
      // TODO: Schema defaults not yet implemented - will be added in separate issue
      // expect(persisted?.properties).toHaveProperty('task');
    });

    it('should persist nodeType even with zero delay between calls', async () => {
      // Stress test: verify synchronous store updates work correctly
      // with NO delay between updateNodeType and updateNodeContent
      const placeholderNode: Node = {
        id: 'slash-test-2',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(placeholderNode, viewerSource);

      // Convert to task via slash command
      reactiveService.updateNodeType(placeholderNode.id, 'task');
      // Immediate content update with ZERO delay (stress test)
      reactiveService.updateNodeContent(placeholderNode.id, 'Buy milk');

      // Verify UI shows correct state immediately
      let node = store.getNode(placeholderNode.id);
      expect(node?.nodeType).toBe('task');
      expect(node?.content).toBe('Buy milk');

      // Wait for debounce + persistence
      await sleep(600);

      // Verify persistence preserved both fields
      const persisted = store.getNode(placeholderNode.id);
      expect(persisted?.nodeType).toBe('task');
      expect(persisted?.content).toBe('Buy milk');
    });

    it('should preserve nodeType through multiple content updates', async () => {
      const placeholderNode: Node = {
        id: 'slash-test-3',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(placeholderNode, viewerSource);

      // Convert to task via slash command
      reactiveService.updateNodeType(placeholderNode.id, 'task');

      // User types content incrementally (simulating real typing)
      await sleep(50);
      reactiveService.updateNodeContent(placeholderNode.id, 'Buy ');
      await sleep(50);
      reactiveService.updateNodeContent(placeholderNode.id, 'Buy milk');
      await sleep(50);
      reactiveService.updateNodeContent(placeholderNode.id, 'Buy milk and eggs');

      // Wait for final debounce
      await sleep(600);

      // Verify nodeType survived all content updates
      const persisted = store.getNode(placeholderNode.id);
      expect(persisted?.nodeType).toBe('task');
      expect(persisted?.content).toBe('Buy milk and eggs');
    });
  });

  describe('Slash Command: Placeholder → Header', () => {
    it('should persist header nodeType from slash command', async () => {
      const placeholderNode: Node = {
        id: 'slash-test-4',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(placeholderNode, viewerSource);

      // Convert to header via slash command (e.g., /h1)
      reactiveService.updateNodeType(placeholderNode.id, 'header');

      await sleep(50);
      reactiveService.updateNodeContent(placeholderNode.id, '# My Title');

      await sleep(600);

      const persisted = store.getNode(placeholderNode.id);
      expect(persisted?.nodeType).toBe('header');
      expect(persisted?.content).toBe('# My Title');
    });
  });

  describe('Pattern Detection: Still Works', () => {
    it('should allow pattern detection to work (typing "[ ] ")', async () => {
      // This test ensures we don't break the existing pattern conversion flow
      const textNode: Node = {
        id: 'pattern-test-1',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewerSource);

      // User types "[ ] " which triggers pattern detection
      // Pattern detection will call updateNodeType() separately
      reactiveService.updateNodeContent(textNode.id, '[ ] ');

      // Simulate pattern detection event (normally fired by TextareaController)
      reactiveService.updateNodeType(textNode.id, 'task');

      // User continues typing
      await sleep(50);
      reactiveService.updateNodeContent(textNode.id, '[ ] Buy groceries');

      await sleep(600);

      const persisted = store.getNode(textNode.id);
      expect(persisted?.nodeType).toBe('task');
      expect(persisted?.content).toBe('[ ] Buy groceries');
    });

    it('should allow bidirectional pattern conversion (header ↔ text)', async () => {
      const textNode: Node = {
        id: 'pattern-test-2',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(textNode, viewerSource);

      // Pattern: type "## " → becomes header
      reactiveService.updateNodeContent(textNode.id, '## ');
      reactiveService.updateNodeType(textNode.id, 'header');
      await sleep(50);
      reactiveService.updateNodeContent(textNode.id, '## Hello');

      await sleep(600);

      let persisted = store.getNode(textNode.id);
      expect(persisted?.nodeType).toBe('header');

      // Pattern: remove "## " → becomes text again
      reactiveService.updateNodeContent(textNode.id, 'Hello');
      reactiveService.updateNodeType(textNode.id, 'text');

      await sleep(600);

      persisted = store.getNode(textNode.id);
      expect(persisted?.nodeType).toBe('text');
      expect(persisted?.content).toBe('Hello');
    });
  });

  describe('Content Not Truncated', () => {
    it('should persist full content, not just the pattern prefix', async () => {
      const placeholderNode: Node = {
        id: 'truncation-test-1',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(placeholderNode, viewerSource);

      // Slash command conversion
      reactiveService.updateNodeType(placeholderNode.id, 'task');

      // Add substantial content
      await sleep(50);
      const fullContent = 'Buy groceries from the store: milk, eggs, bread, cheese';
      reactiveService.updateNodeContent(placeholderNode.id, fullContent);

      await sleep(600);

      const persisted = store.getNode(placeholderNode.id);
      expect(persisted?.content).toBe(fullContent); // ❌ CURRENTLY TRUNCATED
      expect(persisted?.content.length).toBe(fullContent.length);
    });
  });

  describe('Schema Defaults Applied', () => {
    it('should apply task schema defaults when converting to task', async () => {
      // Issue #427: Schema defaults are now applied when converting node types
      const placeholderNode: Node = {
        id: 'schema-test-1',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(placeholderNode, viewerSource);

      // Convert to task - should apply schema defaults immediately
      reactiveService.updateNodeType(placeholderNode.id, 'task');

      // Check if defaults were applied immediately (synchronous)
      let node = store.getNode(placeholderNode.id);

      expect(node?.properties).toHaveProperty('task');
      expect((node?.properties.task as Record<string, unknown>)).toHaveProperty('status');
      expect((node?.properties.task as Record<string, unknown>).status).toBe('todo');
      expect((node?.properties.task as Record<string, unknown>)).toHaveProperty('priority');
      expect((node?.properties.task as Record<string, unknown>).priority).toBe('medium');

      // Update content and verify defaults persist
      await sleep(50);
      reactiveService.updateNodeContent(placeholderNode.id, 'My task');
      await sleep(600);

      const persisted = store.getNode(placeholderNode.id);

      expect(persisted?.properties).toHaveProperty('task');
      expect((persisted?.properties.task as Record<string, unknown>)).toHaveProperty('status');
      expect((persisted?.properties.task as Record<string, unknown>).status).toBe('todo');
      expect((persisted?.properties.task as Record<string, unknown>)).toHaveProperty('priority');
      expect((persisted?.properties.task as Record<string, unknown>).priority).toBe('medium');
    });

    it('should merge defaults with existing properties without overwriting', () => {
      // Test that existing properties take precedence over defaults
      const nodeWithProperties: Node = {
        id: 'merge-test-1',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {
          task: {
            status: 'in-progress', // User-set value (should NOT be overwritten)
            customField: 'my custom data'
          }
        },
        mentions: []
      };

      store.setNode(nodeWithProperties, viewerSource);

      // Convert to task - should merge defaults without overwriting
      reactiveService.updateNodeType(nodeWithProperties.id, 'task');

      const node = store.getNode(nodeWithProperties.id);

      // User's status should be preserved
      expect((node?.properties.task as Record<string, unknown>).status).toBe('in-progress');
      // Default priority should be added
      expect((node?.properties.task as Record<string, unknown>).priority).toBe('medium');
      // Custom field should be preserved
      expect((node?.properties.task as Record<string, unknown>).customField).toBe('my custom data');
    });

    it('should handle node types without schemas gracefully', () => {
      // Test that converting to a type without a schema works without errors
      const placeholderNode: Node = {
        id: 'no-schema-test-1',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(placeholderNode, viewerSource);

      // Convert to a type without a schema (mock returns empty)
      reactiveService.updateNodeType(placeholderNode.id, 'header');

      const node = store.getNode(placeholderNode.id);

      // Should still update nodeType
      expect(node?.nodeType).toBe('header');
      // Properties should remain empty (no defaults available)
      expect(node?.properties).toEqual({});
    });

    it('should work with empty existing properties namespace', () => {
      // Test that defaults are applied when properties object exists but namespace is empty
      const nodeWithEmptyNamespace: Node = {
        id: 'empty-namespace-test-1',
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {
          someOtherNamespace: {
            data: 'unrelated'
          }
        },
        mentions: []
      };

      store.setNode(nodeWithEmptyNamespace, viewerSource);

      // Convert to task
      reactiveService.updateNodeType(nodeWithEmptyNamespace.id, 'task');

      const node = store.getNode(nodeWithEmptyNamespace.id);

      // Should have schema defaults for task
      expect(node?.properties).toHaveProperty('task');
      expect((node?.properties.task as Record<string, unknown>)).toHaveProperty('status', 'todo');
      expect((node?.properties.task as Record<string, unknown>)).toHaveProperty('priority', 'medium');
      // Should preserve other namespaces
      expect((node?.properties.someOtherNamespace as Record<string, unknown>)).toEqual({
        data: 'unrelated'
      });
    });
  });
});
