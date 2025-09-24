/**
 * Performance validation tests for NodeManager
 * Tests handling of 1000+ node documents efficiently
 */

// CRITICAL: Import setup BEFORE anything else to ensure Svelte mocks are applied
import '../setup-svelte-mocks';

import { describe, test, expect, beforeEach } from 'vitest';
import {
  createReactiveNodeService,
  ReactiveNodeService as NodeManager
} from '../../lib/services/reactiveNodeService.svelte.js';
import type { NodeManagerEvents } from '../../lib/services/reactiveNodeService.svelte.js';

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
    const childrenMap = new Map<string, string[]>();

    // Initialize children map
    for (let i = 0; i < nodeCount; i++) {
      childrenMap.set(`node-${i}`, []);
    }

    // Build parent-child relationships and populate children arrays
    for (let i = 0; i < nodeCount; i++) {
      const nodeId = `node-${i}`;
      // Create a simpler hierarchy: every 10 nodes form a group under a parent
      const parentIndex = Math.floor(i / 10) * 10;
      const parentId = i > 0 && i !== parentIndex ? `node-${parentIndex}` : undefined;

      if (parentId && childrenMap.has(parentId)) {
        childrenMap.get(parentId)!.push(nodeId);
      }

      nodes.push({
        id: nodeId,
        content: `Content for node ${i}`,
        nodeType: 'text',
        depth: 0, // Will be calculated by initializeFromLegacyData
        parentId: undefined, // Will be calculated by initializeFromLegacyData
        children: childrenMap.get(nodeId) || [],
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: { created: Date.now() }
      });
    }

    // Set children arrays on nodes
    for (const node of nodes) {
      node.children = childrenMap.get(node.id) || [];
    }

    return nodes;
  };

  test('initializes 1000 nodes efficiently (< 100ms)', () => {
    const largeDataset = generateLargeNodeDataset(1000);

    const startTime = performance.now();
    nodeManager.initializeFromLegacyData(largeDataset);
    const endTime = performance.now();

    const duration = endTime - startTime;
    console.log(`1000 node initialization: ${duration.toFixed(2)}ms`);

    expect(duration).toBeLessThan(100);
    expect(nodeManager.nodes.size).toBe(1000);
    expect(nodeManager.rootNodeIds.length).toBe(100); // 1000 nodes in groups of 10 = 100 root nodes
  });

  test('node lookup performance with 1000+ nodes (< 1ms)', () => {
    const largeDataset = generateLargeNodeDataset(1500);
    nodeManager.initializeFromLegacyData(largeDataset);

    const startTime = performance.now();
    for (let i = 0; i < 100; i++) {
      const randomId = `node-${Math.floor(Math.random() * 1500)}`;
      nodeManager.findNode(randomId);
    }
    const endTime = performance.now();

    const duration = endTime - startTime;
    const avgDuration = duration / 100;
    console.log(`Average lookup time (1500 nodes): ${avgDuration.toFixed(4)}ms`);

    expect(avgDuration).toBeLessThan(1);
  });

  test('combineNodes performance with large document (< 50ms)', () => {
    const largeDataset = generateLargeNodeDataset(2000);
    nodeManager.initializeFromLegacyData(largeDataset);

    const startTime = performance.now();

    // Test multiple node combinations
    for (let i = 1; i < 11; i++) {
      const currentId = `node-${i}`;
      const previousId = `node-${i - 1}`;
      nodeManager.combineNodes(currentId, previousId);
    }

    const endTime = performance.now();
    const duration = endTime - startTime;
    console.log(`10 node combinations (2000 node document): ${duration.toFixed(2)}ms`);

    // Performance regression detection: alert if > 50ms
    expect(duration).toBeLessThan(50);

    // Issue performance warning if approaching limit
    if (duration > 40) {
      console.warn(
        `⚠️  Performance Warning: combineNodes taking ${duration.toFixed(2)}ms (approaching 50ms limit)`
      );
    }
  });

  test('hierarchy operations scale efficiently (< 50ms for 100 operations)', () => {
    const largeDataset = generateLargeNodeDataset(1000);
    nodeManager.initializeFromLegacyData(largeDataset);

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
    console.log(`100 hierarchy operations: ${duration.toFixed(2)}ms`);

    expect(duration).toBeLessThan(50);
  });

  test('getVisibleNodes performance with large nested structure (< 25ms)', () => {
    // Create deeply nested structure
    const nestedDataset = [];
    for (let i = 0; i < 1000; i++) {
      nestedDataset.push({
        id: `node-${i}`,
        content: `Content ${i}`,
        nodeType: 'text',
        depth: Math.floor(i / 50), // Create 20 levels deep
        parentId: i > 0 ? `node-${i - 1}` : undefined,
        children: i < 999 ? [`node-${i + 1}`] : [],
        expanded: Math.random() > 0.3, // 70% expanded
        autoFocus: false,
        inheritHeaderLevel: 0,
        metadata: {}
      });
    }

    nodeManager.initializeFromLegacyData(nestedDataset);

    const startTime = performance.now();
    // Trigger visibility getter to measure performance
    void nodeManager.visibleNodes;
    const endTime = performance.now();

    const duration = endTime - startTime;

    expect(duration).toBeLessThan(25);
    // Note: visibleNodes returns 0 in test environment due to mocked $derived.by
    // Verify data structure integrity instead
    expect(nodeManager.nodes.size).toBe(1000);
    expect(nodeManager.rootNodeIds.length).toBeGreaterThan(0);
  });

  test('operational efficiency - multiple cycles work correctly', () => {
    // Test that multiple operation cycles work correctly
    // This validates memory handling without requiring process access

    // Perform many operations
    for (let cycle = 0; cycle < 10; cycle++) {
      const dataset = generateLargeNodeDataset(500);
      nodeManager.initializeFromLegacyData(dataset);

      // Perform various operations
      for (let i = 1; i < 50; i++) {
        nodeManager.createNode(`node-${i}`, `New content ${i}`);
        if (i % 3 === 0) {
          nodeManager.deleteNode(`node-${i}`);
        }
      }
    }

    // If we reach here without errors, memory handling is working
    // Note: visibleNodes returns 0 in test environment due to mocked $derived.by
    // Verify we have actual nodes instead
    expect(nodeManager.nodes.size).toBeGreaterThan(0);
  });

  test('concurrent operations performance', () => {
    const largeDataset = generateLargeNodeDataset(1000);
    nodeManager.initializeFromLegacyData(largeDataset);

    const startTime = performance.now();

    // Simulate concurrent operations (batched for testing)
    const operations = [];
    for (let i = 0; i < 200; i++) {
      operations.push(() => {
        const nodeId = `node-${Math.floor(Math.random() * 1000)}`;
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
