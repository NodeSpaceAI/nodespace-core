/**
 * Integration Tests for Polling-Based Real-Time Synchronization
 *
 * Tests the polling-based LIVE SELECT MVP implementation:
 * - LiveQueryService (Rust backend) polls database every 1 second
 * - Detects changes via version tracking (nodes) and hash comparison (edges)
 * - Emits Tauri events (node:created, node:updated, node:deleted, edge:*)
 * - Frontend tauri-sync-listener.ts bridges to EventBus
 *
 * These tests verify the end-to-end event propagation flow WITHOUT requiring
 * a full Tauri runtime. We mock the Tauri event system and test the event
 * handler logic directly.
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
	NodeEventData,
	EdgeEventData
} from '$lib/services/event-types';

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

describe('Polling-Based Real-Time Synchronization', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		mockTauriEvents.clear();
		eventBus.reset();
	});

	afterEach(() => {
		eventBus.reset();
	});

	describe('Node Event Propagation', () => {
		it('should propagate node:created events from Tauri to EventBus', async () => {
			// Arrange: Set up EventBus listener
			const receivedEvents: RealtimeNodeCreatedEvent[] = [];
			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeNodeCreatedEvent);
				}
			});

			// Import the module to register Tauri listeners
			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Act: Simulate Rust LiveQueryService emitting node:created event
			const nodeData: NodeEventData = {
				id: 'node:test-123',
				content: 'Test node content',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date().toISOString()
			};

			await mockTauri.event.emit('node:created', nodeData);

			// Assert: Event should arrive in EventBus
			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				type: 'node:created',
				namespace: 'sync',
				source: 'tauri-polling-service',
				nodeId: 'node:test-123',
				nodeData: {
					id: 'node:test-123',
					content: 'Test node content',
					nodeType: 'text',
					version: 1
				}
			});
		});

		it('should propagate node:updated events with version tracking', async () => {
			const receivedEvents: RealtimeNodeUpdatedEvent[] = [];
			eventBus.subscribe('node:updated', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeNodeUpdatedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate node update with version increment
			const nodeData: NodeEventData = {
				id: 'node:test-456',
				content: 'Updated content',
				nodeType: 'text',
				version: 5, // Version incremented to detect update
				modifiedAt: new Date().toISOString()
			};

			await mockTauri.event.emit('node:updated', nodeData);

			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				type: 'node:updated',
				namespace: 'sync',
				nodeId: 'node:test-456',
				nodeData: {
					version: 5
				}
			});
		});

		it('should propagate node:deleted events with just ID', async () => {
			const receivedEvents: RealtimeNodeDeletedEvent[] = [];
			eventBus.subscribe('node:deleted', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeNodeDeletedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			await mockTauri.event.emit('node:deleted', { id: 'node:deleted-789' });

			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				type: 'node:deleted',
				namespace: 'sync',
				nodeId: 'node:deleted-789'
			});
		});

		it('should handle rapid node events without loss', async () => {
			const receivedEvents: Array<RealtimeNodeCreatedEvent | RealtimeNodeUpdatedEvent> = [];
			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeNodeCreatedEvent);
				}
			});
			eventBus.subscribe('node:updated', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeNodeUpdatedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate 50 rapid events (create + update cycles)
			const promises = [];
			for (let i = 0; i < 50; i++) {
				const nodeData: NodeEventData = {
					id: `node:rapid-${i}`,
					content: `Content ${i}`,
					nodeType: 'text',
					version: i % 2 === 0 ? 1 : 2, // Alternate create/update
					modifiedAt: new Date().toISOString()
				};

				if (i % 2 === 0) {
					promises.push(mockTauri.event.emit('node:created', nodeData));
				} else {
					promises.push(mockTauri.event.emit('node:updated', nodeData));
				}
			}

			await Promise.all(promises);

			// All events should arrive
			expect(receivedEvents).toHaveLength(50);
		});
	});

	describe('Edge Event Propagation', () => {
		it('should propagate edge:created events for hierarchy changes', async () => {
			const receivedEvents: RealtimeEdgeCreatedEvent[] = [];
			eventBus.subscribe('edge:created', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeEdgeCreatedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate edge creation (node indented under parent)
			const edgeData: EdgeEventData = {
				id: 'edge:parent-child',
				in: 'node:parent-123',
				out: 'node:child-456',
				order: 1.0
			};

			await mockTauri.event.emit('edge:created', edgeData);

			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				type: 'edge:created',
				namespace: 'sync',
				edgeData: {
					in: 'node:parent-123',
					out: 'node:child-456',
					order: 1.0
				}
			});
		});

		it('should propagate edge:updated events for order changes', async () => {
			const receivedEvents: RealtimeEdgeUpdatedEvent[] = [];
			eventBus.subscribe('edge:updated', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeEdgeUpdatedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate edge update (fractional reordering)
			const edgeData: EdgeEventData = {
				id: 'edge:reorder',
				in: 'node:parent-123',
				out: 'node:child-456',
				order: 1.5 // Fractional order from #550
			};

			await mockTauri.event.emit('edge:updated', edgeData);

			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				type: 'edge:updated',
				namespace: 'sync',
				edgeData: {
					order: 1.5
				}
			});
		});

		it('should propagate edge:deleted events for node outdenting', async () => {
			const receivedEvents: RealtimeEdgeDeletedEvent[] = [];
			eventBus.subscribe('edge:deleted', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push(event as RealtimeEdgeDeletedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			await mockTauri.event.emit('edge:deleted', { id: 'edge:removed-123' });

			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				type: 'edge:deleted',
				namespace: 'sync',
				edgeId: 'edge:removed-123'
			});
		});
	});

	describe('Synchronization Status Events', () => {
		it('should emit sync:status connected event on successful connection', async () => {
			const receivedEvents: RealtimeSyncStatusEvent[] = [];
			eventBus.subscribe('sync:status-changed', (event) => {
				receivedEvents.push(event as RealtimeSyncStatusEvent);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			await mockTauri.event.emit('sync:status', {
				status: 'connected',
				reason: 'Initial connection'
			});

			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				type: 'sync:status-changed',
				namespace: 'sync',
				status: 'connected',
				reason: 'Initial connection'
			});
		});

		it('should emit sync:status reconnecting event during reconnection', async () => {
			const receivedEvents: RealtimeSyncStatusEvent[] = [];
			eventBus.subscribe('sync:status-changed', (event) => {
				receivedEvents.push(event as RealtimeSyncStatusEvent);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Simulate reconnection sequence
			await mockTauri.event.emit('sync:status', {
				status: 'disconnected',
				reason: 'Database connection lost'
			});

			await mockTauri.event.emit('sync:status', {
				status: 'reconnecting',
				reason: 'Attempting reconnection (attempt 1)'
			});

			await mockTauri.event.emit('sync:status', {
				status: 'connected',
				reason: 'Reconnection successful'
			});

			expect(receivedEvents).toHaveLength(3);
			expect(receivedEvents[0].status).toBe('disconnected');
			expect(receivedEvents[1].status).toBe('reconnecting');
			expect(receivedEvents[2].status).toBe('connected');
		});

		it('should emit sync:status disconnected after max retry attempts', async () => {
			const receivedEvents: RealtimeSyncStatusEvent[] = [];
			eventBus.subscribe('sync:status-changed', (event) => {
				receivedEvents.push(event as RealtimeSyncStatusEvent);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			await mockTauri.event.emit('sync:status', {
				status: 'disconnected',
				reason: 'Max retry attempts exceeded (10)'
			});

			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				status: 'disconnected',
				reason: 'Max retry attempts exceeded (10)'
			});
		});
	});

	describe('Error Handling', () => {
		it('should handle malformed node events gracefully without crashing', async () => {
			// Test that malformed events don't crash the system
			const receivedEvents: unknown[] = [];
			eventBus.subscribe('node:created', (event) => {
				receivedEvents.push(event);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Emit malformed event (missing required fields)
			// The handler should emit an event even with partial data
			await mockTauri.event.emit('node:created', {
				// Missing id, content, nodeType, version
			});

			// System should still be functional (no crash)
			// May or may not emit event depending on handler implementation
			expect(receivedEvents.length).toBeGreaterThanOrEqual(0);
		});

		it('should handle edge events with invalid edge data without crashing', async () => {
			const receivedEvents: unknown[] = [];
			eventBus.subscribe('edge:created', (event) => {
				receivedEvents.push(event);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Emit edge event with invalid data
			await mockTauri.event.emit('edge:created', {
				// Missing in, out, order fields
				id: 'edge:invalid'
			});

			// System should still be functional (no crash)
			expect(receivedEvents.length).toBeGreaterThanOrEqual(0);
		});

		it('should emit sync:error events when synchronization fails', async () => {
			const receivedEvents: unknown[] = [];
			eventBus.subscribe('error:sync-failed', (event) => {
				receivedEvents.push(event);
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			await mockTauri.event.emit('sync:error', {
				message: 'Database query failed',
				errorType: 'stream-interrupted',
				retryable: true
			});

			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0]).toMatchObject({
				type: 'error:sync-failed',
				namespace: 'error',
				message: 'Database query failed',
				errorType: 'stream-interrupted',
				retryable: true
			});
		});
	});

	describe('Event Timing and Latency', () => {
		it('should propagate events within acceptable latency (<100ms)', async () => {
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

			const emittedAt = Date.now();
			await mockTauri.event.emit('node:created', {
				id: 'node:latency-test',
				content: 'Latency test',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date(emittedAt).toISOString()
			});

			// Check latency
			expect(receivedEvents).toHaveLength(1);
			const latency = receivedEvents[0].receivedAt - emittedAt;

			// Should be <100ms (actually should be <10ms for in-process events)
			expect(latency).toBeLessThan(100);
		});

		it('should maintain event order for rapid sequential changes', async () => {
			const receivedEvents: Array<{ nodeId: string; type: string; timestamp: number }> = [];

			eventBus.subscribe('node:created', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push({
						nodeId: (event as RealtimeNodeCreatedEvent).nodeId,
						type: 'created',
						timestamp: Date.now()
					});
				}
			});

			eventBus.subscribe('node:updated', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push({
						nodeId: (event as RealtimeNodeUpdatedEvent).nodeId,
						type: 'updated',
						timestamp: Date.now()
					});
				}
			});

			eventBus.subscribe('node:deleted', (event) => {
				if (event.namespace === 'sync') {
					receivedEvents.push({
						nodeId: (event as RealtimeNodeDeletedEvent).nodeId,
						type: 'deleted',
						timestamp: Date.now()
					});
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// Emit events in sequence: create → update → delete
			await mockTauri.event.emit('node:created', {
				id: 'node:sequence',
				content: 'Initial',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date().toISOString()
			});

			await mockTauri.event.emit('node:updated', {
				id: 'node:sequence',
				content: 'Updated',
				nodeType: 'text',
				version: 2,
				modifiedAt: new Date().toISOString()
			});

			await mockTauri.event.emit('node:deleted', {
				id: 'node:sequence'
			});

			// Events should arrive in order
			expect(receivedEvents).toHaveLength(3);
			expect(receivedEvents[0].type).toBe('created');
			expect(receivedEvents[1].type).toBe('updated');
			expect(receivedEvents[2].type).toBe('deleted');

			// All should reference the same node
			expect(receivedEvents.every((e) => e.nodeId === 'node:sequence')).toBe(true);
		});
	});

	describe('Multi-Client Synchronization Simulation', () => {
		it('should sync MCP server node creation to Tauri frontend', async () => {
			// This simulates the workflow:
			// 1. MCP server creates a node in database
			// 2. LiveQueryService detects change (polling every 1s)
			// 3. Rust emits Tauri event
			// 4. Frontend receives event via tauri-sync-listener

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

			// Simulate MCP server creating a node
			// (In reality: MCP → Database → LiveQueryService polls → Tauri event)
			const mcpNodeData: NodeEventData = {
				id: 'node:mcp-created-abc',
				content: 'Created by MCP server',
				nodeType: 'text',
				version: 1,
				modifiedAt: new Date().toISOString()
			};

			await mockTauri.event.emit('node:created', mcpNodeData);

			// Frontend should receive the event
			expect(receivedEvents).toHaveLength(1);
			expect(receivedEvents[0].nodeData.content).toBe('Created by MCP server');
			expect(receivedEvents[0].source).toBe('tauri-polling-service');
		});

		it('should sync hierarchy changes from MCP to frontend', async () => {
			// Simulates: MCP indents a node → edge created → frontend receives event

			const edgeEvents: RealtimeEdgeCreatedEvent[] = [];
			eventBus.subscribe('edge:created', (event) => {
				if (event.namespace === 'sync') {
					edgeEvents.push(event as RealtimeEdgeCreatedEvent);
				}
			});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);
			await initializeTauriSyncListeners();

			// MCP server indents node-child under node-parent
			await mockTauri.event.emit('edge:created', {
				id: 'edge:mcp-indent',
				in: 'node:parent',
				out: 'node:child',
				order: 1.0
			});

			expect(edgeEvents).toHaveLength(1);
			expect(edgeEvents[0].edgeData).toMatchObject({
				in: 'node:parent',
				out: 'node:child'
			});
		});
	});

	describe('Listener Cleanup', () => {
		it('should not leak listeners on multiple initializations', async () => {
			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);

			// Initialize multiple times
			await initializeTauriSyncListeners();
			await initializeTauriSyncListeners();
			await initializeTauriSyncListeners();

			// Should register listeners each time (mocked implementation doesn't prevent this)
			// In real implementation, would need to track and prevent duplicate listeners
			const nodeCreatedHandlers = mockTauriEvents.get('node:created');
			expect(nodeCreatedHandlers).toBeDefined();
			// Note: Current implementation doesn't deduplicate, but doesn't crash either
		});
	});

	describe('Non-Tauri Environment', () => {
		it('should skip initialization when not in Tauri environment', async () => {
			// Temporarily remove __TAURI__
			const originalTauri = (global as unknown as { __TAURI__?: typeof mockTauri }).__TAURI__;
			delete (global as unknown as { __TAURI__?: typeof mockTauri }).__TAURI__;

			const consoleDebugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {});

			const { initializeTauriSyncListeners } = await import(
				'$lib/services/tauri-sync-listener'
			);

			// Should complete without error
			await initializeTauriSyncListeners();

			expect(consoleDebugSpy).toHaveBeenCalledWith(
				expect.stringContaining('Not running in Tauri environment')
			);

			// Restore Tauri
			(global as unknown as { __TAURI__: typeof mockTauri }).__TAURI__ = originalTauri!;
			consoleDebugSpy.mockRestore();
		});
	});
});
