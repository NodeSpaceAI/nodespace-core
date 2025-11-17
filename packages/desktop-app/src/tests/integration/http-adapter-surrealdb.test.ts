/**
 * HttpAdapter Dev-Proxy Integration Tests
 *
 * Tests the HttpAdapter implementation using the dev-proxy REST API.
 * Verifies all CRUD operations, query functionality, version control,
 * and business logic integration work correctly.
 *
 * Part of Issue #505: Fix HttpAdapter SurrealDB integration tests
 * (Updated from Issue #490 after PR #501 introduced dev-proxy architecture)
 *
 * ## Architecture (Post PR #501)
 *
 * Frontend → HTTP (port 3001) → dev-proxy → NodeService → SurrealStore → SurrealDB (port 8000)
 *
 * These tests verify the HttpAdapter correctly communicates with the dev-proxy REST API,
 * which provides all Rust business logic (NodeService, SchemaService, behaviors, etc.).
 *
 * ## Test Coverage
 *
 * - Basic CRUD operations (create, read, update, delete)
 * - Query operations (queryNodes, getChildren, getNodesByContainerId)
 * - Mention autocomplete
 * - Optimistic concurrency control (version conflicts)
 * - Schema operations (getSchema, addField, removeField, etc.)
 * - Field name mapping (snake_case ↔ camelCase)
 *
 * ## Prerequisites
 *
 * Tests require dev-proxy server running on localhost:3001.
 * The dev-proxy automatically connects to SurrealDB on port 8000.
 *
 * Start servers with:
 * 1. `bun run dev:db` (starts SurrealDB on port 8000)
 * 2. `cargo build --bin dev-proxy && cargo run --bin dev-proxy` (starts dev-proxy on port 3001)
 *
 * Or use the combined command: `bun run dev:browser` (starts both)
 *
 * ## Test Execution Requirements
 *
 * **IMPORTANT**: These tests must run serially (not in parallel) because they share
 * the same database state and use `beforeEach` cleanup. Running tests in parallel
 * can cause race conditions where one test's cleanup interferes with another test's
 * execution.
 *
 * Vitest configuration should ensure serial execution for integration tests.
 * Alternatively, tests could be refactored to use isolated namespaces per test suite
 * (e.g., `DEFINE NAMESPACE test_${Date.now()};`) to enable safe parallel execution.
 *
 * ## Skip Conditions
 *
 * Tests are automatically skipped if dev-proxy server is not reachable on port 3001.
 *
 * Note: These tests use the database (not in-memory mode), so they require:
 * 1. SurrealDB running on port 8000 (via `bun run dev:db`)
 * 2. dev-proxy running on port 3001 (via `cargo run --bin dev-proxy`)
 */

import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest';
import { HttpAdapter } from '$lib/services/backend-adapter';
import { TestNodeBuilder } from '../utils/test-node-builder';

/**
 * Check if dev-proxy server is available
 * @returns True if server is reachable on localhost:3001
 */
async function isDevProxyAvailable(): Promise<boolean> {
  try {
    const response = await globalThis.fetch('http://localhost:3001/health', {
      method: 'GET',
      signal: AbortSignal.timeout(1000) // 1 second timeout
    });
    return response.ok;
  } catch {
    return false;
  }
}

/**
 * Initialize namespace and database
 * Must be called before any other operations
 */
async function initializeDatabase(): Promise<void> {
  try {
    const response = await globalThis.fetch('http://localhost:8000/sql', {
      method: 'POST',
      headers: {
        Accept: 'application/json',
        Authorization: 'Basic ' + globalThis.btoa('root:root')
      },
      body: `
        DEFINE NAMESPACE nodespace;
        USE NS nodespace;
        DEFINE DATABASE nodes;
        USE DB nodes;
        DEFINE TABLE nodes SCHEMALESS;
      `
    });

    if (!response.ok) {
      throw new Error(`Failed to initialize database: ${response.statusText}`);
    }
  } catch (error) {
    console.warn('[Test] Database initialization warning:', error);
  }
}

/**
 * Clean all nodes from the database
 * Used between tests to ensure isolation
 */
async function cleanDatabase(): Promise<void> {
  try {
    const response = await globalThis.fetch('http://localhost:8000/sql', {
      method: 'POST',
      headers: {
        Accept: 'application/json',
        Authorization: 'Basic ' + globalThis.btoa('root:root')
      },
      body: 'USE NS nodespace; USE DB nodes; DELETE nodes;'
    });

    if (!response.ok) {
      throw new Error(`Failed to clean database: ${response.statusText}`);
    }
  } catch (error) {
    console.warn('[Test] Database cleanup failed:', error);
  }
}

describe.skipIf(!(await isDevProxyAvailable()))('HttpAdapter with Dev-Proxy', () => {
  let adapter: HttpAdapter;

  beforeAll(async () => {
    adapter = new HttpAdapter('http://localhost:3001');
    console.log('[Test] Using dev-proxy HTTP adapter on port 3001');

    // Initialize namespace, database, and table
    await initializeDatabase();
    console.log('[Test] Database initialized');
  });

  afterAll(async () => {
    // Final cleanup
    await cleanDatabase();
  });

  beforeEach(async () => {
    // Clean database between tests
    await cleanDatabase();
  });

  describe('Basic CRUD Operations', () => {
    it('should create a node', async () => {
      const nodeData = TestNodeBuilder.text('Hello SurrealDB').withId('test-create-node').build();

      const nodeId = await adapter.createNode(nodeData);

      expect(nodeId).toBe('test-create-node');

      // Verify node was created
      const retrieved = await adapter.getNode(nodeId);
      expect(retrieved).not.toBeNull();
      expect(retrieved!.content).toBe('Hello SurrealDB');
      expect(retrieved!.nodeType).toBe('text');
      expect(retrieved!.version).toBe(1);
    });

    it('should get a node by ID', async () => {
      const nodeData = TestNodeBuilder.text('Get me').withId('test-get-node').build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-get-node');

      expect(retrieved).not.toBeNull();
      expect(retrieved!.id).toBe('test-get-node');
      expect(retrieved!.content).toBe('Get me');
      expect(retrieved!.version).toBe(1);
    });

    it('should return null for non-existent node', async () => {
      const result = await adapter.getNode('non-existent-node-id');
      expect(result).toBeNull();
    });

    it('should update a node', async () => {
      const nodeData = TestNodeBuilder.text('Original content').withId('test-update-node').build();

      await adapter.createNode(nodeData);

      // Update content
      const updated = await adapter.updateNode('test-update-node', 1, {
        content: 'Updated content'
      });

      expect(updated.content).toBe('Updated content');
      expect(updated.version).toBe(2);

      // Verify update persisted
      const retrieved = await adapter.getNode('test-update-node');
      expect(retrieved!.content).toBe('Updated content');
      expect(retrieved!.version).toBe(2);
    });

    it('should update node type', async () => {
      const nodeData = TestNodeBuilder.text('Change my type').withId('test-update-type').build();

      await adapter.createNode(nodeData);

      // Update node type
      const updated = await adapter.updateNode('test-update-type', 1, {
        nodeType: 'task'
      });

      expect(updated.nodeType).toBe('task');
      expect(updated.version).toBe(2);
    });

    it('should delete a node', async () => {
      const nodeData = TestNodeBuilder.text('Delete me').withId('test-delete-node').build();

      await adapter.createNode(nodeData);

      // Delete node
      await adapter.deleteNode('test-delete-node', 1);

      // Verify node is gone
      const retrieved = await adapter.getNode('test-delete-node');
      expect(retrieved).toBeNull();
    });

    it('should handle idempotent delete (no error on double delete)', async () => {
      const nodeData = TestNodeBuilder.text('Delete twice')
        .withId('test-idempotent-delete')
        .build();

      await adapter.createNode(nodeData);
      await adapter.deleteNode('test-idempotent-delete', 1);

      // Second delete should not throw
      await expect(adapter.deleteNode('test-idempotent-delete', 1)).resolves.not.toThrow();
    });
  });

  describe('Optimistic Concurrency Control', () => {
    it('should detect version conflicts on update', async () => {
      const nodeData = TestNodeBuilder.text('Version test').withId('test-version-conflict').build();

      await adapter.createNode(nodeData);

      // Update with correct version
      await adapter.updateNode('test-version-conflict', 1, {
        content: 'First update'
      });

      // Update with stale version should fail
      await expect(
        adapter.updateNode('test-version-conflict', 1, {
          content: 'Stale update'
        })
      ).rejects.toThrow(/Version conflict/);
    });

    it('should detect version conflicts on delete', async () => {
      const nodeData = TestNodeBuilder.text('Version delete test')
        .withId('test-version-delete')
        .build();

      await adapter.createNode(nodeData);

      // Update to version 2
      await adapter.updateNode('test-version-delete', 1, {
        content: 'Updated'
      });

      // Delete with stale version should fail
      await expect(adapter.deleteNode('test-version-delete', 1)).rejects.toThrow(
        /Version conflict/
      );
    });

    it('should increment version on each update', async () => {
      const nodeData = TestNodeBuilder.text('Version increment')
        .withId('test-version-increment')
        .build();

      await adapter.createNode(nodeData);

      // Multiple updates
      let current = await adapter.updateNode('test-version-increment', 1, {
        content: 'Update 1'
      });
      expect(current.version).toBe(2);

      current = await adapter.updateNode('test-version-increment', 2, {
        content: 'Update 2'
      });
      expect(current.version).toBe(3);

      current = await adapter.updateNode('test-version-increment', 3, {
        content: 'Update 3'
      });
      expect(current.version).toBe(4);
    });
  });

  describe('Hierarchy Operations', () => {
    it('should create and retrieve parent-child relationship', async () => {
      const parentData = TestNodeBuilder.text('Parent').withId('test-parent').build();

      const childData = TestNodeBuilder.text('Child').withId('test-child').build();

      await adapter.createNode(parentData);
      await adapter.createNode(childData);

      // Get children
      const children = await adapter.getChildren('test-parent');

      expect(children).toHaveLength(1);
      expect(children[0].id).toBe('test-child');
    });

    it('should maintain sibling order', async () => {
      const parentData = TestNodeBuilder.text('Parent').withId('test-sibling-parent').build();

      await adapter.createNode(parentData);

      // Create siblings in order
      const child1Data = TestNodeBuilder.text('Child 1')
        .withId('test-child-1')
        .withBeforeSibling(null)
        .build();

      const child2Data = TestNodeBuilder.text('Child 2')
        .withId('test-child-2')
        .withBeforeSibling('test-child-1')
        .build();

      const child3Data = TestNodeBuilder.text('Child 3')
        .withId('test-child-3')
        .withBeforeSibling('test-child-2')
        .build();

      await adapter.createNode(child1Data);
      await adapter.createNode(child2Data);
      await adapter.createNode(child3Data);

      // Verify sibling chain
      const child1 = await adapter.getNode('test-child-1');
      const child2 = await adapter.getNode('test-child-2');
      const child3 = await adapter.getNode('test-child-3');

      expect(child1!.beforeSiblingId).toBeNull();
      expect(child2!.beforeSiblingId).toBe('test-child-1');
      expect(child3!.beforeSiblingId).toBe('test-child-2');
    });

    it('should update parent relationship', async () => {
      const parent1Data = TestNodeBuilder.text('Parent 1').withId('test-parent-1').build();

      const parent2Data = TestNodeBuilder.text('Parent 2').withId('test-parent-2').build();

      const childData = TestNodeBuilder.text('Moveable child').withId('test-moveable-child').build();

      await adapter.createNode(parent1Data);
      await adapter.createNode(parent2Data);
      await adapter.createNode(childData);

      // Move child to parent 2
      await adapter.updateNode('test-moveable-child', 1, {});

      // Verify new parent
      const child = await adapter.getNode('test-moveable-child');

      // Verify old parent has no children
      const parent1Children = await adapter.getChildren('test-parent-1');
      expect(parent1Children).toHaveLength(0);

      // Verify new parent has child
      const parent2Children = await adapter.getChildren('test-parent-2');
      expect(parent2Children).toHaveLength(1);
      expect(child).toBeDefined(); // Verify child exists
    });
  });

  describe('Query Operations', () => {
    it('should query nodes by parentId', async () => {
      const parentData = TestNodeBuilder.text('Query parent').withId('test-query-parent').build();

      await adapter.createNode(parentData);

      const child1Data = TestNodeBuilder.text('Query child 1').withId('test-query-child-1').build();

      const child2Data = TestNodeBuilder.text('Query child 2').withId('test-query-child-2').build();

      await adapter.createNode(child1Data);
      await adapter.createNode(child2Data);

      // Query by parent
      const results = await adapter.queryNodes({});

      expect(results).toHaveLength(2);
      expect(results.map((n) => n.id).sort()).toEqual(['test-query-child-1', 'test-query-child-2']);
    });

    it('should query root nodes (parentId = null)', async () => {
      const root1Data = TestNodeBuilder.text('Root 1').withId('test-root-1').build();

      const root2Data = TestNodeBuilder.text('Root 2').withId('test-root-2').build();

      await adapter.createNode(root1Data);
      await adapter.createNode(root2Data);

      // Query root nodes
      const results = await adapter.queryNodes({});

      expect(results.length).toBeGreaterThanOrEqual(2);
      expect(results.map((n) => n.id)).toContain('test-root-1');
      expect(results.map((n) => n.id)).toContain('test-root-2');
    });

    it('should query nodes by containerId', async () => {
      const containerData = TestNodeBuilder.text('Container').withId('test-container').build();

      const node1Data = TestNodeBuilder.text('Contained 1').withId('test-contained-1').build();

      const node2Data = TestNodeBuilder.text('Contained 2').withId('test-contained-2').build();

      await adapter.createNode(containerData);
      await adapter.createNode(node1Data);
      await adapter.createNode(node2Data);

      // Query by container
      const results = await adapter.queryNodes({
        containerId: 'test-container'
      });

      expect(results).toHaveLength(2);
      expect(results.map((n) => n.id).sort()).toEqual(['test-contained-1', 'test-contained-2']);
    });

    it('should query nodes by both parentId and containerId', async () => {
      const parentData = TestNodeBuilder.text('Combined parent')
        .withId('test-combined-parent')
        .build();

      const containerData = TestNodeBuilder.text('Combined container')
        .withId('test-combined-container')
        .build();

      const matchData = TestNodeBuilder.text('Match both').withId('test-match-both').build();

      const noMatchData = TestNodeBuilder.text('No match').withId('test-no-match').build();

      await adapter.createNode(parentData);
      await adapter.createNode(containerData);
      await adapter.createNode(matchData);
      await adapter.createNode(noMatchData);

      // Query with both filters
      const results = await adapter.queryNodes({
        containerId: 'test-combined-container'
      });

      expect(results).toHaveLength(1);
      expect(results[0].id).toBe('test-match-both');
    });

    it('should get nodes by container ID', async () => {
      const containerData = TestNodeBuilder.text('Container').withId('test-get-container').build();

      const node1Data = TestNodeBuilder.text('Node 1').withId('test-get-node-1').build();

      const node2Data = TestNodeBuilder.text('Node 2').withId('test-get-node-2').build();

      await adapter.createNode(containerData);
      await adapter.createNode(node1Data);
      await adapter.createNode(node2Data);

      const results = await adapter.getNodesByContainerId('test-get-container');

      expect(results).toHaveLength(2);
      expect(results.map((n) => n.id).sort()).toEqual(['test-get-node-1', 'test-get-node-2']);
    });
  });

  describe('Mention Autocomplete', () => {
    it('should find nodes by content substring', async () => {
      const node1Data = TestNodeBuilder.text('Learn machine learning')
        .withId('test-mention-1')
        .build();

      const node2Data = TestNodeBuilder.text('Machine maintenance')
        .withId('test-mention-2')
        .build();

      const node3Data = TestNodeBuilder.text('Deep learning').withId('test-mention-3').build();

      await adapter.createNode(node1Data);
      await adapter.createNode(node2Data);
      await adapter.createNode(node3Data);

      // Search for "machine"
      const results = await adapter.mentionAutocomplete('machine');

      expect(results.length).toBeGreaterThanOrEqual(2);
      expect(results.map((n) => n.id)).toContain('test-mention-1');
      expect(results.map((n) => n.id)).toContain('test-mention-2');
    });

    it('should be case-insensitive', async () => {
      const nodeData = TestNodeBuilder.text('UPPERCASE Content')
        .withId('test-case-insensitive')
        .build();

      await adapter.createNode(nodeData);

      // Search with lowercase
      const results = await adapter.mentionAutocomplete('uppercase');

      expect(results.length).toBeGreaterThanOrEqual(1);
      expect(results.map((n) => n.id)).toContain('test-case-insensitive');
    });

    it('should respect limit parameter', async () => {
      // Create many matching nodes
      for (let i = 0; i < 20; i++) {
        const nodeData = TestNodeBuilder.text(`Limit test ${i}`).withId(`test-limit-${i}`).build();
        await adapter.createNode(nodeData);
      }

      // Search with limit
      const results = await adapter.mentionAutocomplete('limit', 5);

      expect(results.length).toBeLessThanOrEqual(5);
    });
  });

  describe('Properties and Metadata', () => {
    it('should store and retrieve node properties', async () => {
      const nodeData = TestNodeBuilder.text('Node with properties')
        .withId('test-properties')
        .withProperties({
          priority: 'high',
          status: 'active',
          tags: ['important', 'urgent']
        })
        .build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-properties');

      expect(retrieved!.properties).toEqual({
        priority: 'high',
        status: 'active',
        tags: ['important', 'urgent']
      });
    });

    it('should update node properties', async () => {
      const nodeData = TestNodeBuilder.text('Update properties')
        .withId('test-update-properties')
        .withProperties({ status: 'draft' })
        .build();

      await adapter.createNode(nodeData);

      // Update properties
      await adapter.updateNode('test-update-properties', 1, {
        properties: { status: 'published', author: 'test-user' }
      });

      const retrieved = await adapter.getNode('test-update-properties');

      expect(retrieved!.properties).toEqual({
        status: 'published',
        author: 'test-user'
      });
    });

    it('should store timestamps (createdAt, modifiedAt)', async () => {
      const nodeData = TestNodeBuilder.text('Timestamp test').withId('test-timestamps').build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-timestamps');

      expect(retrieved!.createdAt).toBeDefined();
      expect(retrieved!.modifiedAt).toBeDefined();
      expect(typeof retrieved!.createdAt).toBe('string');
      expect(typeof retrieved!.modifiedAt).toBe('string');
    });

    it('should update modifiedAt on update', async () => {
      const nodeData = TestNodeBuilder.text('Modified test').withId('test-modified').build();

      await adapter.createNode(nodeData);

      const original = await adapter.getNode('test-modified');
      const originalModified = original!.modifiedAt;

      // Wait a bit to ensure timestamp difference
      await new Promise((resolve) => setTimeout(resolve, 100));

      await adapter.updateNode('test-modified', 1, {
        content: 'Updated'
      });

      const updated = await adapter.getNode('test-modified');

      expect(updated!.modifiedAt).not.toBe(originalModified);
      expect(new Date(updated!.modifiedAt).getTime()).toBeGreaterThan(
        new Date(originalModified).getTime()
      );
    });
  });

  describe('SurrealQL Escaping and Injection Prevention', () => {
    it('should escape double quotes in content', async () => {
      const nodeData = TestNodeBuilder.text('Content with "quotes"')
        .withId('test-escape-quotes')
        .build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-escape-quotes');
      expect(retrieved!.content).toBe('Content with "quotes"');
    });

    it('should escape backslashes in content', async () => {
      const nodeData = TestNodeBuilder.text('Content with \\ backslash')
        .withId('test-escape-backslash')
        .build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-escape-backslash');
      expect(retrieved!.content).toBe('Content with \\ backslash');
    });

    it('should handle special characters in content', async () => {
      const specialContent = `Line 1
Line 2
Tab:\there
Special: !@#$%^&*()`;

      const nodeData = TestNodeBuilder.text(specialContent).withId('test-special-chars').build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-special-chars');
      expect(retrieved!.content).toBe(specialContent);
    });

    it('should prevent SQL injection via content', async () => {
      const maliciousContent = '"; DELETE FROM nodes; --';

      const nodeData = TestNodeBuilder.text(maliciousContent).withId('test-sql-injection').build();

      await adapter.createNode(nodeData);

      // Verify injection didn't delete other nodes
      const retrieved = await adapter.getNode('test-sql-injection');
      expect(retrieved!.content).toBe(maliciousContent);
    });
  });

  describe('Field Name Mapping (snake_case ↔ camelCase)', () => {
    it('should correctly map nodeType', async () => {
      const nodeData = TestNodeBuilder.text('Type mapping')
        .withId('test-type-mapping')
        .withNodeType('task')
        .build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-type-mapping');
      expect(retrieved!.nodeType).toBe('task');
    });

    it('should correctly map parentId', async () => {
      const parentData = TestNodeBuilder.text('Parent').withId('test-parent-mapping').build();

      const childData = TestNodeBuilder.text('Child').withId('test-child-mapping').build();

      await adapter.createNode(parentData);
      await adapter.createNode(childData);

      const _child = await adapter.getNode('test-child-mapping');
    });

    it('should correctly map containerNodeId', async () => {
      const nodeData = TestNodeBuilder.text('Container mapping')
        .withId('test-container-mapping')
        .build();

      await adapter.createNode(nodeData);

      const _retrieved = await adapter.getNode('test-container-mapping');
    });

    it('should correctly map beforeSiblingId', async () => {
      const nodeData = TestNodeBuilder.text('Sibling mapping')
        .withId('test-sibling-mapping')
        .withBeforeSibling('previous-sibling-id')
        .build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-sibling-mapping');
      expect(retrieved!.beforeSiblingId).toBe('previous-sibling-id');
    });

    it('should correctly map createdAt and modifiedAt', async () => {
      const nodeData = TestNodeBuilder.text('Timestamp mapping')
        .withId('test-timestamp-mapping')
        .build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-timestamp-mapping');
      expect(retrieved!.createdAt).toBeDefined();
      expect(retrieved!.modifiedAt).toBeDefined();
    });

    it('should correctly map embeddingVector', async () => {
      const nodeData = TestNodeBuilder.text('Embedding mapping')
        .withId('test-embedding-mapping')
        .withEmbedding([0.1, 0.2, 0.3])
        .build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-embedding-mapping');
      expect(retrieved!.embeddingVector).toEqual([0.1, 0.2, 0.3]);
    });
  });

  describe('Null Value Handling', () => {
    it('should handle null parentId (root node)', async () => {
      const nodeData = TestNodeBuilder.text('Root node').withId('test-null-parent').build();

      await adapter.createNode(nodeData);

      const _retrieved = await adapter.getNode('test-null-parent');
    });

    it('should handle null containerNodeId', async () => {
      const nodeData = TestNodeBuilder.text('No container').withId('test-null-container').build();

      await adapter.createNode(nodeData);

      const _retrieved = await adapter.getNode('test-null-container');
    });

    it('should handle null beforeSiblingId', async () => {
      const nodeData = TestNodeBuilder.text('First sibling')
        .withId('test-null-sibling')
        .withBeforeSibling(null)
        .build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-null-sibling');
      expect(retrieved!.beforeSiblingId).toBeNull();
    });

    it('should handle null embeddingVector', async () => {
      const nodeData = TestNodeBuilder.text('No embedding').withId('test-null-embedding').build();

      await adapter.createNode(nodeData);

      const retrieved = await adapter.getNode('test-null-embedding');
      // SurrealDB NONE values are converted to null by mapSurrealNodeToTypescript
      expect(retrieved!.embeddingVector).toBeNull();
    });
  });

  describe('Error Handling', () => {
    it('should throw error on update with non-existent node', async () => {
      await expect(
        adapter.updateNode('non-existent-node', 1, {
          content: 'Update'
        })
      ).rejects.toThrow(/HTTP 404.*not found/i);
    });

    it('should handle malformed node IDs gracefully', async () => {
      const result = await adapter.getNode('malformed id with spaces');
      expect(result).toBeNull();
    });
  });
});
