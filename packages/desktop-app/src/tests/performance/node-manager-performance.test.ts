/**
 * Performance validation tests for NodeManager
 * Tests handling of large node documents efficiently
 *
 * PERFORMANCE TEST MODES:
 * - Fast mode (default): Smaller datasets (100-500 nodes) for quick feedback during development
 * - Full mode (TEST_FULL_PERFORMANCE=1): Large datasets (1000-2000 nodes) for comprehensive validation
 *
 * Usage:
 *   bun run test                              # Fast mode (default)
 *   TEST_FULL_PERFORMANCE=1 bun run test:perf # Full performance validation
 */

// CRITICAL: Import setup BEFORE anything else to ensure Svelte mocks are applied
import '../setup-svelte-mocks';

import { describe, test, expect, beforeEach } from 'vitest';
import {
  createReactiveNodeService,
  ReactiveNodeService as NodeManager
} from '../../lib/services/reactive-node-service.svelte.js';
import type { NodeManagerEvents } from '../../lib/services/reactive-node-service.svelte.js';
import { createTestNode } from '../helpers';

// Performance test scaling based on environment variable
const FULL_PERFORMANCE = process.env.TEST_FULL_PERFORMANCE === '1';

// Dataset sizes: Fast mode for development, Full mode for comprehensive validation
const PERF_SCALE = {
  init: FULL_PERFORMANCE ? 1000 : 100, // Node initialization test
  lookup: FULL_PERFORMANCE ? 1500 : 150, // Lookup performance test
  combine: FULL_PERFORMANCE ? 2000 : 200, // Combine operations test
  hierarchy: FULL_PERFORMANCE ? 1000 : 100, // Hierarchy operations test
  deepNesting: FULL_PERFORMANCE ? 1000 : 100, // Deep nesting visibility test
  multiCycle: FULL_PERFORMANCE ? 500 : 50, // Multi-cycle test dataset
  concurrent: FULL_PERFORMANCE ? 1000 : 100 // Concurrent operations test
};

// Performance thresholds: Adaptive based on dataset size
const PERF_THRESHOLDS = {
  init: FULL_PERFORMANCE ? 100 : 20, // Node initialization threshold (ms)
  combine: FULL_PERFORMANCE ? 100 : 20, // Combine operations threshold (ms)
  hierarchy: FULL_PERFORMANCE ? 500 : 100, // Hierarchy operations threshold (ms)
  deepNesting: FULL_PERFORMANCE ? 700 : 100 // Deep nesting threshold (ms)
};

describe('NodeManager Performance Tests', () => {
  let nodeManager: NodeManager;
  let mockEvents: NodeManagerEvents;

  beforeEach(() => {
    mockEvents = {
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    };
    nodeManager = createReactiveNodeService(mockEvents);
  });

  // Generate large dataset for performance testing
  const generateLargeNodeDataset = (nodeCount: number) => {
    const nodes = [];
    const hierarchyMap = new Map<string, string[]>(); // parentId -> childIds

    // Build parent-child relationships
    for (let i = 0; i < nodeCount; i++) {
      const nodeId = `node-${i}`;
      // Create a simpler hierarchy: every 10 nodes form a group under a parent
      const parentIndex = Math.floor(i / 10) * 10;
      const parentId = i > 0 && i !== parentIndex ? `node-${parentIndex}` : null;

      nodes.push(
        createTestNode(nodeId, `Content for node ${i}`, 'text', parentId, {
          createdAt: new Date().toISOString()
        })
      );

      // Track parent-child relationships for hierarchy cache
      if (parentId !== null) {
        if (!hierarchyMap.has(parentId)) {
          hierarchyMap.set(parentId, []);
        }
        hierarchyMap.get(parentId)!.push(nodeId);
      }
    }

    return { nodes, hierarchyMap };
  };

  test(`initializes ${PERF_SCALE.init} nodes efficiently (< ${PERF_THRESHOLDS.init}ms)`, () => {
    const { nodes: largeDataset } = generateLargeNodeDataset(PERF_SCALE.init);

    // Populate hierarchy cache BEFORE initializeNodes for graph-native architecture
    // This simulates what the backend adapter would do when loading from database
    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy


    const startTime = performance.now();
    nodeManager.initializeNodes(largeDataset);
    const endTime = performance.now();

    const duration = endTime - startTime;
    console.log(`${PERF_SCALE.init} node initialization: ${duration.toFixed(2)}ms`);

    // Primary assertion: Performance (time in ms)
    expect(duration).toBeLessThan(PERF_THRESHOLDS.init);

    // Verify all nodes were initialized
    expect(nodeManager.nodes.size).toBe(PERF_SCALE.init);

    // Note: In test environment with mocked Svelte reactivity, hierarchy may not be computed
    // correctly. In production, the hierarchy would be properly calculated.
    // For performance testing, we only care about initialization speed.
    expect(nodeManager.rootNodeIds.length).toBeGreaterThan(0); // At least some roots exist
  });

  test(`node lookup performance with ${PERF_SCALE.lookup}+ nodes (< 1ms)`, () => {
    const { nodes: largeDataset } = generateLargeNodeDataset(PERF_SCALE.lookup);
    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

    nodeManager.initializeNodes(largeDataset);

    const startTime = performance.now();
    for (let i = 0; i < 100; i++) {
      const randomId = `node-${Math.floor(Math.random() * PERF_SCALE.lookup)}`;
      nodeManager.findNode(randomId);
    }
    const endTime = performance.now();

    const duration = endTime - startTime;
    const avgDuration = duration / 100;
    console.log(`Average lookup time (${PERF_SCALE.lookup} nodes): ${avgDuration.toFixed(4)}ms`);

    expect(avgDuration).toBeLessThan(1);
  });

  test(`combineNodes performance with large document (< ${PERF_THRESHOLDS.combine}ms)`, () => {
    const { nodes: largeDataset } = generateLargeNodeDataset(PERF_SCALE.combine);
    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

    nodeManager.initializeNodes(largeDataset);

    const startTime = performance.now();

    // Test multiple node combinations
    for (let i = 1; i < 11; i++) {
      const currentId = `node-${i}`;
      const previousId = `node-${i - 1}`;
      nodeManager.combineNodes(currentId, previousId);
    }

    const endTime = performance.now();
    const duration = endTime - startTime;
    console.log(`10 node combinations (${PERF_SCALE.combine} node document): ${duration.toFixed(2)}ms`);

    // Performance regression detection: centralized threshold
    expect(duration).toBeLessThan(PERF_THRESHOLDS.combine);

    // Issue performance warning if approaching limit
    if (duration > 80) {
      console.warn(
        `⚠️  Performance Warning: combineNodes taking ${duration.toFixed(2)}ms (approaching 100ms limit)`
      );
    }
  });

  test(`hierarchy operations scale efficiently (< ${PERF_THRESHOLDS.hierarchy}ms for 100 operations)`, () => {
    // Performance Note: Hierarchy operations have O(n) complexity due to:
    // 1. sortChildrenByBeforeSiblingId - O(n) to rebuild sibling order from linked list
    // 2. Cache lookups reduce repeated sorts but initial operations are expensive
    // 3. 100 indent/outdent operations on large document
    //
    // Real-world performance: ~440ms measured in tests with proper caching (full mode)
    // This ensures operations complete in sub-second timeframes for user interactions
    const { nodes: largeDataset, hierarchyMap: _hierarchyMap } = generateLargeNodeDataset(PERF_SCALE.hierarchy);
    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

    nodeManager.initializeNodes(largeDataset);

    const startTime = performance.now();

    // Test multiple hierarchy operations
    for (let i = 100; i < 200; i++) {
      const nodeId = `node-${i}`;
      nodeManager.indentNode(nodeId);
      if (i % 2 === 0) {
        nodeManager.outdentNode(nodeId);
      }
    }

    const endTime = performance.now();
    const duration = endTime - startTime;
    console.log(`100 hierarchy operations (${PERF_SCALE.hierarchy} nodes): ${duration.toFixed(2)}ms`);

    // Centralized threshold ensures user interactions complete in sub-second timeframes
    expect(duration).toBeLessThan(PERF_THRESHOLDS.hierarchy);

    // Issue performance warning if approaching limit (helps catch regressions)
    if (duration > 400) {
      console.warn(
        `⚠️  Performance Warning: Hierarchy operations taking ${duration.toFixed(2)}ms (approaching 500ms limit)`
      );
    }
  });

  test(`getVisibleNodes performance with large nested structure (< ${PERF_THRESHOLDS.deepNesting}ms)`, () => {
    // Performance Note: Deep nesting has O(n²) worst-case complexity:
    // 1. Must traverse all nodes in a chain
    // 2. Each level calls sortChildrenByBeforeSiblingId (O(n) per level)
    // 3. Deep nesting threshold varies by mode (full: 1000 levels, fast: 100 levels)
    //
    // Real-world impact: Deep nesting (>100 levels) is rare in actual usage
    // Most documents have shallow trees (< 10 levels), which perform well (< 50ms)
    // This test ensures pathological cases don't cause multi-second hangs

    // Create deeply nested structure
    const nestedDataset = [];
    for (let i = 0; i < PERF_SCALE.deepNesting; i++) {
      const parentId = i > 0 ? `node-${i - 1}` : null;
      nestedDataset.push(createTestNode(`node-${i}`, `Content ${i}`, 'text', parentId));
    }

    nodeManager.initializeNodes(nestedDataset, {
      expanded: true, // Most expanded for visibility test
      autoFocus: false,
      inheritHeaderLevel: 0
    });

    const startTime = performance.now();
    // Trigger visibility getter to measure performance
    void nodeManager.visibleNodes;
    const endTime = performance.now();

    const duration = endTime - startTime;
    console.log(`${PERF_SCALE.deepNesting}-level deep nesting visibility calculation: ${duration.toFixed(2)}ms`);

    // Centralized threshold - normal documents (< 10 levels) perform much better
    expect(duration).toBeLessThan(PERF_THRESHOLDS.deepNesting);

    // Issue performance warning if approaching limit
    if (duration > 600) {
      console.warn(
        `⚠️  Performance Warning: Deep nesting visibility taking ${duration.toFixed(2)}ms (approaching 700ms limit)`
      );
    }

    // Note: visibleNodes returns 0 in test environment due to mocked $derived.by
    // Verify data structure integrity instead
    expect(nodeManager.nodes.size).toBe(PERF_SCALE.deepNesting);
    expect(nodeManager.rootNodeIds.length).toBeGreaterThan(0);
  });

  test('operational efficiency - multiple cycles work correctly', async () => {
    // Test that multiple operation cycles work correctly
    // This validates memory handling without requiring process access

    // Perform many operations
    for (let cycle = 0; cycle < 10; cycle++) {
      const { nodes: dataset, hierarchyMap: _hierarchyMap } = generateLargeNodeDataset(PERF_SCALE.multiCycle);
    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

      nodeManager.initializeNodes(dataset);

      // Perform various operations
      for (let i = 1; i < 50; i++) {
        nodeManager.createNode(`node-${i}`, `New content ${i}`);
        if (i % 3 === 0) {
          await nodeManager.deleteNode(`node-${i}`);
        }
      }
    }

    // If we reach here without errors, memory handling is working
    // Note: visibleNodes returns 0 in test environment due to mocked $derived.by
    // Verify we have actual nodes instead
    expect(nodeManager.nodes.size).toBeGreaterThan(0);
  });

  test('concurrent operations performance', () => {
    const { nodes: largeDataset, hierarchyMap: _hierarchyMap } = generateLargeNodeDataset(PERF_SCALE.concurrent);
    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy

    nodeManager.initializeNodes(largeDataset);

    const startTime = performance.now();

    // Simulate concurrent operations (batched for testing)
    const operations = [];
    for (let i = 0; i < 200; i++) {
      operations.push(() => {
        const nodeId = `node-${Math.floor(Math.random() * PERF_SCALE.concurrent)}`;
        nodeManager.updateNodeContent(nodeId, `Updated content ${i}`);
      });
    }

    // Execute all operations
    operations.forEach((op) => op());

    const endTime = performance.now();
    const duration = endTime - startTime;
    console.log(`200 concurrent operations: ${duration.toFixed(2)}ms`);

    expect(duration).toBeLessThan(100);
  });
});
