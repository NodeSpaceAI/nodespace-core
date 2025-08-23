/**
 * NodeReferenceService Performance Tests - Comprehensive Benchmarking Suite
 *
 * Performance testing suite ensuring all performance targets are met:
 * - @ trigger detection: <10ms
 * - Autocomplete response: <50ms
 * - Node decoration rendering: <16ms (60fps)
 * - 500+ references handling efficiently
 * - Memory usage monitoring and leak prevention
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { NodeReferenceService } from '../../lib/services/NodeReferenceService.js';
import { OptimizedNodeReferenceService } from '../../lib/services/OptimizedNodeReferenceService.js';
import { PerformanceMonitor } from '../../lib/services/PerformanceMonitor.js';
import { NodeManager, type Node } from '../../lib/services/NodeManager.js';
import { HierarchyService } from '../../lib/services/HierarchyService.js';
import { NodeOperationsService } from '../../lib/services/NodeOperationsService.js';
import { MockDatabaseService, type NodeSpaceNode } from '../../lib/services/MockDatabaseService.js';
import { ContentProcessor } from '../../lib/services/contentProcessor.js';
import { eventBus } from '../../lib/services/EventBus.js';

// Union type for service operations
type ReferenceService = NodeReferenceService | OptimizedNodeReferenceService;

// Performance test configuration
const PERFORMANCE_TARGETS = {
  TRIGGER_DETECTION_MS: 10,
  AUTOCOMPLETE_RESPONSE_MS: 50,
  DECORATION_RENDER_MS: 16,
  URI_RESOLUTION_MS: 5,
  LARGE_DATASET_SIZE: 500,
  MEMORY_THRESHOLD_MB: 100
};

// Helper to convert NodeManager Node to NodeSpaceNode
function convertToNodeSpaceNode(managerNode: Node, index: number = 0): NodeSpaceNode {
  return {
    id: managerNode.id || `test-node-${index}`,
    type: managerNode.nodeType || 'text',
    content: managerNode.content || `Test content ${index}`,
    parent_id: managerNode.parentId || null,
    root_id: managerNode.id || `test-node-${index}`,
    before_sibling_id: null,
    created_at: new Date().toISOString(),
    mentions: [],
    metadata: managerNode.metadata || {},
    embedding_vector: null
  };
}

// Test data generator
class PerformanceTestDataGenerator {
  static generateLargeContent(size: number): string {
    const words = ['test', 'node', 'content', 'reference', 'system', 'performance', 'optimization'];
    let content = '';

    for (let i = 0; i < size; i++) {
      content += words[i % words.length] + ' ';
      if (i % 50 === 0) content += '\n';
    }

    return content;
  }

  static async generateLargeNodeSet(
    databaseService: MockDatabaseService,
    size: number
  ): Promise<NodeSpaceNode[]> {
    const nodes: NodeSpaceNode[] = [];

    for (let i = 0; i < size; i++) {
      const node: NodeSpaceNode = {
        id: `perf-node-${i}`,
        type: i % 3 === 0 ? 'project' : i % 3 === 1 ? 'document' : 'text',
        content: `Performance test node ${i} - ${this.generateLargeContent(20)}`,
        parent_id: i > 0 ? `perf-node-${Math.floor(i / 10)}` : null,
        root_id: `perf-node-0`,
        before_sibling_id: i > 0 ? `perf-node-${i - 1}` : null,
        created_at: new Date().toISOString(),
        mentions: i % 5 === 0 ? [`perf-node-${(i + 1) % size}`] : [],
        metadata: { index: i, category: `category-${i % 10}` },
        embedding_vector: null
      };

      await databaseService.upsertNode(node);
      nodes.push(node);
    }

    return nodes;
  }
}

describe('NodeReferenceService Performance Tests', () => {
  let nodeReferenceService: NodeReferenceService;
  let optimizedService: OptimizedNodeReferenceService;
  let performanceMonitor: PerformanceMonitor;
  let nodeManager: NodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;
  let databaseService: MockDatabaseService;
  let contentProcessor: ContentProcessor;

  beforeEach(async () => {
    // Clear EventBus state
    eventBus.reset();

    // Initialize performance monitor
    performanceMonitor = PerformanceMonitor.getInstance();

    // Initialize services
    const mockEvents = {
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    };

    nodeManager = new NodeManager(mockEvents);
    hierarchyService = new HierarchyService(nodeManager);
    contentProcessor = ContentProcessor.getInstance();
    databaseService = new MockDatabaseService();
    nodeOperationsService = new NodeOperationsService(
      nodeManager,
      hierarchyService,
      contentProcessor
    );

    // Initialize both services for comparison
    nodeReferenceService = new NodeReferenceService(
      nodeManager,
      hierarchyService,
      nodeOperationsService,
      databaseService,
      contentProcessor
    );

    optimizedService = new OptimizedNodeReferenceService(
      nodeManager,
      hierarchyService,
      nodeOperationsService,
      databaseService,
      contentProcessor
    );
  });

  afterEach(() => {
    eventBus.reset();
    optimizedService?.cleanup();
  });

  describe('@ Trigger Detection Performance', () => {
    it('should detect @ triggers within 10ms target', async () => {
      const testCases = [
        'Simple @query test',
        'Multiple @first and @second triggers',
        PerformanceTestDataGenerator.generateLargeContent(1000) + ' @query',
        '@start of line query',
        'End of content @query'
      ];

      let totalTime = 0;
      let maxTime = 0;

      for (const content of testCases) {
        const start = performance.now();
        const result = nodeReferenceService.detectTrigger(content, content.length);
        expect(result).toBeDefined(); // Ensure the call completes successfully
        const duration = performance.now() - start;

        totalTime += duration;
        maxTime = Math.max(maxTime, duration);

        // Individual operation should be fast
        expect(duration).toBeLessThan(PERFORMANCE_TARGETS.TRIGGER_DETECTION_MS);
      }

      const avgTime = totalTime / testCases.length;
      console.log(`Trigger detection - Avg: ${avgTime.toFixed(2)}ms, Max: ${maxTime.toFixed(2)}ms`);

      expect(avgTime).toBeLessThan(PERFORMANCE_TARGETS.TRIGGER_DETECTION_MS);
      expect(maxTime).toBeLessThan(PERFORMANCE_TARGETS.TRIGGER_DETECTION_MS * 2);
    });

    it('should show performance improvement with optimization', () => {
      const content = PerformanceTestDataGenerator.generateLargeContent(500) + ' @query test';
      const position = content.length;

      // Measure standard service
      const start1 = performance.now();
      const result1 = nodeReferenceService.detectTrigger(content, position);
      const time1 = performance.now() - start1;

      // Measure optimized service
      const start2 = performance.now();
      const result2 = optimizedService.detectTrigger(content, position);
      const time2 = performance.now() - start2;

      console.log(`Standard: ${time1.toFixed(2)}ms, Optimized: ${time2.toFixed(2)}ms`);

      // Both should work correctly
      expect(result1?.query).toBe(result2?.query);

      // Optimized should be faster or at least not slower
      expect(time2).toBeLessThanOrEqual(time1 * 1.1); // Allow 10% variance
    });

    it('should handle stress test with rapid triggers', () => {
      const iterations = 1000;
      const content = 'Test @query content';

      const start = performance.now();

      for (let i = 0; i < iterations; i++) {
        const result = nodeReferenceService.detectTrigger(content, 12);
        expect(result?.query).toBe('query');
      }

      const totalTime = performance.now() - start;
      const avgTime = totalTime / iterations;

      console.log(
        `Stress test - ${iterations} iterations: Avg ${avgTime.toFixed(3)}ms per operation`
      );

      expect(avgTime).toBeLessThan(1); // Should be sub-millisecond for simple cases
    });
  });

  describe('Autocomplete Performance', () => {
    beforeEach(async () => {
      // Generate large dataset for performance testing
      await PerformanceTestDataGenerator.generateLargeNodeSet(
        databaseService,
        PERFORMANCE_TARGETS.LARGE_DATASET_SIZE
      );
    });

    it('should respond to autocomplete within 50ms target', async () => {
      const queries = ['perf', 'node', 'test', 'project', 'document'];

      for (const query of queries) {
        const triggerContext = {
          trigger: '@',
          query,
          startPosition: 0,
          endPosition: query.length + 1,
          element: null,
          isValid: true,
          metadata: {}
        };

        const start = performance.now();
        const result = await nodeReferenceService.showAutocomplete(triggerContext);
        const duration = performance.now() - start;

        console.log(
          `Autocomplete "${query}": ${duration.toFixed(2)}ms (${result.suggestions.length} results)`
        );

        expect(duration).toBeLessThan(PERFORMANCE_TARGETS.AUTOCOMPLETE_RESPONSE_MS);
        expect(result.suggestions.length).toBeGreaterThan(0);
      }
    });

    it('should handle large dataset efficiently', async () => {
      const triggerContext = {
        trigger: '@',
        query: 'perf',
        startPosition: 0,
        endPosition: 5,
        element: null,
        isValid: true,
        metadata: {}
      };

      // First call (cold cache)
      const start1 = performance.now();
      const result1 = await nodeReferenceService.showAutocomplete(triggerContext);
      const coldTime = performance.now() - start1;

      // Second call (warm cache)
      const start2 = performance.now();
      const result2 = await nodeReferenceService.showAutocomplete(triggerContext);
      const warmTime = performance.now() - start2;

      console.log(`Large dataset - Cold: ${coldTime.toFixed(2)}ms, Warm: ${warmTime.toFixed(2)}ms`);

      expect(coldTime).toBeLessThan(PERFORMANCE_TARGETS.AUTOCOMPLETE_RESPONSE_MS * 2); // Allow more time for large dataset
      expect(warmTime).toBeLessThan(10); // Cache should make it very fast
      expect(result1.suggestions.length).toBe(result2.suggestions.length);
    });

    it('should show caching effectiveness', async () => {
      const query = 'performance';
      const triggerContext = {
        trigger: '@',
        query,
        startPosition: 0,
        endPosition: query.length + 1,
        element: null,
        isValid: true,
        metadata: {}
      };

      // Multiple calls to test cache hit ratio
      const times: number[] = [];

      for (let i = 0; i < 10; i++) {
        const start = performance.now();
        await nodeReferenceService.showAutocomplete(triggerContext);
        times.push(performance.now() - start);
      }

      const firstCall = times[0];
      const avgCachedCalls = times.slice(1).reduce((a, b) => a + b, 0) / (times.length - 1);

      console.log(
        `Cache effectiveness - First: ${firstCall.toFixed(2)}ms, Avg cached: ${avgCachedCalls.toFixed(2)}ms`
      );

      // Cached calls should be significantly faster
      expect(avgCachedCalls).toBeLessThan(firstCall * 0.5);
    });
  });

  describe('URI Resolution Performance', () => {
    beforeEach(async () => {
      // Create nodes for URI resolution testing
      for (let i = 0; i < 100; i++) {
        const nodeId = nodeManager.createNode(
          i === 0 ? 'root' : `test-node-${i - 1}`, // afterNodeId
          `URI Test Node ${i}`, // content
          'text' // nodeType
        );
        const node = nodeManager.findNode(nodeId);
        if (node) {
          await databaseService.upsertNode(convertToNodeSpaceNode(node));
        }
      }
    });

    it('should resolve URIs within 5ms target', () => {
      const nodes = Array.from({ length: 50 }, (_, i) => `perf-node-${i}`);

      for (const nodeId of nodes) {
        const uri = `nodespace://node/${nodeId}`;

        const start = performance.now();
        const result = nodeReferenceService.resolveNodespaceURI(uri);
        expect(result).toBeDefined(); // Ensure the call completes successfully
        const duration = performance.now() - start;

        expect(duration).toBeLessThan(PERFORMANCE_TARGETS.URI_RESOLUTION_MS);
      }
    });

    it('should handle batch URI resolution efficiently', () => {
      const uris = Array.from({ length: 100 }, (_, i) => `nodespace://node/perf-node-${i}`);

      const start = performance.now();

      const results = uris.map((uri) => nodeReferenceService.resolveNodespaceURI(uri));

      const totalTime = performance.now() - start;
      const avgTime = totalTime / uris.length;

      console.log(
        `Batch URI resolution - Total: ${totalTime.toFixed(2)}ms, Avg: ${avgTime.toFixed(3)}ms`
      );

      expect(avgTime).toBeLessThan(PERFORMANCE_TARGETS.URI_RESOLUTION_MS);
      expect(results.filter((r) => r !== null).length).toBeGreaterThan(0);
    });
  });

  describe('Memory Performance and Leak Prevention', () => {
    it('should maintain memory usage within limits during heavy operations', async () => {
      const nodeProcess = (globalThis as Record<string, unknown>).process as
        | { memoryUsage?: () => { heapUsed: number } }
        | undefined;
      const initialMemory = nodeProcess?.memoryUsage ? nodeProcess.memoryUsage().heapUsed : 0;

      // Perform memory-intensive operations
      for (let i = 0; i < 1000; i++) {
        const content = PerformanceTestDataGenerator.generateLargeContent(100);
        nodeReferenceService.detectTrigger(content, content.length);

        const triggerContext = {
          trigger: '@',
          query: `query${i}`,
          startPosition: 0,
          endPosition: 10,
          element: null,
          isValid: true,
          metadata: {}
        };

        await nodeReferenceService.showAutocomplete(triggerContext);

        // Clear caches periodically to prevent unbounded growth
        if (i % 100 === 0) {
          nodeReferenceService.clearCaches();
        }
      }

      const finalMemory = nodeProcess?.memoryUsage ? nodeProcess.memoryUsage().heapUsed : 0;
      const memoryGrowthMB = (finalMemory - initialMemory) / 1024 / 1024;

      console.log(`Memory growth: ${memoryGrowthMB.toFixed(2)}MB`);

      if (nodeProcess?.memoryUsage) {
        expect(memoryGrowthMB).toBeLessThan(PERFORMANCE_TARGETS.MEMORY_THRESHOLD_MB);
      }
    });

    it('should clean up resources properly', () => {
      // Create service and use it
      const tempService = new OptimizedNodeReferenceService(
        nodeManager,
        hierarchyService,
        nodeOperationsService,
        databaseService,
        contentProcessor
      );

      // Use the service to create some cached data
      tempService.detectTrigger('test @query', 12);

      // Cleanup should not throw
      expect(() => tempService.cleanup()).not.toThrow();
    });
  });

  describe('Large Dataset Handling', () => {
    beforeEach(async () => {
      // Create large dataset
      await PerformanceTestDataGenerator.generateLargeNodeSet(
        databaseService,
        PERFORMANCE_TARGETS.LARGE_DATASET_SIZE
      );
    });

    it('should handle 500+ references efficiently', async () => {
      const start = performance.now();

      // Search across large dataset
      const results = await nodeReferenceService.searchNodes('perf');

      const searchTime = performance.now() - start;

      console.log(`Large dataset search: ${searchTime.toFixed(2)}ms for ${results.length} results`);

      expect(searchTime).toBeLessThan(200); // Allow more time for large dataset
      expect(results.length).toBeGreaterThan(50); // Should find many matches
    });

    it('should maintain performance with complex reference graphs', async () => {
      // Create interconnected references
      const nodes = await databaseService.queryNodes({});

      for (let i = 0; i < 100; i++) {
        const sourceNode = nodes[i];
        const targetNode = nodes[(i + 1) % nodes.length];

        await nodeReferenceService.addReference(sourceNode.id, targetNode.id);
      }

      // Test reference traversal performance
      const testNode = nodes[0];

      const start = performance.now();
      const outgoing = nodeReferenceService.getOutgoingReferences(testNode.id);
      const incoming = await nodeReferenceService.getIncomingReferences(testNode.id);
      const totalTime = performance.now() - start;

      console.log(
        `Reference traversal: ${totalTime.toFixed(2)}ms (${outgoing.length} out, ${incoming.length} in)`
      );

      expect(totalTime).toBeLessThan(50);
      expect(outgoing.length).toBeGreaterThan(0);
    });
  });

  describe('Benchmark Comparison - Standard vs Optimized', () => {
    beforeEach(async () => {
      await PerformanceTestDataGenerator.generateLargeNodeSet(databaseService, 100);
    });

    it('should demonstrate performance improvements with optimization', async () => {
      const testOperations = [
        {
          name: 'trigger-detection',
          op: (service: ReferenceService) => service.detectTrigger('test @query here', 12)
        },
        {
          name: 'autocomplete',
          op: async (service: ReferenceService) =>
            await service.showAutocomplete({
              trigger: '@',
              query: 'perf',
              startPosition: 0,
              endPosition: 5,
              element: null,
              isValid: true,
              metadata: {}
            })
        },
        {
          name: 'uri-resolution',
          op: (service: ReferenceService) =>
            service.resolveNodespaceURI('nodespace://node/perf-node-1')
        }
      ];

      const benchmarks: { [key: string]: { standard: number; optimized: number } } = {};

      for (const { name, op } of testOperations) {
        // Benchmark standard service
        const standardTimes: number[] = [];
        for (let i = 0; i < 10; i++) {
          const start = performance.now();
          await op(nodeReferenceService);
          standardTimes.push(performance.now() - start);
        }

        // Benchmark optimized service
        const optimizedTimes: number[] = [];
        for (let i = 0; i < 10; i++) {
          const start = performance.now();
          await op(optimizedService);
          optimizedTimes.push(performance.now() - start);
        }

        benchmarks[name] = {
          standard: standardTimes.reduce((a, b) => a + b, 0) / standardTimes.length,
          optimized: optimizedTimes.reduce((a, b) => a + b, 0) / optimizedTimes.length
        };
      }

      // Log performance comparison
      console.log('\nPerformance Benchmark Results:');
      for (const [operation, times] of Object.entries(benchmarks)) {
        const improvement = (((times.standard - times.optimized) / times.standard) * 100).toFixed(
          1
        );
        console.log(
          `${operation}: Standard ${times.standard.toFixed(2)}ms -> Optimized ${times.optimized.toFixed(2)}ms (${improvement}% improvement)`
        );

        // Optimized should be at least as fast (allowing for measurement variance)
        expect(times.optimized).toBeLessThanOrEqual(times.standard * 1.2);
      }
    });
  });

  describe('Performance Regression Detection', () => {
    it('should detect performance regressions', () => {
      const metrics = performanceMonitor.getComprehensiveMetrics();

      // All core operations should meet targets
      if (metrics.triggerDetectionTime > 0) {
        expect(metrics.triggerDetectionTime).toBeLessThan(PERFORMANCE_TARGETS.TRIGGER_DETECTION_MS);
      }

      if (metrics.autocompleteResponseTime > 0) {
        expect(metrics.autocompleteResponseTime).toBeLessThan(
          PERFORMANCE_TARGETS.AUTOCOMPLETE_RESPONSE_MS
        );
      }

      console.log('Performance Metrics:', {
        triggerDetection: metrics.triggerDetectionTime.toFixed(2) + 'ms',
        autocomplete: metrics.autocompleteResponseTime.toFixed(2) + 'ms',
        operations: metrics.totalOperations,
        cacheHitRatio: (metrics.cacheHitRatio * 100).toFixed(1) + '%'
      });
    });
  });
});
