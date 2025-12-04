import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { initializeTauriSyncListeners } from '$lib/services/tauri-sync-listener';
import { SharedNodeStore, sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { Node } from '$lib/types';
import * as backendAdapterModule from '$lib/services/backend-adapter';

/**
 * Tests for Tauri Domain Event Listener
 *
 * Verifies that TauriSyncListener correctly handles domain events from the Rust backend
 * via Tauri's event system, ensuring real-time sync works correctly in desktop mode.
 *
 * ## Event Flow
 *
 * 1. Backend emits domain events via DomainEventForwarder
 * 2. Tauri event system forwards events to frontend
 * 3. TauriSyncListener handles events and updates stores
 *
 * ## Issue #724: ID-Only Events
 *
 * Events now send only node_id (not full payload). Tests mock backendAdapter.getNode
 * to return test data when frontend fetches node details.
 *
 * ## Test Coverage
 *
 * - Node events (created, updated, deleted)
 * - Edge events (hierarchy created, updated, deleted)
 * - Conditional fetching (nodeUpdated only fetches if node in store)
 * - Error handling for failed fetches
 * - Tauri environment detection
 */

/**
 * Helper to create test nodes with proper schema
 */
function createTestNode(id: string, content = 'Test node'): Node {
	return {
		id,
		nodeType: 'text',
		content,
		properties: {},
		mentions: [],
		createdAt: new Date().toISOString(),
		modifiedAt: new Date().toISOString(),
		version: 1
	};
}

/**
 * Mock node storage for backendAdapter.getNode
 */
const mockNodes = new Map<string, Node>();

/**
 * Mock Tauri event type
 */
interface MockTauriEvent<T = unknown> {
	payload: T;
}

/**
 * Mock Tauri event listeners storage
 * Maps event name to handler function
 */
const mockEventListeners = new Map<string, (event: MockTauriEvent) => void>();

/**
 * Setup mock for backendAdapter.getNode
 */
function setupMockGetNode() {
	vi.spyOn(backendAdapterModule.backendAdapter, 'getNode').mockImplementation(
		async (id: string) => {
			return mockNodes.get(id) || null;
		}
	);
}

/**
 * Register a node to be returned by mocked getNode
 */
function registerMockNode(node: Node) {
	mockNodes.set(node.id, node);
}

/**
 * Mock Tauri's listen function to capture event listeners
 */
function setupMockTauriListen() {
	// Mock @tauri-apps/api/event module
	vi.mock('@tauri-apps/api/event', () => ({
		listen: vi.fn(async (eventName: string, handler: (event: MockTauriEvent) => void) => {
			mockEventListeners.set(eventName, handler);
			// Return unsubscribe function (not used in tests)
			return () => {
				mockEventListeners.delete(eventName);
			};
		})
	}));
}

/**
 * Simulate emitting a Tauri event
 */
function emitTauriEvent(eventName: string, payload: unknown) {
	const handler = mockEventListeners.get(eventName);
	if (handler) {
		handler({ payload });
	} else {
		throw new Error(`No listener registered for event: ${eventName}`);
	}
}

/**
 * Mock Tauri environment detection
 */
function mockTauriEnvironment(isTauri: boolean) {
	interface WindowWithTauri extends Window {
		__TAURI__?: Record<string, unknown>;
	}

	if (isTauri) {
		(global.window as WindowWithTauri).__TAURI__ = {};
	} else {
		delete (global.window as WindowWithTauri).__TAURI__;
	}
}

describe('TauriSyncListener', () => {
	beforeEach(() => {
		// Reset stores
		SharedNodeStore.resetInstance();
		structureTree.children.clear();
		mockNodes.clear();
		mockEventListeners.clear();

		// Setup mocks
		setupMockGetNode();
		setupMockTauriListen();
		mockTauriEnvironment(true);
	});

	afterEach(() => {
		// Cleanup
		sharedNodeStore.clearAll();
		structureTree.children.clear();
		SharedNodeStore.resetInstance();
		mockNodes.clear();
		mockEventListeners.clear();
		vi.restoreAllMocks();
	});

	describe('Environment Detection', () => {
		it('should skip initialization when not in Tauri environment', async () => {
			mockTauriEnvironment(false);

			await initializeTauriSyncListeners();

			// No listeners should be registered
			expect(mockEventListeners.size).toBe(0);
		});

		it('should initialize listeners in Tauri environment', async () => {
			mockTauriEnvironment(true);

			await initializeTauriSyncListeners();

			// Verify all expected listeners are registered
			expect(mockEventListeners.has('node:created')).toBe(true);
			expect(mockEventListeners.has('node:updated')).toBe(true);
			expect(mockEventListeners.has('node:deleted')).toBe(true);
			expect(mockEventListeners.has('edge:created')).toBe(true);
			expect(mockEventListeners.has('edge:updated')).toBe(true);
			expect(mockEventListeners.has('edge:deleted')).toBe(true);
			expect(mockEventListeners.has('sync:error')).toBe(true);
			expect(mockEventListeners.has('sync:status')).toBe(true);
		});
	});

	describe('Node Events - Issue #724 ID-Only Optimization', () => {
		beforeEach(async () => {
			await initializeTauriSyncListeners();
		});

		it('should fetch and store node on node:created event', async () => {
			const testNode = createTestNode('node1', 'New node');
			registerMockNode(testNode);

			emitTauriEvent('node:created', { id: 'node1' });

			// Wait for async fetch to complete
			await vi.waitFor(() => {
				expect(sharedNodeStore.hasNode('node1')).toBe(true);
			});

			const storedNode = sharedNodeStore.getNode('node1');
			expect(storedNode).toBeDefined();
			expect(storedNode?.content).toBe('New node');
		});

		it('should always fetch node data on node:created (unconditional)', async () => {
			const testNode = createTestNode('node1');
			registerMockNode(testNode);

			// Node is NOT in store yet
			expect(sharedNodeStore.hasNode('node1')).toBe(false);

			emitTauriEvent('node:created', { id: 'node1' });

			// Should fetch even though node is not in store
			await vi.waitFor(() => {
				expect(backendAdapterModule.backendAdapter.getNode).toHaveBeenCalledWith('node1');
			});
		});

		it('should only fetch node:updated if node already in store', async () => {
			const testNode = createTestNode('node1', 'Original content');
			registerMockNode(testNode);

			// Pre-populate store with node
			sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test' }, false);

			// Update node in mock backend
			const updatedNode = { ...testNode, content: 'Updated content' };
			registerMockNode(updatedNode);

			emitTauriEvent('node:updated', { id: 'node1' });

			// Should fetch since node is in store
			await vi.waitFor(() => {
				const storedNode = sharedNodeStore.getNode('node1');
				expect(storedNode?.content).toBe('Updated content');
			});
		});

		it('should NOT fetch node:updated if node not in store', async () => {
			const testNode = createTestNode('node1');
			registerMockNode(testNode);

			// Node is NOT in store
			expect(sharedNodeStore.hasNode('node1')).toBe(false);

			emitTauriEvent('node:updated', { id: 'node1' });

			// Should NOT fetch since node is not in store
			await new Promise((resolve) => setTimeout(resolve, 50));
			expect(sharedNodeStore.hasNode('node1')).toBe(false);
		});

		it('should delete node on node:deleted event', async () => {
			const testNode = createTestNode('node1');
			sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test' }, false);

			expect(sharedNodeStore.hasNode('node1')).toBe(true);

			emitTauriEvent('node:deleted', { id: 'node1' });

			expect(sharedNodeStore.hasNode('node1')).toBe(false);
		});
	});

	describe('Edge Events - Hierarchy', () => {
		beforeEach(async () => {
			await initializeTauriSyncListeners();
		});

		it('should add hierarchy edge on edge:created', async () => {
			emitTauriEvent('edge:created', {
				type: 'hierarchy',
				parentId: 'parent1',
				childId: 'child1',
				order: 100
			});

			const children = structureTree.getChildrenWithOrder('parent1');
			expect(children).toHaveLength(1);
			expect(children[0].nodeId).toBe('child1');
			expect(children[0].order).toBe(100);
		});

		it('should not add duplicate edges (idempotent)', async () => {
			// Add edge first time
			emitTauriEvent('edge:created', {
				type: 'hierarchy',
				parentId: 'parent1',
				childId: 'child1',
				order: 100
			});

			// Try to add same edge again
			emitTauriEvent('edge:created', {
				type: 'hierarchy',
				parentId: 'parent1',
				childId: 'child1',
				order: 100
			});

			// Should still only have one child
			const children = structureTree.getChildrenWithOrder('parent1');
			expect(children).toHaveLength(1);
		});

		it('should remove hierarchy edge on edge:deleted', async () => {
			// Add edge first
			structureTree.addChild({
				parentId: 'parent1',
				childId: 'child1',
				order: 100
			});

			expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(1);

			// Delete edge
			emitTauriEvent('edge:deleted', {
				type: 'hierarchy',
				parentId: 'parent1',
				childId: 'child1',
				order: 0
			});

			expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(0);
		});

		it('should handle edge:updated for hierarchy (currently logs only)', async () => {
			// Add edge first
			structureTree.addChild({
				parentId: 'parent1',
				childId: 'child1',
				order: 100
			});

			// Update should not crash (currently just logs)
			expect(() => {
				emitTauriEvent('edge:updated', {
					type: 'hierarchy',
					parentId: 'parent1',
					childId: 'child1',
					order: 200
				});
			}).not.toThrow();
		});
	});

	describe('Edge Events - Mentions', () => {
		beforeEach(async () => {
			await initializeTauriSyncListeners();
		});

		it('should handle mention edge:created (logs only)', async () => {
			expect(() => {
				emitTauriEvent('edge:created', {
					type: 'mention',
					sourceId: 'node1',
					targetId: 'node2'
				});
			}).not.toThrow();
		});

		it('should handle mention edge:updated (logs only)', async () => {
			expect(() => {
				emitTauriEvent('edge:updated', {
					type: 'mention',
					sourceId: 'node1',
					targetId: 'node2'
				});
			}).not.toThrow();
		});

		it('should handle mention edge:deleted (logs only)', async () => {
			expect(() => {
				emitTauriEvent('edge:deleted', {
					type: 'mention',
					sourceId: 'node1',
					targetId: 'node2'
				});
			}).not.toThrow();
		});
	});

	describe('Error Handling', () => {
		beforeEach(async () => {
			await initializeTauriSyncListeners();
		});

		it('should handle failed node fetch gracefully', async () => {
			// Mock getNode to throw error
			vi.spyOn(backendAdapterModule.backendAdapter, 'getNode').mockRejectedValue(
				new Error('Network error')
			);

			// Should not throw
			expect(() => {
				emitTauriEvent('node:created', { id: 'node1' });
			}).not.toThrow();

			// Node should not be in store
			await new Promise((resolve) => setTimeout(resolve, 50));
			expect(sharedNodeStore.hasNode('node1')).toBe(false);
		});

		it('should handle node not found (returns null)', async () => {
			// Mock getNode to return null
			vi.spyOn(backendAdapterModule.backendAdapter, 'getNode').mockResolvedValue(null);

			emitTauriEvent('node:created', { id: 'nonexistent' });

			// Should not crash, node should not be in store
			await new Promise((resolve) => setTimeout(resolve, 50));
			expect(sharedNodeStore.hasNode('nonexistent')).toBe(false);
		});

		it('should handle sync:error events', async () => {
			// Should not crash
			expect(() => {
				emitTauriEvent('sync:error', {
					message: 'Database connection lost',
					errorType: 'connection'
				});
			}).not.toThrow();
		});

		it('should handle sync:status events', async () => {
			// Should not crash
			expect(() => {
				emitTauriEvent('sync:status', {
					status: 'connected',
					reason: 'Initial connection'
				});
			}).not.toThrow();
		});
	});

	describe('Event Ordering Scenarios', () => {
		beforeEach(async () => {
			await initializeTauriSyncListeners();
		});

		it('should handle edge created before node exists', async () => {
			// Edge event arrives first
			emitTauriEvent('edge:created', {
				type: 'hierarchy',
				parentId: 'parent1',
				childId: 'child1',
				order: 100
			});

			// Edge should be in structure tree
			expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(1);

			// Node event arrives later
			const testNode = createTestNode('child1');
			registerMockNode(testNode);
			emitTauriEvent('node:created', { id: 'child1' });

			// Node should be in store
			await vi.waitFor(() => {
				expect(sharedNodeStore.hasNode('child1')).toBe(true);
			});
		});

		it('should handle node deleted before edge deleted', async () => {
			// Setup: node and edge exist
			const testNode = createTestNode('child1');
			sharedNodeStore.setNode(testNode, { type: 'database', reason: 'test' }, false);
			structureTree.addChild({
				parentId: 'parent1',
				childId: 'child1',
				order: 100
			});

			// Node deleted first
			emitTauriEvent('node:deleted', { id: 'child1' });
			expect(sharedNodeStore.hasNode('child1')).toBe(false);

			// Edge deleted second (should not crash)
			expect(() => {
				emitTauriEvent('edge:deleted', {
					type: 'hierarchy',
					parentId: 'parent1',
					childId: 'child1',
					order: 0
				});
			}).not.toThrow();

			expect(structureTree.getChildrenWithOrder('parent1')).toHaveLength(0);
		});

		it('should handle multiple concurrent node creations', async () => {
			const node1 = createTestNode('node1');
			const node2 = createTestNode('node2');
			const node3 = createTestNode('node3');

			registerMockNode(node1);
			registerMockNode(node2);
			registerMockNode(node3);

			// Emit events rapidly
			emitTauriEvent('node:created', { id: 'node1' });
			emitTauriEvent('node:created', { id: 'node2' });
			emitTauriEvent('node:created', { id: 'node3' });

			// All nodes should be fetched and stored
			await vi.waitFor(() => {
				expect(sharedNodeStore.hasNode('node1')).toBe(true);
				expect(sharedNodeStore.hasNode('node2')).toBe(true);
				expect(sharedNodeStore.hasNode('node3')).toBe(true);
			});
		});
	});

	describe('Task Node Normalization', () => {
		beforeEach(async () => {
			await initializeTauriSyncListeners();
		});

		it('should normalize task nodes with flat status field', async () => {
			const taskNode: Node = {
				id: 'task1',
				nodeType: 'task',
				content: 'Test task',
				properties: {
					status: 'open',
					priority: 'high'
				},
				mentions: [],
				createdAt: new Date().toISOString(),
				modifiedAt: new Date().toISOString(),
				version: 1
			};

			registerMockNode(taskNode);
			emitTauriEvent('node:created', { id: 'task1' });

			await vi.waitFor(() => {
				const storedNode = sharedNodeStore.getNode('task1');
				expect(storedNode).toBeDefined();
				// After normalization, task nodes have flat status field
				interface TaskNode extends Node {
					status?: string;
				}
				expect((storedNode as TaskNode).status).toBeDefined();
			});
		});
	});
});
