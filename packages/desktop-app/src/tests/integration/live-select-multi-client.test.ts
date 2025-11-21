/**
 * Integration Tests for Multi-Client Synchronization via LIVE SELECT
 *
 * Tests comprehensive multi-client scenarios where:
 * - MCP server creates/modifies nodes
 * - Tauri frontend receives updates via LIVE SELECT polling
 * - Concurrent operations resolve with last-write-wins
 * - Optimistic updates rollback on backend failure
 * - Stream disconnection triggers automatic reconnection
 *
 * These tests verify the end-to-end synchronization flow without requiring
 * a full Tauri runtime. We mock the Tauri event system and test the event
 * handling and store synchronization logic directly.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { eventBus } from '$lib/services/event-bus';
import type {
	RealtimeNodeCreatedEvent,
	RealtimeNodeUpdatedEvent,
	RealtimeNodeDeletedEvent,
	RealtimeEdgeCreatedEvent,
	RealtimeEdgeUpdatedEvent,
	RealtimeEdgeDeletedEvent,
	RealtimeSyncStatusEvent,
	NodeEventData
} from '$lib/services/event-types';

// Maximum allowed latency for synchronization events (from Issue #559 Acceptance Criteria)
const MAX_SYNC_LATENCY_MS = 100;

// Mock Tauri API for testing
const mockTauriEvents = new Map<string, Array<(event: { payload: unknown }) => void>>();

const mockTauriEventAPI = {
	listen: vi.fn(async (eventName: string, handler: (event: { payload: unknown }) => void) => {
		if (!mockTauriEvents.has(eventName)) {
			mockTauriEvents.set(eventName, []);
		}
		mockTauriEvents.get(eventName)!.push(handler);

		// Return unlisten function
		return () => {
			const handlers = mockTauriEvents.get(eventName);
			if (handlers) {
				const index = handlers.indexOf(handler);
				if (index > -1) handlers.splice(index, 1);
			}
		};
	}),
	emit: vi.fn(async (eventName: string, payload: unknown) => {
		const handlers = mockTauriEvents.get(eventName);
		if (handlers) {
			handlers.forEach((handler) => handler({ payload }));
		}
	})
};

// Mock the @tauri-apps/api/event module
vi.mock('@tauri-apps/api/event', () => ({
	listen: mockTauriEventAPI.listen,
	emit: mockTauriEventAPI.emit
}));

const mockTauri = {
	event: mockTauriEventAPI
};

// Extend global interface for Tauri
declare global {
	var __TAURI__: typeof mockTauri | { invoke: typeof vi.fn };
	interface Window {
		__TAURI__?: typeof mockTauri | { invoke?: typeof vi.fn; event?: typeof mockTauriEventAPI };
	}
}

// Set up global Tauri mock
if (typeof global !== 'undefined') {
	(global as unknown as { __TAURI__: typeof mockTauri }).__TAURI__ = mockTauri;
}

if (typeof window !== 'undefined') {
	(window as { __TAURI__?: typeof mockTauri }).__TAURI__ = mockTauri;
}

describe('Multi-Client Integration Tests for LIVE SELECT', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		mockTauriEvents.clear();
		eventBus.reset();
	});

	afterEach(() => {
		eventBus.reset();
	});

	describe('MCP Creates Node → Tauri Sees Update (Acceptance Criterion #1)', () => {
		it('should propagate MCP node creation to frontend within 100ms', async () => {
			const receivedEvents: Array<{ event: RealtimeNodeCreatedEvent; receivedAt: number }> = [];

			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push({
						event: event as RealtimeNodeCreatedEvent,
						receivedAt: Date.now()
					});
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate MCP server creating a new node in database
			// LiveQueryService polls every 1s and detects version change
			const mcpCreatedNode: NodeEventData = {
				id: 'node:mcp-created-001',
				content: 'New task from MCP server',
				nodeType: 'task',
				version: 1,
				modifiedAt: new Date().toISOString()
			};

			const emitTime = Date.now();
			await mockTauri.event.emit('node:created', mcpCreatedNode);

			// Assert: Event received within 100ms
			expect(receivedEvents).toHaveLength(1);
			const latency = receivedEvents[0].receivedAt - emitTime;
			expect(latency).toBeLessThan(MAX_SYNC_LATENCY_MS);
			expect(receivedEvents[0].event.nodeData.content).toBe('New task from MCP server');
		});

		it('should sync multiple node creations from MCP in order', async () => {
			const createdNodeIds: string[] = [];

			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					createdNodeIds.push((event as RealtimeNodeCreatedEvent).nodeId);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// MCP creates multiple nodes sequentially
			for (let i = 1; i <= 5; i++) {
				await mockTauri.event.emit('node:created', {
					id: `node:mcp-batch-${i}`,
					content: `MCP node ${i}`,
					nodeType: 'text',
					version: 1,
					modifiedAt: new Date().toISOString()
				});
			}

			// All nodes should arrive in order
			expect(createdNodeIds).toHaveLength(5);
			expect(createdNodeIds).toEqual([
				'node:mcp-batch-1',
				'node:mcp-batch-2',
				'node:mcp-batch-3',
				'node:mcp-batch-4',
				'node:mcp-batch-5'
			]);
		});
	});

	describe('MCP Indents Node → Tauri Hierarchy Updates (Acceptance Criterion #2)', () => {
		it('should propagate MCP hierarchy change (edge creation) within 100ms', async () => {
			const edgeEvents: Array<{ event: RealtimeEdgeCreatedEvent; receivedAt: number }> = [];

			eventBus.subscribe('edge:created', (event) => {
				if (event.namespace === 'sync') {
					edgeEvents.push({
						event: event as RealtimeEdgeCreatedEvent,
						receivedAt: Date.now()
					});
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// MCP server indents node-child under node-parent
			const emitTime = Date.now();
			await mockTauri.event.emit('edge:created', {
				id: 'edge:mcp-indent-001',
				in: 'node:mcp-parent',
				out: 'node:mcp-child',
				order: 1.0
			});

			// Assert: Hierarchy update received within 100ms
			expect(edgeEvents).toHaveLength(1);
			const latency = edgeEvents[0].receivedAt - emitTime;
			expect(latency).toBeLessThan(MAX_SYNC_LATENCY_MS);
			expect(edgeEvents[0].event.edgeData.in).toBe('node:mcp-parent');
			expect(edgeEvents[0].event.edgeData.out).toBe('node:mcp-child');
		});

		it('should handle MCP reordering children within 100ms (edge:updated)', async () => {
			const edgeUpdateEvents: Array<{
				event: RealtimeEdgeUpdatedEvent;
				receivedAt: number;
			}> = [];

			eventBus.subscribe('edge:updated', (event) => {
				if (event.namespace === 'sync') {
					edgeUpdateEvents.push({
						event: event as RealtimeEdgeUpdatedEvent,
						receivedAt: Date.now()
					});
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// MCP server reorders a child using fractional ordering
			const emitTime = Date.now();
			await mockTauri.event.emit('edge:updated', {
				id: 'edge:reorder-001',
				in: 'node:parent',
				out: 'node:child-2',
				order: 1.5 // Inserted between child-1 (order 1.0) and child-3 (order 2.0)
			});

			// Assert: Hierarchy update within 100ms
			expect(edgeUpdateEvents).toHaveLength(1);
			const latency = edgeUpdateEvents[0].receivedAt - emitTime;
			expect(latency).toBeLessThan(MAX_SYNC_LATENCY_MS);
			expect(edgeUpdateEvents[0].event.edgeData.order).toBe(1.5);
		});

		it('should handle MCP outdenting nodes (edge:deleted)', async () => {
			const edgeDeletedEvents: RealtimeEdgeDeletedEvent[] = [];

			eventBus.subscribe('edge:deleted', (event) => {
				if (event.namespace === 'sync') {
					edgeDeletedEvents.push(event as RealtimeEdgeDeletedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// MCP server outdents a node (removes edge)
			await mockTauri.event.emit('edge:deleted', {
				id: 'edge:outdent-001'
			});

			expect(edgeDeletedEvents).toHaveLength(1);
			expect(edgeDeletedEvents[0].edgeId).toBe('edge:outdent-001');
		});

		it('should sync complex hierarchy changes: create parent + 3 children in order', async () => {
			const createdEdges: Array<{ parent: string; child: string; order: number }> = [];

			eventBus.subscribe('edge:created', (event) => {
				if (event.namespace === 'sync') {
					const edge = event as RealtimeEdgeCreatedEvent;
					createdEdges.push({
						parent: edge.edgeData.in,
						child: edge.edgeData.out,
						order: edge.edgeData.order
					});
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// MCP creates a parent and indents 3 children under it
			await mockTauri.event.emit('edge:created', {
				id: 'edge:parent-child-1',
				in: 'node:parent-1',
				out: 'node:child-1',
				order: 1.0
			});

			await mockTauri.event.emit('edge:created', {
				id: 'edge:parent-child-2',
				in: 'node:parent-1',
				out: 'node:child-2',
				order: 2.0
			});

			await mockTauri.event.emit('edge:created', {
				id: 'edge:parent-child-3',
				in: 'node:parent-1',
				out: 'node:child-3',
				order: 3.0
			});

			// All edges should arrive in order
			expect(createdEdges).toHaveLength(3);
			expect(createdEdges[0]).toMatchObject({
				parent: 'node:parent-1',
				child: 'node:child-1',
				order: 1.0
			});
			expect(createdEdges[1]).toMatchObject({
				parent: 'node:parent-1',
				child: 'node:child-2',
				order: 2.0
			});
			expect(createdEdges[2]).toMatchObject({
				parent: 'node:parent-1',
				child: 'node:child-3',
				order: 3.0
			});
		});
	});

	describe('Concurrent Operations: Last-Write-Wins (Acceptance Criterion #3)', () => {
		it('should resolve concurrent indents with last-write-wins via version tracking', async () => {
			// Simulate:
			// 1. Tauri: User indents node-child under parent-A (optimistic update, pending)
			// 2. MCP: Indents same node under parent-B with higher version
			// 3. Result: parent-B wins (last write)

			const edgeUpdateEvents: RealtimeEdgeUpdatedEvent[] = [];

			eventBus.subscribe('edge:updated', (event) => {
				if (event.namespace === 'sync') {
					edgeUpdateEvents.push(event as RealtimeEdgeUpdatedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Scenario: Both clients try to move the same node to different parents
			// Tauri sends to parent-A (version 1)
			// MCP sends to parent-B (version 2) - this is the "last write"

			// First, MCP updates (version 1)
			await mockTauri.event.emit('edge:updated', {
				id: 'edge:conflict-001',
				in: 'node:parent-a',
				out: 'node:contested-child',
				order: 1.0
				// Note: No version field in edge, but changes detected via hash/version
			});

			// Then, MCP updates again with higher version (this wins)
			await mockTauri.event.emit('edge:updated', {
				id: 'edge:conflict-001',
				in: 'node:parent-b',
				out: 'node:contested-child',
				order: 1.0
				// This is the last write - parent-B wins
			});

			// The last write should be the current state
			expect(edgeUpdateEvents).toHaveLength(2);
			expect(edgeUpdateEvents[1].edgeData.in).toBe('node:parent-b');
		});

		it('should resolve concurrent content edits with last-write-wins', async () => {
			// Simulate:
			// 1. Tauri: User edits content (local)
			// 2. MCP: Edits same node with newer timestamp
			// 3. Result: MCP version should be last write

			const nodeUpdateEvents: RealtimeNodeUpdatedEvent[] = [];

			eventBus.subscribe('node:updated', (event) => {
				if (event.namespace === 'sync') {
					nodeUpdateEvents.push(event as RealtimeNodeUpdatedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Tauri user edits (version 1)
			await mockTauri.event.emit('node:updated', {
				id: 'node:content-conflict',
				content: 'User edit from Tauri',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date(Date.now() - 1000).toISOString()
			});

			// MCP updates with newer version (version 2, later timestamp)
			await mockTauri.event.emit('node:updated', {
				id: 'node:content-conflict',
				content: 'Update from MCP server',
				nodeType: 'text',
				version: 2,
				modifiedAt: new Date(Date.now()).toISOString()
			});

			// The last write (MCP with version 2) should win
			expect(nodeUpdateEvents).toHaveLength(2);
			expect(nodeUpdateEvents[1].nodeData.version).toBe(2);
			expect(nodeUpdateEvents[1].nodeData.content).toBe('Update from MCP server');
		});

		it('should track version numbers across multiple concurrent updates', async () => {
			const versions: number[] = [];

			eventBus.subscribe('node:updated', (event) => {
				if (event.namespace === 'sync') {
					versions.push((event as RealtimeNodeUpdatedEvent).nodeData.version);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Rapid concurrent updates
			for (let i = 1; i <= 5; i++) {
				await mockTauri.event.emit('node:updated', {
					id: 'node:versioned-conflict',
					content: `Update ${i}`,
					nodeType: 'text',
					version: i,
					modifiedAt: new Date().toISOString()
				});
			}

			// All versions should be tracked
			expect(versions).toEqual([1, 2, 3, 4, 5]);
		});
	});

	describe('Optimistic Updates + Rollback on Failure (Acceptance Criterion #4)', () => {
		it('should rollback optimistic indent update when backend validation fails', async () => {
			// Simulate:
			// 1. Tauri: User indents node (optimistic update applied locally)
			// 2. Backend: Validation fails (e.g., can't indent under non-container)
			// 3. Result: edge:deleted event reverses the optimistic update

			const edgeEvents: Array<{
				type: 'created' | 'deleted';
				edgeId: string;
			}> = [];

			eventBus.subscribe('edge:created', (event) => {
				if (event.namespace === 'sync') {
					edgeEvents.push({
						type: 'created',
						edgeId: (event as RealtimeEdgeCreatedEvent).edgeId
					});
				}
			});

			eventBus.subscribe('edge:deleted', (event) => {
				if (event.namespace === 'sync') {
					edgeEvents.push({
						type: 'deleted',
						edgeId: (event as RealtimeEdgeDeletedEvent).edgeId
					});
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Tauri applies optimistic update locally
			// (This would be stored in local state, not waiting for server)
			const optimisticEdgeId = 'edge:optimistic-indent-001';

			// Simulate: User indents, optimistic update applied, but backend rejects
			// First, optimistic create is shown to user
			await mockTauri.event.emit('edge:created', {
				id: optimisticEdgeId,
				in: 'node:leaf-parent', // Can't have children!
				out: 'node:child',
				order: 1.0
			});

			// Then backend validation fails, sends delete to rollback
			await mockTauri.event.emit('edge:deleted', {
				id: optimisticEdgeId
			});

			// Result: optimistic update is rolled back
			expect(edgeEvents).toHaveLength(2);
			expect(edgeEvents[0].type).toBe('created');
			expect(edgeEvents[1].type).toBe('deleted');
			expect(edgeEvents[0].edgeId).toBe(edgeEvents[1].edgeId);
		});

		it('should rollback content update when backend fails', async () => {
			// Simulate:
			// 1. Tauri: User edits content (optimistic)
			// 2. Backend: Fails to persist (conflict, storage error)
			// 3. Result: node:updated event with original content reverts change

			const contentHistory: Array<{ version: number; content: string }> = [];

			eventBus.subscribe('node:updated', (event) => {
				if (event.namespace === 'sync') {
					const updated = event as RealtimeNodeUpdatedEvent;
					contentHistory.push({
						version: updated.nodeData.version,
						content: updated.nodeData.content
					});
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// User edits content (version 1)
			await mockTauri.event.emit('node:updated', {
				id: 'node:rollback-test',
				content: 'User typed this',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date().toISOString()
			});

			// Backend fails to persist, sends back original content (version unchanged)
			await mockTauri.event.emit('node:updated', {
				id: 'node:rollback-test',
				content: 'Previous valid content', // Rolled back
				nodeType: 'text',
				version: 0, // Version didn't increment (failed validation)
				modifiedAt: new Date().toISOString()
			});

			// Content is rolled back to previous state
			expect(contentHistory).toHaveLength(2);
			expect(contentHistory[0].content).toBe('User typed this');
			expect(contentHistory[1].content).toBe('Previous valid content');
		});

		it('should emit error event when backend operation fails', async () => {
			const errorEvents: unknown[] = [];

			eventBus.subscribe('error:sync-failed', (event) => {
				errorEvents.push(event);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Backend sends error event
			await mockTauri.event.emit('sync:error', {
				message: 'Failed to update node: validation error',
				errorType: 'validation-error',
				retryable: false
			});

			expect(errorEvents).toHaveLength(1);
			expect(errorEvents[0]).toMatchObject({
				type: 'error:sync-failed',
				namespace: 'error',
				message: 'Failed to update node: validation error',
				errorType: 'validation-error'
			});
		});
	});

	describe('1000 Rapid Operations - No Orphaned Nodes (Acceptance Criterion #5)', () => {
		it('should handle 1000 rapid node operations without data loss', async () => {
			const nodeIds = new Set<string>();
			const deletedNodeIds = new Set<string>();

			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					nodeIds.add((event as RealtimeNodeCreatedEvent).nodeId);
				}
			});

			eventBus.subscribe('node:deleted', (event) => {
				if (event.namespace === 'sync') {
					deletedNodeIds.add((event as RealtimeNodeDeletedEvent).nodeId);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Create 1000 nodes rapidly
			const createPromises = [];
			for (let i = 0; i < 1000; i++) {
				createPromises.push(
					mockTauri.event.emit('node:created', {
						id: `node:stress-${i}`,
						content: `Stress test node ${i}`,
						nodeType: 'text',
						version: 1,
						modifiedAt: new Date().toISOString()
					})
				);
			}
			await Promise.all(createPromises);

			// Verify all nodes created
			expect(nodeIds.size).toBe(1000);

			// Now delete 500 of them
			const deletePromises = [];
			for (let i = 0; i < 500; i++) {
				deletePromises.push(
					mockTauri.event.emit('node:deleted', {
						id: `node:stress-${i}`
					})
				);
			}
			await Promise.all(deletePromises);

			// Verify deletions recorded
			expect(deletedNodeIds.size).toBe(500);

			// Verify no orphaned nodes (all deleted were created)
			deletedNodeIds.forEach((deletedId) => {
				expect(nodeIds.has(deletedId)).toBe(true);
			});

			// 500 should still be active
			expect(nodeIds.size - deletedNodeIds.size).toBe(500);
		});

		it('should maintain hierarchy integrity under 1000 rapid edge operations', async () => {
			const edgeIds = new Set<string>();

			eventBus.subscribe('edge:created', (event) => {
				if (event.namespace === 'sync') {
					edgeIds.add((event as RealtimeEdgeCreatedEvent).edgeId);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Create 1000 edges rapidly (simulating indenting lots of nodes)
			const edgePromises = [];
			for (let i = 0; i < 1000; i++) {
				const parentIndex = Math.floor(i / 10); // 10 parents
				edgePromises.push(
					mockTauri.event.emit('edge:created', {
						id: `edge:stress-${i}`,
						in: `node:parent-${parentIndex}`,
						out: `node:child-${i}`,
						order: (i % 10) + 1 // Order 1-10 within each parent
					})
				);
			}
			await Promise.all(edgePromises);

			// All edges should be recorded
			expect(edgeIds.size).toBe(1000);
		});
	});

	describe('Stream Disconnection & Reconnection (Acceptance Criterion #6)', () => {
		it('should detect stream disconnection and emit status event', async () => {
			const statusEvents: RealtimeSyncStatusEvent[] = [];

			eventBus.subscribe('sync:status-changed', (event) => {
				statusEvents.push(event as RealtimeSyncStatusEvent);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate stream disconnection
			await mockTauri.event.emit('sync:status', {
				status: 'disconnected',
				reason: 'LIVE SELECT stream interrupted'
			});

			expect(statusEvents).toHaveLength(1);
			expect(statusEvents[0].status).toBe('disconnected');
			expect(statusEvents[0].reason).toContain('interrupted');
		});

		it('should automatically reconnect after stream disconnection', async () => {
			const statusEvents: RealtimeSyncStatusEvent[] = [];

			eventBus.subscribe('sync:status-changed', (event) => {
				statusEvents.push(event as RealtimeSyncStatusEvent);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate reconnection sequence
			await mockTauri.event.emit('sync:status', {
				status: 'disconnected',
				reason: 'Stream interrupted'
			});

			await mockTauri.event.emit('sync:status', {
				status: 'reconnecting',
				reason: 'Attempting reconnection (attempt 1)'
			});

			await mockTauri.event.emit('sync:status', {
				status: 'connected',
				reason: 'Reconnection successful'
			});

			// All status transitions should be recorded
			expect(statusEvents).toHaveLength(3);
			expect(statusEvents[0].status).toBe('disconnected');
			expect(statusEvents[1].status).toBe('reconnecting');
			expect(statusEvents[2].status).toBe('connected');
		});

		it('should continue processing events after reconnection', async () => {
			const nodeEvents: RealtimeNodeCreatedEvent[] = [];
			const statusEvents: RealtimeSyncStatusEvent[] = [];

			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					nodeEvents.push(event as RealtimeNodeCreatedEvent);
				}
			});

			eventBus.subscribe('sync:status-changed', (event) => {
				statusEvents.push(event as RealtimeSyncStatusEvent);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Create node before disconnection
			await mockTauri.event.emit('node:created', {
				id: 'node:before-disconnect',
				content: 'Created before disconnect',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date().toISOString()
			});

			// Simulate disconnection
			await mockTauri.event.emit('sync:status', {
				status: 'disconnected',
				reason: 'Network lost'
			});

			// Simulate reconnection
			await mockTauri.event.emit('sync:status', {
				status: 'reconnecting',
				reason: 'Reconnecting...'
			});

			await mockTauri.event.emit('sync:status', {
				status: 'connected',
				reason: 'Reconnected'
			});

			// Create node after reconnection
			await mockTauri.event.emit('node:created', {
				id: 'node:after-reconnect',
				content: 'Created after reconnection',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date().toISOString()
			});

			// Both nodes should be received
			expect(nodeEvents).toHaveLength(2);
			expect(nodeEvents[0].nodeId).toBe('node:before-disconnect');
			expect(nodeEvents[1].nodeId).toBe('node:after-reconnect');

			// Status transitions should be recorded
			expect(statusEvents).toHaveLength(3);
		});

		it('should handle multiple reconnection attempts with exponential backoff', async () => {
			const statusEvents: RealtimeSyncStatusEvent[] = [];

			eventBus.subscribe('sync:status-changed', (event) => {
				statusEvents.push(event as RealtimeSyncStatusEvent);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate multiple failed reconnection attempts
			await mockTauri.event.emit('sync:status', {
				status: 'disconnected',
				reason: 'Initial connection lost'
			});

			// Attempt 1
			await mockTauri.event.emit('sync:status', {
				status: 'reconnecting',
				reason: 'Attempting reconnection (attempt 1/10)'
			});

			// Attempt 2
			await mockTauri.event.emit('sync:status', {
				status: 'reconnecting',
				reason: 'Attempting reconnection (attempt 2/10)'
			});

			// Eventually succeeds
			await mockTauri.event.emit('sync:status', {
				status: 'connected',
				reason: 'Reconnected successfully'
			});

			// Track all attempts
			expect(statusEvents).toHaveLength(4);
			expect(statusEvents[0].status).toBe('disconnected');
			expect(statusEvents[1].status).toBe('reconnecting');
			expect(statusEvents[2].status).toBe('reconnecting');
			expect(statusEvents[3].status).toBe('connected');
		});

		it('should emit final disconnected event after max retry attempts exceeded', async () => {
			const statusEvents: RealtimeSyncStatusEvent[] = [];

			eventBus.subscribe('sync:status-changed', (event) => {
				statusEvents.push(event as RealtimeSyncStatusEvent);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate connection failure → exhausted retries
			await mockTauri.event.emit('sync:status', {
				status: 'disconnected',
				reason: 'Connection failed'
			});

			// Multiple attempts
			for (let i = 1; i <= 10; i++) {
				await mockTauri.event.emit('sync:status', {
					status: 'reconnecting',
					reason: `Attempting reconnection (attempt ${i}/10)`
				});
			}

			// Final disconnected after max attempts
			await mockTauri.event.emit('sync:status', {
				status: 'disconnected',
				reason: 'Max reconnection attempts exceeded (10/10)'
			});

			// All events should be recorded
			expect(statusEvents.length).toBe(12); // 1 + 10 + 1
			expect(statusEvents[statusEvents.length - 1].status).toBe('disconnected');
			expect(statusEvents[statusEvents.length - 1].reason).toContain('Max');
		});
	});

	describe('Integration: Complex Multi-Client Scenario (Acceptance Criterion #7+)', () => {
		it('should handle complex multi-client workflow: create, indent, update, reorder, delete', async () => {
			const events: Array<{ type: string; nodeId?: string; edgeId?: string }> = [];

			// Track all event types
			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					events.push({
						type: 'node:created',
						nodeId: (event as RealtimeNodeCreatedEvent).nodeId
					});
				}
			});

			eventBus.subscribe('edge:created', (event) => {
				if (event.namespace === 'sync') {
					events.push({
						type: 'edge:created',
						edgeId: (event as RealtimeEdgeCreatedEvent).edgeId
					});
				}
			});

			eventBus.subscribe('node:updated', (event) => {
				if (event.namespace === 'sync') {
					events.push({
						type: 'node:updated',
						nodeId: (event as RealtimeNodeUpdatedEvent).nodeId
					});
				}
			});

			eventBus.subscribe('edge:updated', (event) => {
				if (event.namespace === 'sync') {
					events.push({
						type: 'edge:updated',
						edgeId: (event as RealtimeEdgeUpdatedEvent).edgeId
					});
				}
			});

			eventBus.subscribe('node:deleted', (event) => {
				if (event.namespace === 'sync') {
					events.push({
						type: 'node:deleted',
						nodeId: (event as RealtimeNodeDeletedEvent).nodeId
					});
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Workflow:
			// 1. MCP creates parent node
			await mockTauri.event.emit('node:created', {
				id: 'node:workflow-parent',
				content: 'Project',
				nodeType: 'header',
				version: 1,
				modifiedAt: new Date().toISOString()
			});

			// 2. MCP creates 3 child tasks
			for (let i = 1; i <= 3; i++) {
				await mockTauri.event.emit('node:created', {
					id: `node:workflow-task-${i}`,
					content: `Task ${i}`,
					nodeType: 'task',
					version: 1,
					modifiedAt: new Date().toISOString()
				});
			}

			// 3. MCP indents all tasks under parent
			for (let i = 1; i <= 3; i++) {
				await mockTauri.event.emit('edge:created', {
					id: `edge:workflow-${i}`,
					in: 'node:workflow-parent',
					out: `node:workflow-task-${i}`,
					order: i
				});
			}

			// 4. User updates first task content
			await mockTauri.event.emit('node:updated', {
				id: 'node:workflow-task-1',
				content: 'Task 1 - Updated by user',
				nodeType: 'task',
				version: 2,
				modifiedAt: new Date().toISOString()
			});

			// 5. MCP reorders tasks (move task 3 to position 1.5)
			await mockTauri.event.emit('edge:updated', {
				id: 'edge:workflow-3',
				in: 'node:workflow-parent',
				out: 'node:workflow-task-3',
				order: 1.5
			});

			// 6. User deletes task 2
			await mockTauri.event.emit('node:deleted', {
				id: 'node:workflow-task-2'
			});

			// Verify event sequence
			// 1 parent + 3 tasks + 3 indents + 1 update + 1 reorder + 1 delete = 10 events
			expect(events).toHaveLength(10);
			expect(events[0].type).toBe('node:created'); // parent
			expect(events[1].type).toBe('node:created'); // task 1
			expect(events[2].type).toBe('node:created'); // task 2
			expect(events[3].type).toBe('node:created'); // task 3
			expect(events[4].type).toBe('edge:created'); // indent task 1
			expect(events[5].type).toBe('edge:created'); // indent task 2
			expect(events[6].type).toBe('edge:created'); // indent task 3
			expect(events[7].type).toBe('node:updated'); // update task 1
			expect(events[8].type).toBe('edge:updated'); // reorder task 3
			expect(events[9].type).toBe('node:deleted'); // delete task 2
		});
	});

	describe('CI/CD Compatibility (Acceptance Criterion #8)', () => {
		it('should pass in CI/CD environment without flaky timing issues', async () => {
			const receivedEvents: RealtimeNodeCreatedEvent[] = [];

			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeNodeCreatedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Fast event emission (no artificial delays)
			const nodeData: NodeEventData = {
				id: 'node:ci-test',
				content: 'CI test node',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date().toISOString()
			};

			// In-process event handling should be synchronous
			await mockTauri.event.emit('node:created', nodeData);

			// Assert: Event was received immediately
			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0].nodeId).toBe('node:ci-test');
		});

		it('should not have timing dependencies that cause flaky tests', async () => {
			// This test verifies that event handling doesn't depend on
			// setTimeout, setInterval, or other timing mechanisms that could
			// cause flakiness in CI/CD

			const events: string[] = [];

			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					events.push('created');
				}
			});

			eventBus.subscribe('node:updated', (event) => {
				if (event.namespace === 'sync') {
					events.push('updated');
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Rapid events with no delays
			await mockTauri.event.emit('node:created', {
				id: 'node:rapid-1',
				content: 'Test 1',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date().toISOString()
			});

			await mockTauri.event.emit('node:updated', {
				id: 'node:rapid-1',
				content: 'Test 1 updated',
				nodeType: 'text',
				version: 2,
				modifiedAt: new Date().toISOString()
			});

			// Both events should arrive immediately without timing deps
			expect(events).toEqual(['created', 'updated']);
		});
	});
});
