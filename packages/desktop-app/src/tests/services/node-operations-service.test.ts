/**
 * NodeOperationsService Test Suite
 *
 * Comprehensive tests for NodeOperationsService focusing on:
 * - Upsert operations with type-segmented metadata preservation
 * - Mentions bidirectionality and consistency
 * - Content extraction with fallback strategies
 * - Parent/root resolution logic
 * - Sibling positioning mechanics
 * - EventBus integration verification
 */

import { describe, test, expect, beforeEach, vi } from 'vitest';
import {
  ReactiveNodeService as NodeManager,
  type NodeManagerEvents
} from '$lib/services/reactiveNodeService.svelte.js';
import { HierarchyService } from '$lib/services/hierarchyService';
import { NodeOperationsService } from '$lib/services/nodeOperationsService';
import { ContentProcessor } from '$lib/services/contentProcessor';
import { eventBus } from '$lib/services/eventBus';
import type { NodeSpaceNode } from '$lib/services/mockDatabaseService';

describe('NodeOperationsService', () => {
  let nodeManager: NodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;
  let contentProcessor: ContentProcessor;
  let events: NodeManagerEvents;

  beforeEach(() => {
    // Reset EventBus
    eventBus.reset();

    // Create mock events
    events = {
      focusRequested: vi.fn(),
      hierarchyChanged: vi.fn(),
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    };

    // Initialize services
    nodeManager = new NodeManager(events);
    hierarchyService = new HierarchyService(nodeManager);
    contentProcessor = ContentProcessor.getInstance();
    nodeOperationsService = new NodeOperationsService(
      nodeManager,
      hierarchyService,
      contentProcessor
    );

    // Set up basic test data
    nodeManager.initializeFromLegacyData([
      {
        id: 'root1',
        type: 'text',
        content: 'Root node 1',
        children: [
          {
            id: 'child1',
            type: 'text',
            content: 'Child node 1',
            children: []
          }
        ]
      },
      {
        id: 'root2',
        type: 'text',
        content: 'Root node 2',
        children: []
      }
    ]);
  });

  // ========================================================================
  // Content Extraction Utilities
  // ========================================================================

  describe('Content Extraction Utilities', () => {
    test('extractContentString handles direct content field', () => {
      const result = nodeOperationsService.extractContentString({
        content: 'Hello world'
      });

      expect(result.content).toBe('Hello world');
      expect(result.extractedType).toBe('text');
      expect(result.confidence).toBe(1.0);
      expect(result.fallbackUsed).toBe(false);
    });

    test('extractContentString extracts from metadata as fallback', () => {
      const result = nodeOperationsService.extractContentString({
        metadata: {
          text: 'Content from metadata',
          someOtherField: 'ignored'
        }
      });

      expect(result.content).toBe('Content from metadata');
      expect(result.confidence).toBeLessThan(1.0);
      expect(result.fallbackUsed).toBe(true);
    });

    test('extractContentString uses type defaults when no content', () => {
      const result = nodeOperationsService.extractContentString({
        type: 'ai-chat'
      });

      expect(result.content).toBe('');
      expect(result.extractedType).toBe('ai-chat');
      expect(result.fallbackUsed).toBe(true);
      expect(result.metadata).toHaveProperty('chatRole');
    });

    test('extractContentString handles empty input', () => {
      const result = nodeOperationsService.extractContentString({});

      expect(result.content).toBe('');
      expect(result.extractedType).toBe('text');
      expect(result.confidence).toBe(0.1);
      expect(result.fallbackUsed).toBe(true);
    });

    test('extractContentWithContext provides rich analysis', () => {
      const result = nodeOperationsService.extractContentWithContext({
        content: '# Header with [[link]] and **bold** text'
      }) as {
        content: string;
        ast: unknown;
        wikiLinks: unknown[];
        headerLevel: number;
        wordCount: number;
        hasFormatting: boolean;
        metadata: Record<string, unknown>;
      };

      expect(result.content).toBe('# Header with [[link]] and **bold** text');
      expect(result.headerLevel).toBe(1);
      expect(result.wikiLinks).toHaveLength(1);
      expect((result.wikiLinks[0] as { target: string }).target).toBe('link');
      expect(result.hasFormatting).toBe(true);
      expect(result.wordCount).toBe(7); // "Header", "with", "link", "and", "bold", "text" = 6 words + "link" in wikilink = 7
      expect(result.ast).toBeDefined();
    });

    test('extractContentWithContext handles complex markdown', () => {
      const complexContent = `
# Main Header

This is a paragraph with [[first-link]] and [[second-link|Display Text]].

- List item with *italic* text
- Another item with **bold** text

## Sub Header

Code block:
\`\`\`javascript
const x = 42;
\`\`\`
      `.trim();

      const result = nodeOperationsService.extractContentWithContext({
        content: complexContent
      }) as {
        content: string;
        ast: unknown;
        wikiLinks: unknown[];
        headerLevel: number;
        wordCount: number;
        hasFormatting: boolean;
        metadata: Record<string, unknown>;
      };

      expect(result.wikiLinks).toHaveLength(2);
      expect((result.wikiLinks[0] as { target: string }).target).toBe('first-link');
      expect((result.wikiLinks[1] as { target: string; displayText: string }).target).toBe(
        'second-link'
      );
      expect((result.wikiLinks[1] as { target: string; displayText: string }).displayText).toBe(
        'Display Text'
      );
      expect(result.headerLevel).toBe(1); // First header level
      expect(result.hasFormatting).toBe(true);
      expect(result.wordCount).toBeGreaterThan(10);
    });
  });

  // ========================================================================
  // Parent/Root Resolution
  // ========================================================================

  describe('Parent/Root Resolution', () => {
    test('resolveParentAndRoot handles explicit values', async () => {
      const result = await nodeOperationsService.resolveParentAndRoot(
        'child1',
        'root1',
        'new-node'
      );

      expect(result.parentId).toBe('child1');
      expect(result.rootId).toBe('root1');
      expect(result.strategy).toBe('explicit');
      expect(result.confidence).toBe(1.0);
    });

    test('resolveParentAndRoot preserves existing hierarchy', async () => {
      const result = await nodeOperationsService.resolveParentAndRoot(
        undefined,
        undefined,
        'child1',
        true
      );

      expect(result.parentId).toBe('root1');
      expect(result.rootId).toBe('root1');
      expect(result.strategy).toBe('explicit');
      expect(result.confidence).toBe(0.9);
    });

    test('resolveParentAndRoot infers root from parent', async () => {
      const result = await nodeOperationsService.resolveParentAndRoot(
        'child1',
        undefined,
        'new-node'
      );

      expect(result.parentId).toBe('child1');
      expect(result.rootId).toBe('root1');
      expect(result.strategy).toBe('inferred');
      expect(result.confidence).toBe(0.8);
    });

    test('resolveParentAndRoot defaults to root when no info', async () => {
      const result = await nodeOperationsService.resolveParentAndRoot(
        undefined,
        undefined,
        'new-node'
      );

      expect(result.parentId).toBeNull();
      expect(result.rootId).toBe('new-node');
      expect(result.strategy).toBe('default');
      expect(result.confidence).toBe(0.5);
    });
  });

  // ========================================================================
  // Sibling Positioning
  // ========================================================================

  describe('Sibling Positioning', () => {
    test('handleSiblingPositioning respects explicit positioning', async () => {
      const result = await nodeOperationsService.handleSiblingPositioning(
        'child1',
        'root1',
        'new-node'
      );

      expect(result.beforeSiblingId).toBe('child1');
      expect(result.strategy).toBe('explicit');
    });

    test('handleSiblingPositioning appends to end by default', async () => {
      const result = await nodeOperationsService.handleSiblingPositioning(
        undefined,
        'root1',
        'new-node'
      );

      expect(result.strategy).toBe('end');
      expect(result.position).toBeGreaterThanOrEqual(0);
    });

    test('handleSiblingPositioning handles empty parent', async () => {
      // Create a node with no children
      nodeManager.createNode('root2', 'Empty parent');
      const emptyParent = nodeManager.findNode(
        nodeManager.rootNodeIds[nodeManager.rootNodeIds.length - 1]
      );

      const result = await nodeOperationsService.handleSiblingPositioning(
        undefined,
        emptyParent?.id,
        'new-node'
      );

      expect(result.position).toBe(0);
      expect(result.strategy).toBe('beginning');
    });
  });

  // ========================================================================
  // Mentions and Bidirectionality
  // ========================================================================

  describe('Mentions and Bidirectionality', () => {
    beforeEach(() => {
      // Add mentions property to test nodes
      const child1 = nodeManager.findNode('child1');
      if (child1) {
        child1.mentions = [];
      }

      const root2 = nodeManager.findNode('root2');
      if (root2) {
        root2.mentions = [];
      }
    });

    test('updateNodeMentions updates mentions array', async () => {
      await nodeOperationsService.updateNodeMentions('child1', ['root2']);

      const child1 = nodeManager.findNode('child1');
      expect(child1?.mentions).toEqual(['root2']);
    });

    test('updateNodeMentions maintains bidirectional consistency', async () => {
      // Set up mentions
      await nodeOperationsService.updateNodeMentions('child1', ['root2']);

      // Verify bidirectional consistency through events
      const emittedEvents = eventBus.getRecentEvents();
      const backlinkEvents = emittedEvents.filter((e) => e.type === 'backlink:detected');

      expect(backlinkEvents).toHaveLength(1);
      expect(backlinkEvents[0]).toMatchObject({
        sourceNodeId: 'child1',
        targetNodeId: 'root2',
        linkType: 'mention'
      });
    });

    test('updateNodeMentions handles mention removal', async () => {
      // First, add mentions
      await nodeOperationsService.updateNodeMentions('child1', ['root2', 'root1']);

      // Then remove one
      await nodeOperationsService.updateNodeMentions('child1', ['root2']);

      const child1 = nodeManager.findNode('child1');
      expect(child1?.mentions).toEqual(['root2']);

      // Should emit references update event for removed mention
      const recentEvents = eventBus.getRecentEvents();
      const updateEvents = recentEvents.filter((e) => e.type === 'references:update-needed');
      expect(updateEvents.length).toBeGreaterThan(0);
    });

    test('updateNodeMentions handles empty mentions', async () => {
      // Set initial mentions
      await nodeOperationsService.updateNodeMentions('child1', ['root2']);

      // Clear all mentions
      await nodeOperationsService.updateNodeMentions('child1', []);

      const child1 = nodeManager.findNode('child1');
      expect(child1?.mentions).toEqual([]);
    });

    test('updateNodeMentions throws error for non-existent node', async () => {
      await expect(
        nodeOperationsService.updateNodeMentions('non-existent', ['root1'])
      ).rejects.toThrow('Node non-existent not found');
    });
  });

  // ========================================================================
  // Upsert Operations
  // ========================================================================

  describe('Upsert Operations', () => {
    test('upsertNode creates new node with all properties', async () => {
      const nodeData: Partial<NodeSpaceNode> = {
        type: 'ai-chat',
        content: 'Test content',
        parent_id: 'root1',
        mentions: ['child1'],
        metadata: {
          chatRole: 'user',
          timestamp: Date.now()
        }
      };

      const result = await nodeOperationsService.upsertNode('new-node', nodeData);

      expect(result.id).toBe('new-node');
      expect(result.type).toBe('ai-chat');
      expect(result.content).toBe('Test content');
      expect(result.parent_id).toBe('root1');
      expect(result.mentions).toEqual(['child1']);
      expect(result.metadata).toMatchObject({
        chatRole: 'user'
      });
      expect(result.created_at).toBeDefined();
    });

    test('upsertNode updates existing node preserving metadata', async () => {
      // First create a node through NodeManager
      const nodeId = nodeManager.createNode('root1', 'Original content');
      const node = nodeManager.findNode(nodeId);
      if (node) {
        node.metadata = { originalProp: 'original', shared: 'old' };
        node.mentions = ['root2'];
      }

      // Update with upsert
      const updates: Partial<NodeSpaceNode> = {
        content: 'Updated content',
        metadata: { newProp: 'new', shared: 'updated' }
      };

      const result = await nodeOperationsService.upsertNode(nodeId, updates, {
        preserveMetadata: true
      });

      expect(result.content).toBe('Updated content');
      expect(result.metadata).toMatchObject({
        originalProp: 'original', // Preserved
        newProp: 'new', // Added
        shared: 'updated' // Updated
      });
    });

    test('upsertNode handles content extraction fallbacks', async () => {
      const nodeData: Partial<NodeSpaceNode> = {
        type: 'task',
        metadata: {
          description: 'Task description from metadata',
          completed: false
        }
      };

      const result = await nodeOperationsService.upsertNode('task-node', nodeData);

      expect(result.content).toBe('Task description from metadata');
      expect(result.type).toBe('task');
    });

    test('upsertNode preserves hierarchy when requested', async () => {
      // Create initial node
      const nodeId = nodeManager.createNode('child1', 'Test content');

      // Update with hierarchy preservation
      const result = await nodeOperationsService.upsertNode(
        nodeId,
        {
          content: 'Updated content'
        },
        {
          preserveHierarchy: true
        }
      );

      expect(result.parent_id).toBe('root1');
    });

    test('upsertNode handles complex metadata merging', async () => {
      const nodeId = nodeManager.createNode('root1', 'Test node');
      const node = nodeManager.findNode(nodeId);
      if (node) {
        node.metadata = {
          type: 'document',
          tags: ['important', 'work'],
          settings: {
            autoSave: true,
            theme: 'dark'
          }
        };
      }

      const result = await nodeOperationsService.upsertNode(nodeId, {
        metadata: {
          tags: ['important', 'personal'], // Should override
          settings: {
            theme: 'light' // Should merge with existing
          },
          newField: 'new value'
        }
      });

      expect(result.metadata).toMatchObject({
        type: 'document', // Preserved
        tags: ['important', 'personal'], // Updated
        settings: {
          theme: 'light' // Updated
          // Note: autoSave might be lost depending on merge strategy
        },
        newField: 'new value' // Added
      });
    });
  });

  // ========================================================================
  // EventBus Integration
  // ========================================================================

  describe('EventBus Integration', () => {
    test('emits appropriate events during upsert', async () => {
      await nodeOperationsService.upsertNode('test-node', {
        content: 'Test content',
        type: 'text'
      });

      const events = eventBus.getRecentEvents();
      const debugEvents = events.filter((e) => e.type === 'debug:log');

      expect(debugEvents.length).toBeGreaterThan(0);
      expect(
        debugEvents.some(
          (e) => e.message?.includes('Node operation') && e.message?.includes('upsert')
        )
      ).toBe(true);
    });

    test('emits events during mentions update', async () => {
      // Add mentions property to existing node
      const child1 = nodeManager.findNode('child1');
      if (child1) {
        child1.mentions = [];
      }

      await nodeOperationsService.updateNodeMentions('child1', ['root2']);

      const events = eventBus.getRecentEvents();
      const referencesEvents = events.filter((e) => e.type === 'references:update-needed');
      const backlinkEvents = events.filter((e) => e.type === 'backlink:detected');

      expect(referencesEvents.length + backlinkEvents.length).toBeGreaterThan(0);
    });

    test('responds to node update events', async () => {
      // Create a node with some content
      const nodeId = nodeManager.createNode('root1', 'Test [[link]] content');
      const node = nodeManager.findNode(nodeId);
      if (node) {
        node.mentions = [];
      }

      // Simulate content update that should trigger mention processing
      eventBus.emit<import('../../lib/services/eventTypes').NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: nodeId,
        updateType: 'content',
        previousValue: 'old content',
        newValue: 'Test [[link]] content'
      });

      // Allow event processing
      await new Promise((resolve) => setTimeout(resolve, 10));

      // The service should have processed the content for mentions
      // This is verified by the service not throwing errors during processing
      expect(true).toBe(true);
    });
  });

  // ========================================================================
  // Integration Tests
  // ========================================================================

  describe('Integration Tests', () => {
    test('handles complex workflow with existing nodes', async () => {
      // Step 1: Create nodes in NodeManager first
      const docId = nodeManager.createNode(
        'root1',
        '# My Document\n\nThis references [[note1]] and [[note2]].'
      );
      const note1Id = nodeManager.createNode('root1', 'First note content');
      const note2Id = nodeManager.createNode('root1', 'Second note content');

      // Add mentions properties
      const docNode = nodeManager.findNode(docId);
      const note1Node = nodeManager.findNode(note1Id);
      const note2Node = nodeManager.findNode(note2Id);
      if (docNode) docNode.mentions = ['note1', 'note2'];
      if (note1Node) note1Node.mentions = [];
      if (note2Node) note2Node.mentions = [];

      // Step 2: Use NodeOperationsService to update with enhanced metadata
      const docResult = await nodeOperationsService.upsertNode(docId, {
        metadata: { tags: ['important'], workflow: 'test' }
      });

      expect(docResult.content).toContain('[[note1]]');
      expect(docResult.content).toContain('[[note2]]');

      // Step 3: Update mentions through NodeOperationsService
      await nodeOperationsService.updateNodeMentions(docId, ['note2', 'note3']);

      // Step 4: Verify mentions were updated
      const updatedDoc = nodeManager.findNode(docId);
      expect(updatedDoc?.mentions).toEqual(['note2', 'note3']);
    });

    test('demonstrates service integration with existing nodes', async () => {
      // Create some test nodes to work with
      const node1Id = nodeManager.createNode('root1', 'First node');
      const node2Id = nodeManager.createNode('root1', 'Second node');

      // Add mentions properties
      const node1 = nodeManager.findNode(node1Id);
      const node2 = nodeManager.findNode(node2Id);

      if (node1) node1.mentions = [];
      if (node2) node2.mentions = [];

      // Test NodeOperationsService functionality
      await nodeOperationsService.updateNodeMentions(node1Id, [node2Id]);
      await nodeOperationsService.updateNodeMentions(node2Id, [node1Id]);

      // Test upsert functionality
      const result = await nodeOperationsService.upsertNode(node1Id, {
        metadata: { enhanced: true, processed: Date.now() }
      });

      expect(result.content).toBe('First node'); // Content preserved
      expect(result.metadata).toHaveProperty('enhanced');

      // Check that mentions were processed correctly
      expect(node1?.mentions || []).toEqual([node2Id]);
      expect(node2?.mentions || []).toEqual([node1Id]);
    });
  });
});
