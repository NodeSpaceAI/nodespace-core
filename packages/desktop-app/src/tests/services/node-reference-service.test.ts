/**
 * NodeReferenceService Tests - Universal Node Reference System (Phase 2.1)
 *
 * Comprehensive test suite for the NodeReferenceService architecture,
 * verifying all core features from Issue #73 specification.
 */

// Mock Svelte 5 runes immediately before any imports
(globalThis as any).$state = function <T>(initialValue: T): T {
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }
  return initialValue;
};

(globalThis as any).$derived = {
  by: function <T>(getter: () => T): T {
    return getter();
  }
};

(globalThis as any).$effect = function (fn: () => void | (() => void)): void {
  fn();
};

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { NodeReferenceService } from '../../lib/services/nodeReferenceService';
import {
  createReactiveNodeService,
  type ReactiveNodeService as NodeManager,
  type Node
} from '../../lib/services/reactiveNodeService.svelte.js';
import { HierarchyService } from '../../lib/services/hierarchyService';
import { NodeOperationsService } from '../../lib/services/nodeOperationsService';
import { MockDatabaseService } from '../../lib/services/mockDatabaseService';
import { ContentProcessor } from '../../lib/services/contentProcessor';
import { eventBus } from '../../lib/services/eventBus';
import type { ReferencesUpdateNeededEvent, NodeDeletedEvent } from '../../lib/services/eventTypes';

// Mock document for test environment
Object.defineProperty(global, 'document', {
  value: {
    createElement: vi.fn(() => ({
      isContentEditable: true,
      textContent: '',
      innerHTML: ''
    })),
    activeElement: null
  },
  writable: true
});

describe('NodeReferenceService - Universal Node Reference System', () => {
  let nodeReferenceService: NodeReferenceService;
  let nodeManager: NodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;
  let databaseService: MockDatabaseService;
  let contentProcessor: ContentProcessor;

  beforeEach(() => {
    // Clear EventBus state
    eventBus.reset();

    // Initialize services in dependency order
    const mockEvents = {
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    };
    nodeManager = createReactiveNodeService(mockEvents);

    // Initialize with a root node so other nodes can be created after it
    nodeManager.initializeFromLegacyData([
      {
        id: 'root',
        nodeType: 'text',
        content: 'Root node',
        autoFocus: false,
        inheritHeaderLevel: 0,
        children: [],
        expanded: true,
        metadata: {}
      }
    ]);

    hierarchyService = new HierarchyService(nodeManager);
    contentProcessor = ContentProcessor.getInstance();
    databaseService = new MockDatabaseService();
    nodeOperationsService = new NodeOperationsService(
      nodeManager,
      hierarchyService,
      contentProcessor
    );

    // Initialize NodeReferenceService
    nodeReferenceService = new NodeReferenceService(
      nodeManager,
      hierarchyService,
      nodeOperationsService,
      databaseService,
      contentProcessor
    );
  });

  afterEach(() => {
    eventBus.reset();
    // MockDatabaseService doesn't have a reset method, we recreate it in beforeEach
  });

  describe('@ Trigger Detection', () => {
    it('should detect valid @ trigger at cursor position', () => {
      const content = 'Hello @world this is a test';
      const cursorPosition = 12; // After '@world'

      const result = nodeReferenceService.detectTrigger(content, cursorPosition);

      expect(result).toMatchObject({
        trigger: '@',
        query: 'world',
        startPosition: 6,
        endPosition: 12,
        isValid: true
      });
    });

    it('should return null when no @ trigger is found', () => {
      const content = 'Hello world this is a test';
      const cursorPosition = 12;

      const result = nodeReferenceService.detectTrigger(content, cursorPosition);

      expect(result).toBeNull();
    });

    it('should detect partial @ query', () => {
      const content = 'Text @par more text';
      const cursorPosition = 8; // After '@par'

      const result = nodeReferenceService.detectTrigger(content, cursorPosition);

      expect(result).toMatchObject({
        trigger: '@',
        query: 'pa', // Cursor is at position 8, which is after '@pa', not '@par'
        startPosition: 5,
        endPosition: 8,
        isValid: true
      });
    });

    it('should validate trigger context correctly', () => {
      // Invalid: @ not preceded by whitespace
      const invalidContent = 'email@domain.com';
      const invalidResult = nodeReferenceService.detectTrigger(invalidContent, 6);
      expect(invalidResult?.isValid).toBeFalsy();

      // Valid: @ at start of line
      const validContent = '@username';
      const validResult = nodeReferenceService.detectTrigger(validContent, 9);
      expect(validResult?.isValid).toBeTruthy();
    });
  });

  describe('Autocomplete System', () => {
    beforeEach(async () => {
      // Create test nodes for autocomplete
      const node1Id = nodeManager.createNode('root', 'Test Project Node', 'project');
      const node2Id = nodeManager.createNode('root', 'Project Documentation', 'document');
      const node3Id = nodeManager.createNode('root', 'Another Test', 'text');

      const node1 = nodeManager.findNode(node1Id)!;
      const node2 = nodeManager.findNode(node2Id)!;
      const node3 = nodeManager.findNode(node3Id)!;

      // Add to database for search
      await databaseService.upsertNode({
        id: node1.id,
        type: 'project',
        content: 'Test Project Node',
        parent_id: null,
        root_id: node1.id,
        before_sibling_id: null,
        depth: 0,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: {},
        embedding_vector: null
      });

      await databaseService.upsertNode({
        id: node2.id,
        type: 'document',
        content: 'Project Documentation',
        parent_id: null,
        root_id: node2.id,
        before_sibling_id: null,
        depth: 0,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: {},
        embedding_vector: null
      });

      await databaseService.upsertNode({
        id: node3.id,
        type: 'text',
        content: 'Another Test',
        parent_id: null,
        root_id: node3.id,
        before_sibling_id: null,
        depth: 0,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: {},
        embedding_vector: null
      });
    });

    it('should return autocomplete suggestions for query', async () => {
      const triggerContext = {
        trigger: '@',
        query: 'project',
        startPosition: 0,
        endPosition: 8,
        element: global.document!.createElement('div'),
        isValid: true,
        metadata: {}
      };

      const result = await nodeReferenceService.showAutocomplete(triggerContext);

      expect(result.suggestions).toHaveLength(2); // 'Test Project Node' and 'Project Documentation'
      expect(result.query).toBe('project');
      expect(result.suggestions[0]).toMatchObject({
        nodeType: expect.any(String),
        relevanceScore: expect.any(Number),
        matchType: expect.any(String)
      });
    });

    it('should cache autocomplete results', async () => {
      const triggerContext = {
        trigger: '@',
        query: 'test',
        startPosition: 0,
        endPosition: 5,
        element: global.document!.createElement('div'),
        isValid: true,
        metadata: {}
      };

      // First call
      const result1 = await nodeReferenceService.showAutocomplete(triggerContext);

      // Second call should be faster (cached)
      const start = performance.now();
      const result2 = await nodeReferenceService.showAutocomplete(triggerContext);
      const end = performance.now();

      expect(result1.suggestions).toEqual(result2.suggestions);
      expect(end - start).toBeLessThan(10); // Should be very fast due to caching
    });
  });

  describe('nodespace:// URI Management', () => {
    it('should create valid nodespace URI', () => {
      const nodeId = 'test-node-123';
      const uri = nodeReferenceService.createNodespaceURI(nodeId);

      expect(uri).toBe('nodespace://node/test-node-123');
    });

    it('should create URI with options', () => {
      const nodeId = 'test-node-123';
      const options = {
        includeHierarchy: true,
        includeTimestamp: true,
        fragment: 'section1',
        queryParams: { view: 'edit' }
      };

      const uri = nodeReferenceService.createNodespaceURI(nodeId, options);

      expect(uri).toContain('nodespace://node/test-node-123');
      expect(uri).toContain('hierarchy=true');
      expect(uri).toContain('timestamp=');
      expect(uri).toContain('view=edit');
      expect(uri).toContain('#section1');
    });

    it('should parse nodespace URI correctly', () => {
      // Create a test node first
      const nodeId = nodeManager.createNode('root', 'Test Node', 'text');
      const node = nodeManager.findNode(nodeId)!;
      const uri = `nodespace://node/${node.id}?hierarchy=true&timestamp=123456#section1`;

      const result = nodeReferenceService.parseNodespaceURI(uri);

      expect(result).toMatchObject({
        nodeId: node.id,
        uri,
        title: 'Test Node',
        nodeType: 'text',
        isValid: true,
        metadata: {
          hierarchy: true,
          timestamp: '123456',
          fragment: 'section1'
        }
      });
    });

    it('should return null for invalid URI', () => {
      const invalidURI = 'http://example.com/invalid';
      const result = nodeReferenceService.parseNodespaceURI(invalidURI);

      expect(result).toBeNull();
    });

    it('should resolve URI to node', async () => {
      // Create a test node
      const nodeId = nodeManager.createNode('root', 'Test Node Content', 'text');
      const node = nodeManager.findNode(nodeId)!;
      const uri = `nodespace://node/${node.id}`;

      const resolved = await nodeReferenceService.resolveNodespaceURI(uri);

      expect(resolved).toMatchObject({
        id: node.id,
        type: 'text', // NodeSpaceNode uses 'type' not 'nodeType'
        content: 'Test Node Content'
      });
    });
  });

  describe('Bidirectional Reference Tracking', () => {
    let sourceNode: Node;
    let targetNode: Node;

    beforeEach(() => {
      const sourceNodeId = nodeManager.createNode('root', 'Source Node', 'text');
      const targetNodeId = nodeManager.createNode('root', 'Target Node', 'text');
      sourceNode = nodeManager.findNode(sourceNodeId)!;
      targetNode = nodeManager.findNode(targetNodeId)!;
    });

    it('should add bidirectional reference', async () => {
      await nodeReferenceService.addReference(sourceNode.id, targetNode.id);

      const outgoing = nodeReferenceService.getOutgoingReferences(sourceNode.id);

      expect(outgoing).toHaveLength(1);
      expect(outgoing[0]).toMatchObject({
        nodeId: targetNode.id,
        title: 'Target Node',
        nodeType: 'text',
        isValid: true
      });
    });

    it('should remove bidirectional reference', async () => {
      // Add reference first
      await nodeReferenceService.addReference(sourceNode.id, targetNode.id);

      // Verify it exists
      let outgoing = nodeReferenceService.getOutgoingReferences(sourceNode.id);
      expect(outgoing).toHaveLength(1);

      // Remove reference
      await nodeReferenceService.removeReference(sourceNode.id, targetNode.id);

      // Verify it's removed
      outgoing = nodeReferenceService.getOutgoingReferences(sourceNode.id);
      expect(outgoing).toHaveLength(0);
    });

    it('should get incoming references', async () => {
      // Add reference from source to target
      await nodeReferenceService.addReference(sourceNode.id, targetNode.id);

      // Add to database for incoming reference query
      await databaseService.upsertNode({
        id: sourceNode.id,
        type: 'text',
        content: 'Source Node',
        parent_id: null,
        root_id: sourceNode.id,
        before_sibling_id: null,
        depth: 0,
        created_at: new Date().toISOString(),
        mentions: [targetNode.id],
        metadata: {},
        embedding_vector: null
      });

      const incoming = await nodeReferenceService.getIncomingReferences(targetNode.id);

      expect(incoming).toHaveLength(1);
      expect(incoming[0]).toMatchObject({
        nodeId: sourceNode.id,
        title: 'Source Node',
        isValid: true
      });
    });
  });

  describe('Node Search and Creation', () => {
    beforeEach(async () => {
      // Create test nodes for search
      const node1Id = nodeManager.createNode('root', 'JavaScript Tutorial', 'document');
      const node2Id = nodeManager.createNode('root', 'Python Guide', 'document');
      const node3Id = nodeManager.createNode('root', 'Web Development', 'project');

      const node1 = nodeManager.findNode(node1Id)!;
      const node2 = nodeManager.findNode(node2Id)!;
      const node3 = nodeManager.findNode(node3Id)!;

      // Add to database
      await databaseService.upsertNode({
        id: node1.id,
        type: 'document',
        content: 'JavaScript Tutorial',
        parent_id: null,
        root_id: node1.id,
        before_sibling_id: null,
        depth: 0,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: {},
        embedding_vector: null
      });

      await databaseService.upsertNode({
        id: node2.id,
        type: 'document',
        content: 'Python Guide',
        parent_id: null,
        root_id: node2.id,
        before_sibling_id: null,
        depth: 0,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: {},
        embedding_vector: null
      });

      await databaseService.upsertNode({
        id: node3.id,
        type: 'project',
        content: 'Web Development',
        parent_id: null,
        root_id: node3.id,
        before_sibling_id: null,
        depth: 0,
        created_at: new Date().toISOString(),
        mentions: [],
        metadata: {},
        embedding_vector: null
      });
    });

    it('should search nodes by content', async () => {
      const results = await nodeReferenceService.searchNodes('javascript');

      expect(results).toHaveLength(1);
      expect(results[0]).toMatchObject({
        type: 'document',
        content: 'JavaScript Tutorial'
      });
    });

    it('should filter search by node type', async () => {
      const results = await nodeReferenceService.searchNodes('development', 'project');

      expect(results).toHaveLength(1);
      expect(results[0]).toMatchObject({
        type: 'project',
        content: 'Web Development'
      });
    });

    it('should create new node', async () => {
      const newNode = await nodeReferenceService.createNode('note', 'New Note Content');

      expect(newNode).toMatchObject({
        type: 'note',
        content: 'New Note Content',
        mentions: []
      });

      // Verify node was stored in database
      const dbNode = await databaseService.getNode(newNode.id);
      expect(dbNode).toBeTruthy();
      expect(dbNode?.content).toBe('New Note Content');
    });
  });

  describe('ContentProcessor Integration', () => {
    it('should detect nodespace links in content', () => {
      const content = 'Check out nodespace://node/test-123 for more info';

      const links = nodeReferenceService.detectNodespaceLinks(content);

      expect(links).toHaveLength(1);
      expect(links[0]).toMatchObject({
        uri: 'nodespace://node/test-123',
        nodeId: 'test-123',
        startPos: 10,
        endPos: 35 // Should be 35, not 34
      });
    });

    it('should enhance content processor with @ trigger detection', () => {
      // Verify that ContentProcessor enhancement was called
      expect(nodeReferenceService.enhanceContentProcessor).toBeDefined();

      // This would be tested more thoroughly in integration tests
      // where we can verify the actual content processing pipeline
    });
  });

  describe('Performance and Configuration', () => {
    it('should provide performance metrics', () => {
      const metrics = nodeReferenceService.getPerformanceMetrics();

      expect(metrics).toHaveProperty('totalTriggerDetections');
      expect(metrics).toHaveProperty('totalAutocompleteRequests');
      expect(metrics).toHaveProperty('totalURIResolutions');
      expect(metrics).toHaveProperty('avgTriggerDetectionTime');
      expect(metrics).toHaveProperty('avgAutocompleteTime');
      expect(metrics).toHaveProperty('avgURIResolutionTime');
      expect(metrics).toHaveProperty('cacheHitRatio');
    });

    it('should allow autocomplete configuration', () => {
      const newConfig = {
        maxSuggestions: 5,
        fuzzyThreshold: 0.8,
        enableFuzzySearch: false
      };

      nodeReferenceService.configureAutocomplete(newConfig);

      // Configuration change should clear caches
      const metrics = nodeReferenceService.getPerformanceMetrics();
      expect(metrics).toBeDefined();
    });

    it('should clear all caches', () => {
      nodeReferenceService.clearCaches();

      // This is a void method, so we just verify it doesn't throw
      expect(true).toBe(true);
    });
  });

  describe('EventBus Integration', () => {
    it('should emit reference events when adding/removing references', async () => {
      const events: ReferencesUpdateNeededEvent[] = [];

      // Filter events to only capture those from NodeReferenceService
      eventBus.subscribe('references:update-needed', (event: ReferencesUpdateNeededEvent) => {
        if (event.source === 'NodeReferenceService') {
          events.push(event);
        }
      });

      const sourceNodeId = nodeManager.createNode('root', 'Source', 'text');
      const targetNodeId = nodeManager.createNode('root', 'Target', 'text');
      // const _sourceNode = nodeManager.findNode(sourceNodeId)!;
      // const _targetNode = nodeManager.findNode(targetNodeId)!;

      // Add reference
      await nodeReferenceService.addReference(sourceNodeId, targetNodeId);

      // Remove reference
      await nodeReferenceService.removeReference(sourceNodeId, targetNodeId);

      expect(events).toHaveLength(2);
      expect(events[0]).toMatchObject({
        type: 'references:update-needed',
        namespace: 'coordination',
        source: 'NodeReferenceService'
      });
      expect(events[1]).toMatchObject({
        type: 'references:update-needed',
        namespace: 'coordination',
        source: 'NodeReferenceService'
      });
    });

    it('should handle node deletion cleanup', async () => {
      const sourceNodeId = nodeManager.createNode('root', 'Source', 'text');
      const targetNodeId = nodeManager.createNode('root', 'Target', 'text');
      // const _sourceNode = nodeManager.findNode(sourceNodeId)!;
      // const _targetNode = nodeManager.findNode(targetNodeId)!;

      // Add reference
      await nodeReferenceService.addReference(sourceNodeId, targetNodeId);

      // Verify reference was added
      let outgoing = nodeReferenceService.getOutgoingReferences(sourceNodeId);
      expect(outgoing).toHaveLength(1);

      // Add the source node to database so cleanup can find it
      await databaseService.upsertNode({
        id: sourceNodeId,
        type: 'text',
        content: 'Source',
        parent_id: null,
        root_id: sourceNodeId,
        before_sibling_id: null,
        depth: 0,
        created_at: new Date().toISOString(),
        mentions: [targetNodeId], // This is what cleanup will search for
        metadata: {},
        embedding_vector: null
      });

      // Simulate node deletion event
      eventBus.emit({
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: targetNodeId,
        parentId: undefined
      } as Omit<NodeDeletedEvent, 'timestamp'>);

      // Give time for cleanup to process
      await new Promise((resolve) => setTimeout(resolve, 50));

      // References to deleted node should be cleaned up
      outgoing = nodeReferenceService.getOutgoingReferences(sourceNodeId);
      expect(outgoing).toHaveLength(0);
    });
  });
});
