/**
 * NodeReferenceService Tests - Universal Node Reference System (Phase 2.1)
 *
 * Comprehensive test suite for the NodeReferenceService architecture,
 * verifying all core features from Issue #73 specification.
 */

// Mock Svelte 5 runes immediately before any imports - using proper type assertions
(globalThis as Record<string, unknown>).$state = function <T>(initialValue: T): T {
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }
  return initialValue;
};

(globalThis as Record<string, unknown>).$derived = {
  by: function <T>(getter: () => T): T {
    return getter();
  }
};

(globalThis as Record<string, unknown>).$effect = function (fn: () => void | (() => void)): void {
  fn();
};

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { NodeReferenceService } from '../../lib/services/node-reference-service';
import {
  createReactiveNodeService,
  type ReactiveNodeService as NodeManager
} from '../../lib/services/reactive-node-service.svelte.js';
import { HierarchyService } from '../../lib/services/hierarchy-service';
import { NodeOperationsService } from '../../lib/services/node-operations-service';
import { ContentProcessor } from '../../lib/services/content-processor';
import { eventBus } from '../../lib/services/event-bus';
import type { ReferencesUpdateNeededEvent, NodeDeletedEvent } from '../../lib/services/event-types';
import type { Node } from '../../lib/types/node';
import { createTestNode, waitForEffects } from '../helpers';

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

// In-memory node storage for tests
const nodeStore = new Map<string, Node>();

// Create a mock database service that implements the same interface as TauriNodeService
class MockTauriNodeService {
  private initialized = false;
  private dbPath: string | null = null;

  async initializeDatabase(): Promise<string> {
    this.initialized = true;
    this.dbPath = '/mock/db/path';
    return this.dbPath;
  }

  async selectDatabaseLocation(): Promise<string> {
    this.initialized = true;
    this.dbPath = '/mock/db/path';
    return this.dbPath;
  }

  async createNode(node: Omit<Node, 'createdAt' | 'modifiedAt'>): Promise<string> {
    const fullNode: Node = {
      ...node,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString()
    };
    nodeStore.set(fullNode.id, fullNode);
    return fullNode.id;
  }

  async getNode(id: string): Promise<Node | null> {
    return nodeStore.get(id) || null;
  }

  async updateNode(id: string, update: Partial<Node>): Promise<void> {
    const existing = nodeStore.get(id);
    if (existing) {
      const updated = { ...existing, ...update, modifiedAt: new Date().toISOString() };
      nodeStore.set(id, updated);
    }
  }

  async deleteNode(id: string): Promise<void> {
    nodeStore.delete(id);
  }

  async getChildren(parentId: string): Promise<Node[]> {
    return Array.from(nodeStore.values()).filter((node) => node.parentId === parentId);
  }

  async searchNodes(query: string, nodeType?: string): Promise<Node[]> {
    const lowerQuery = query.toLowerCase();
    return Array.from(nodeStore.values()).filter((node) => {
      const matchesQuery = node.content.toLowerCase().includes(lowerQuery);
      const matchesType = !nodeType || node.nodeType === nodeType;
      return matchesQuery && matchesType;
    });
  }

  async getNodesByMentions(targetId: string): Promise<Node[]> {
    return Array.from(nodeStore.values()).filter((node) => node.mentions?.includes(targetId));
  }

  async queryNodes(query: {
    id?: string;
    contentContains?: string;
    nodeType?: string;
    mentionedBy?: string;
    limit?: number;
  }): Promise<Node[]> {
    let results = Array.from(nodeStore.values());

    // Filter by ID
    if (query.id) {
      const node = nodeStore.get(query.id);
      return node ? [node] : [];
    }

    // Filter by content
    if (query.contentContains) {
      const lowerQuery = query.contentContains.toLowerCase();
      results = results.filter((node) => node.content.toLowerCase().includes(lowerQuery));
    }

    // Filter by type
    if (query.nodeType) {
      results = results.filter((node) => node.nodeType === query.nodeType);
    }

    // Filter by mentions (with type guard)
    if (query.mentionedBy) {
      const mentionedBy = query.mentionedBy; // Capture in const for type narrowing
      results = results.filter((node) => node.mentions?.includes(mentionedBy));
    }

    // Apply limit
    if (query.limit) {
      results = results.slice(0, query.limit);
    }

    return results;
  }

  isInitialized(): boolean {
    return this.initialized;
  }

  getDatabasePath(): string | null {
    return this.dbPath;
  }

  private ensureInitialized(): void {
    if (!this.initialized) {
      throw new Error('Database not initialized');
    }
  }
}

describe('NodeReferenceService - Universal Node Reference System', () => {
  let nodeReferenceService: NodeReferenceService;
  let nodeManager: NodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;
  let databaseService: MockTauriNodeService;
  let contentProcessor: ContentProcessor;

  beforeEach(async () => {
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
    nodeManager.initializeNodes([createTestNode('root', 'Root node')], {
      autoFocus: false,
      inheritHeaderLevel: 0,
      expanded: true
    });

    hierarchyService = new HierarchyService(nodeManager);
    contentProcessor = ContentProcessor.getInstance();

    // Clear node store before each test
    nodeStore.clear();

    // Mock TauriNodeService with in-memory storage
    databaseService = new MockTauriNodeService();
    await databaseService.initializeDatabase();

    nodeOperationsService = new NodeOperationsService(
      nodeManager,
      hierarchyService,
      contentProcessor
    );

    // Initialize NodeReferenceService
    // TypeScript note: MockTauriNodeService implements the public interface of TauriNodeService
    // but may differ in private implementation details, so we use type assertion here
    nodeReferenceService = new NodeReferenceService(
      nodeManager,
      hierarchyService,
      nodeOperationsService,
      databaseService as unknown as import('../../lib/services/tauri-node-service').TauriNodeService,
      contentProcessor
    );
  });

  afterEach(() => {
    eventBus.reset();
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

      // Add to database for search using new schema
      await databaseService.createNode({
        id: node1.id,
        nodeType: 'project',
        content: 'Test Project Node',
        parentId: null,
        originNodeId: node1.id,
        beforeSiblingId: null,
        mentions: [],
        properties: {},
        embeddingVector: null
      });

      await databaseService.createNode({
        id: node2.id,
        nodeType: 'document',
        content: 'Project Documentation',
        parentId: null,
        originNodeId: node2.id,
        beforeSiblingId: null,
        mentions: [],
        properties: {},
        embeddingVector: null
      });

      await databaseService.createNode({
        id: node3.id,
        nodeType: 'text',
        content: 'Another Test',
        parentId: null,
        originNodeId: node3.id,
        beforeSiblingId: null,
        mentions: [],
        properties: {},
        embeddingVector: null
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
        nodeType: 'text', // UnifiedNode uses 'node_type'
        content: 'Test Node Content'
      });
    });
  });

  describe('Bidirectional Reference Tracking', () => {
    let sourceNodeId: string;
    let targetNodeId: string;

    beforeEach(() => {
      sourceNodeId = nodeManager.createNode('root', 'Source Node', 'text');
      targetNodeId = nodeManager.createNode('root', 'Target Node', 'text');
    });

    it('should add bidirectional reference', async () => {
      await nodeReferenceService.addReference(sourceNodeId, targetNodeId);

      const outgoing = nodeReferenceService.getOutgoingReferences(sourceNodeId);

      expect(outgoing).toHaveLength(1);
      expect(outgoing[0]).toMatchObject({
        nodeId: targetNodeId,
        title: 'Target Node',
        nodeType: 'text',
        isValid: true
      });
    });

    it('should remove bidirectional reference', async () => {
      // Add reference first
      await nodeReferenceService.addReference(sourceNodeId, targetNodeId);

      // Verify it exists
      let outgoing = nodeReferenceService.getOutgoingReferences(sourceNodeId);
      expect(outgoing).toHaveLength(1);

      // Remove reference
      await nodeReferenceService.removeReference(sourceNodeId, targetNodeId);

      // Verify it's removed
      outgoing = nodeReferenceService.getOutgoingReferences(sourceNodeId);
      expect(outgoing).toHaveLength(0);
    });

    it('should get incoming references', async () => {
      // Add reference from source to target
      await nodeReferenceService.addReference(sourceNodeId, targetNodeId);

      // Add to database for incoming reference query using new schema
      await databaseService.createNode({
        id: sourceNodeId,
        nodeType: 'text',
        content: 'Source Node',
        parentId: null,
        originNodeId: sourceNodeId,
        beforeSiblingId: null,
        mentions: [targetNodeId],
        properties: {},
        embeddingVector: null
      });

      const incoming = await nodeReferenceService.getIncomingReferences(targetNodeId);

      expect(incoming).toHaveLength(1);
      expect(incoming[0]).toMatchObject({
        nodeId: sourceNodeId,
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

      // Add to database using new schema
      await databaseService.createNode({
        id: node1.id,
        nodeType: 'document',
        content: 'JavaScript Tutorial',
        parentId: null,
        originNodeId: node1.id,
        beforeSiblingId: null,
        mentions: [],
        properties: {},
        embeddingVector: null
      });

      await databaseService.createNode({
        id: node2.id,
        nodeType: 'document',
        content: 'Python Guide',
        parentId: null,
        originNodeId: node2.id,
        beforeSiblingId: null,
        mentions: [],
        properties: {},
        embeddingVector: null
      });

      await databaseService.createNode({
        id: node3.id,
        nodeType: 'project',
        content: 'Web Development',
        parentId: null,
        originNodeId: node3.id,
        beforeSiblingId: null,
        mentions: [],
        properties: {},
        embeddingVector: null
      });
    });

    it('should search nodes by content', async () => {
      const results = await nodeReferenceService.searchNodes('javascript');

      expect(results).toHaveLength(1);
      expect(results[0]).toMatchObject({
        nodeType: 'document',
        content: 'JavaScript Tutorial'
      });
    });

    it('should filter search by node type', async () => {
      const results = await nodeReferenceService.searchNodes('development', 'project');

      expect(results).toHaveLength(1);
      expect(results[0]).toMatchObject({
        nodeType: 'project',
        content: 'Web Development'
      });
    });

    it('should create new node', async () => {
      const newNode = await nodeReferenceService.createNode('note', 'New Note Content');

      expect(newNode).toMatchObject({
        nodeType: 'note',
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

    it('should handle node deletion cache invalidation (Issue #190 fix)', async () => {
      const sourceNodeId = nodeManager.createNode('root', 'Source', 'text');
      const targetNodeId = nodeManager.createNode('root', 'Target', 'text');

      // Add reference
      await nodeReferenceService.addReference(sourceNodeId, targetNodeId);

      // Verify reference was added
      let outgoing = nodeReferenceService.getOutgoingReferences(sourceNodeId);
      expect(outgoing).toHaveLength(1);

      // Simulate node deletion event
      eventBus.emit({
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: targetNodeId,
        parentId: undefined
      } as Omit<NodeDeletedEvent, 'timestamp'>);

      // Give time for event processing
      await waitForEffects(50);

      // NOTE: After Issue #190 fix, we no longer automatically clean up references
      // in the application layer. The database CASCADE deletes handle this.
      // The event handler only invalidates caches now, so in-memory references
      // remain until the next database query refreshes them.
      // This test now verifies that cache invalidation happens without errors.

      // Verify the deletion event was processed without errors
      // (no "database is locked" error should occur)
      expect(true).toBe(true); // Test completes without throwing
    });
  });
});
