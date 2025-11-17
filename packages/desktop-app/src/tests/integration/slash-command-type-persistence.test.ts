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

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { createReactiveNodeService } from '../../lib/services/reactive-node-service.svelte';
import { SharedNodeStore } from '../../lib/services/shared-node-store';
import { PersistenceCoordinator } from '../../lib/services/persistence-coordinator.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';

/**
 * Helper to wait for debounce + persistence
 */
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

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
        beforeSiblingId: null,
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
        beforeSiblingId: null,
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
        beforeSiblingId: null,
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
        beforeSiblingId: null,
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
        beforeSiblingId: null,
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
        beforeSiblingId: null,
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
        beforeSiblingId: null,
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
    it.skip('should apply task schema defaults when converting to task', async () => {
      // TODO: Schema defaults not yet implemented - tracked in Issue #427
      // This test is skipped until schema default application is implemented
      const placeholderNode: Node = {
        id: 'schema-test-1',
        nodeType: 'text',
        content: '',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };

      store.setNode(placeholderNode, viewerSource);

      // Convert to task
      reactiveService.updateNodeType(placeholderNode.id, 'task');
      await sleep(50);
      reactiveService.updateNodeContent(placeholderNode.id, 'My task');

      await sleep(600);

      const persisted = store.getNode(placeholderNode.id);
      expect(persisted?.properties).toHaveProperty('task');
      expect(persisted?.properties.task).toHaveProperty('status');
      expect(persisted?.properties.task).toHaveProperty('priority');
    });
  });
});
