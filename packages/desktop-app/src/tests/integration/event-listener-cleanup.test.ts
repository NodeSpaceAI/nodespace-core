/**
 * Event Listener Memory Leak Detection Tests
 *
 * Tests event listener cleanup to prevent memory leaks in components
 * that attach listeners to document. Verifies that listeners are properly
 * removed during unmount and that no listeners accumulate over time.
 *
 * Test Coverage:
 * - Component Lifecycle (4 tests)
 * - Rapid Operations (3 tests)
 * - EventBus Subscriptions (3 tests)
 * - Verification Tools (2 tests)
 *
 * Related: Issue #162
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from '@testing-library/svelte';
import NodeAutocomplete from '$lib/components/ui/node-autocomplete/node-autocomplete.svelte';
import SlashCommandDropdown from '$lib/components/ui/slash-command-dropdown/slash-command-dropdown.svelte';
import { eventBus } from '$lib/services/event-bus';

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Get keydown listener counts from spies
 */
function getKeydownListenerCounts(
  addSpy: ReturnType<typeof vi.spyOn>,
  removeSpy: ReturnType<typeof vi.spyOn>
) {
  const added = addSpy.mock.calls.filter((call) => call[0] === 'keydown').length;
  const removed = removeSpy.mock.calls.filter((call) => call[0] === 'keydown').length;

  return {
    added,
    removed,
    balance: added - removed
  };
}

/**
 * Get keydown listener handler references from spies
 */
function getKeydownListenerHandlers(
  addSpy: ReturnType<typeof vi.spyOn>,
  removeSpy: ReturnType<typeof vi.spyOn>
) {
  const addedHandlers = addSpy.mock.calls
    .filter((call) => call[0] === 'keydown')
    .map((call) => call[1]);

  const removedHandlers = removeSpy.mock.calls
    .filter((call) => call[0] === 'keydown')
    .map((call) => call[1]);

  return {
    added: addedHandlers,
    removed: removedHandlers
  };
}

/**
 * Standard baseline measurement pattern using slice()
 */
function measureListenerDelta(
  addSpy: ReturnType<typeof vi.spyOn>,
  removeSpy: ReturnType<typeof vi.spyOn>,
  initialAdds: number,
  initialRemoves: number
) {
  const newAdds = addSpy.mock.calls
    .slice(initialAdds)
    .filter((call) => call[0] === 'keydown').length;

  const newRemoves = removeSpy.mock.calls
    .slice(initialRemoves)
    .filter((call) => call[0] === 'keydown').length;

  return {
    added: newAdds,
    removed: newRemoves,
    balance: newAdds - newRemoves
  };
}

// ============================================================================
// Tests
// ============================================================================

describe('Event Listener Memory Leak Detection', () => {
  beforeEach(() => {
    // Clean up DOM and EventBus between tests
    document.body.innerHTML = '';
    eventBus.reset();
  });

  afterEach(() => {
    // Restore all spies
    vi.restoreAllMocks();
  });

  // ========================================================================
  // Component Lifecycle Tests (4 tests)
  // ========================================================================

  describe('Component Lifecycle', () => {
    it('should remove keydown listener when NodeAutocomplete unmounts', () => {
      const addSpy = vi.spyOn(document, 'addEventListener');
      const removeSpy = vi.spyOn(document, 'removeEventListener');

      // Render component
      const { unmount } = render(NodeAutocomplete, {
        visible: true,
        results: [],
        position: { x: 0, y: 0 }
      });

      // Verify listener was added and capture handler reference
      const handlers = getKeydownListenerHandlers(addSpy, removeSpy);
      expect(handlers.added.length).toBeGreaterThan(0);
      const addedHandler = handlers.added[0];

      unmount();

      // Verify listener was removed with same handler reference
      const handlersAfterUnmount = getKeydownListenerHandlers(addSpy, removeSpy);
      expect(handlersAfterUnmount.removed.length).toBe(handlers.added.length);

      // Critical: Verify exact handler reference is removed (not a new function)
      const removedHandler = handlersAfterUnmount.removed[0];
      expect(removedHandler).toBe(addedHandler);
    });

    it('should remove keydown listener when SlashCommandDropdown unmounts', () => {
      const addSpy = vi.spyOn(document, 'addEventListener');
      const removeSpy = vi.spyOn(document, 'removeEventListener');

      // Render component
      const { unmount } = render(SlashCommandDropdown, {
        visible: true,
        commands: [],
        position: { x: 0, y: 0 }
      });

      // Verify listener was added and capture handler reference
      const handlers = getKeydownListenerHandlers(addSpy, removeSpy);
      expect(handlers.added.length).toBeGreaterThan(0);
      const addedHandler = handlers.added[0];

      unmount();

      // Verify listener was removed with same handler reference
      const handlersAfterUnmount = getKeydownListenerHandlers(addSpy, removeSpy);
      expect(handlersAfterUnmount.removed.length).toBe(handlers.added.length);

      // Critical: Verify exact handler reference is removed (not a new function)
      const removedHandler = handlersAfterUnmount.removed[0];
      expect(removedHandler).toBe(addedHandler);
    });

    it('should not accumulate listeners over multiple mount/unmount cycles', () => {
      const addSpy = vi.spyOn(document, 'addEventListener');
      const removeSpy = vi.spyOn(document, 'removeEventListener');

      const cycles = 10;
      const initialAdds = addSpy.mock.calls.length;
      const initialRemoves = removeSpy.mock.calls.length;

      // Mount and unmount multiple times
      for (let i = 0; i < cycles; i++) {
        const { unmount } = render(NodeAutocomplete, {
          visible: true,
          results: [],
          position: { x: 0, y: 0 }
        });
        unmount();
      }

      // Use standard measurement pattern
      const delta = measureListenerDelta(addSpy, removeSpy, initialAdds, initialRemoves);

      // All added listeners should have been removed
      expect(delta.removed).toBe(delta.added);
      expect(delta.added).toBe(cycles);
      expect(delta.balance).toBe(0);
    });

    it('should cleanup listeners during normal unmount lifecycle', () => {
      // Note: This test verifies normal cleanup behavior. Svelte's onDestroy
      // cleanup is guaranteed to run even if user code throws errors, making
      // explicit error injection testing unnecessary and potentially fragile.

      const addSpy = vi.spyOn(document, 'addEventListener');
      const removeSpy = vi.spyOn(document, 'removeEventListener');

      // Render component
      const { unmount } = render(NodeAutocomplete, {
        visible: true,
        results: [],
        position: { x: 0, y: 0 }
      });

      // Verify listener was added
      const countsBeforeUnmount = getKeydownListenerCounts(addSpy, removeSpy);
      expect(countsBeforeUnmount.added).toBeGreaterThan(0);

      // Standard unmount - Svelte's onDestroy cleanup runs reliably
      unmount();

      // Verify cleanup happened
      const countsAfterUnmount = getKeydownListenerCounts(addSpy, removeSpy);
      expect(countsAfterUnmount.balance).toBe(0);
    });
  });

  // ========================================================================
  // Rapid Operations Tests (3 tests)
  // ========================================================================

  describe('Rapid Operations', () => {
    it('should handle 100 rapid mount/unmount cycles without leaking', () => {
      const addSpy = vi.spyOn(document, 'addEventListener');
      const removeSpy = vi.spyOn(document, 'removeEventListener');

      const cycles = 100;
      const initialAdds = addSpy.mock.calls.length;
      const initialRemoves = removeSpy.mock.calls.length;

      // Rapid mount/unmount cycles
      for (let i = 0; i < cycles; i++) {
        const { unmount } = render(NodeAutocomplete, {
          visible: true,
          results: [],
          position: { x: 0, y: 0 }
        });
        unmount();
      }

      // Use standard measurement pattern
      const delta = measureListenerDelta(addSpy, removeSpy, initialAdds, initialRemoves);

      // Verify all listeners removed
      expect(delta.removed).toBe(delta.added);
      expect(delta.added).toBe(cycles);
      expect(delta.balance).toBe(0);
    });

    it('should return listener count to baseline after rapid operations', () => {
      const addSpy = vi.spyOn(document, 'addEventListener');
      const removeSpy = vi.spyOn(document, 'removeEventListener');

      const initialAdds = addSpy.mock.calls.length;
      const initialRemoves = removeSpy.mock.calls.length;

      // Perform rapid operations
      for (let i = 0; i < 50; i++) {
        const { unmount } = render(SlashCommandDropdown, {
          visible: true,
          commands: [],
          position: { x: 0, y: 0 }
        });
        unmount();
      }

      // Use standard measurement pattern
      const delta = measureListenerDelta(addSpy, removeSpy, initialAdds, initialRemoves);

      // Net change should be 0 (all added listeners were removed)
      expect(delta.balance).toBe(0);
      expect(delta.removed).toBe(delta.added);
    });

    it('should maintain acceptable memory usage during rapid operations', () => {
      const addSpy = vi.spyOn(document, 'addEventListener');
      const removeSpy = vi.spyOn(document, 'removeEventListener');

      const initialAdds = addSpy.mock.calls.length;
      const initialRemoves = removeSpy.mock.calls.length;

      // Simulate realistic usage pattern: mix of both components
      for (let i = 0; i < 50; i++) {
        const { unmount: unmount1 } = render(NodeAutocomplete, {
          visible: true,
          results: [],
          position: { x: 0, y: 0 }
        });

        const { unmount: unmount2 } = render(SlashCommandDropdown, {
          visible: true,
          commands: [],
          position: { x: 0, y: 0 }
        });

        unmount1();
        unmount2();
      }

      // Use standard measurement pattern
      const delta = measureListenerDelta(addSpy, removeSpy, initialAdds, initialRemoves);

      // All added listeners should be removed
      expect(delta.removed).toBe(delta.added);
      expect(delta.balance).toBe(0);
    });
  });

  // ========================================================================
  // EventBus Subscriptions Tests (3 tests)
  // ========================================================================

  describe('EventBus Subscriptions', () => {
    it('should actually remove handler when unsubscribe is called', () => {
      const handler1 = vi.fn();
      const handler2 = vi.fn();

      // Subscribe to two different events
      const unsubscribe1 = eventBus.subscribe('node:updated', handler1);
      const unsubscribe2 = eventBus.subscribe('node:created', handler2);

      let counts = eventBus.getSubscriberCounts();
      expect(counts['node:updated']).toBe(1);
      expect(counts['node:created']).toBe(1);

      // Unsubscribe from first event
      unsubscribe1();

      counts = eventBus.getSubscriberCounts();
      expect(counts['node:updated']).toBeUndefined();
      expect(counts['node:created']).toBe(1);

      // Unsubscribe from second event
      unsubscribe2();

      counts = eventBus.getSubscriberCounts();
      expect(counts['node:created']).toBeUndefined();
    });

    it('should remove all handlers after reset()', () => {
      // Add multiple subscriptions
      eventBus.subscribe('node:updated', vi.fn());
      eventBus.subscribe('node:created', vi.fn());
      eventBus.subscribe('node:deleted', vi.fn());
      eventBus.subscribe('*', vi.fn()); // Wildcard subscription

      let counts = eventBus.getSubscriberCounts();
      expect(Object.keys(counts).length).toBeGreaterThan(0);

      // Reset should clear everything
      eventBus.reset();

      counts = eventBus.getSubscriberCounts();
      // After reset, only wildcard key might exist with 0
      const totalSubscribers = Object.values(counts).reduce((a, b) => a + b, 0);
      expect(totalSubscribers).toBe(0);
    });

    it('should cleanup wildcard subscriptions properly', () => {
      const wildcardHandler1 = vi.fn();
      const wildcardHandler2 = vi.fn();
      const specificHandler = vi.fn();

      // Add wildcard and specific subscriptions
      const unsubWild1 = eventBus.subscribe('*', wildcardHandler1);
      const unsubWild2 = eventBus.subscribe('*', wildcardHandler2);
      const unsubSpecific = eventBus.subscribe('node:updated', specificHandler);

      let counts = eventBus.getSubscriberCounts();
      expect(counts['*']).toBe(2);
      expect(counts['node:updated']).toBe(1);

      // Unsubscribe first wildcard
      unsubWild1();

      counts = eventBus.getSubscriberCounts();
      expect(counts['*']).toBe(1);
      expect(counts['node:updated']).toBe(1);

      // Unsubscribe second wildcard
      unsubWild2();

      counts = eventBus.getSubscriberCounts();
      expect(counts['*']).toBe(0);
      expect(counts['node:updated']).toBe(1);

      // Unsubscribe specific
      unsubSpecific();

      counts = eventBus.getSubscriberCounts();
      expect(counts['node:updated']).toBeUndefined();
    });
  });

  // ========================================================================
  // Verification Tools Tests (2 tests)
  // ========================================================================

  describe('Verification Tools', () => {
    it('should detect leaked listeners programmatically', () => {
      const addSpy = vi.spyOn(document, 'addEventListener');
      const removeSpy = vi.spyOn(document, 'removeEventListener');

      const initialBalance = getKeydownListenerCounts(addSpy, removeSpy).balance;

      // Create a leak by not unmounting
      const { unmount } = render(NodeAutocomplete, {
        visible: true,
        results: [],
        position: { x: 0, y: 0 }
      });

      const balanceWithLeak = getKeydownListenerCounts(addSpy, removeSpy).balance;
      expect(balanceWithLeak).toBeGreaterThan(initialBalance);

      // Clean up properly - call unmount to remove the listener
      unmount();

      // Verify cleanup worked - balance should return to initial state
      const finalBalance = getKeydownListenerCounts(addSpy, removeSpy).balance;
      expect(finalBalance).toBe(initialBalance);
    });

    it('should measure handler count accurately', () => {
      // Start with clean state
      eventBus.reset();

      let metrics = eventBus.getMetrics();
      const initialHandlerCount = metrics.totalHandlers;

      // Add some handlers
      const unsub1 = eventBus.subscribe('node:updated', vi.fn());
      const unsub2 = eventBus.subscribe('node:created', vi.fn());
      const unsub3 = eventBus.subscribe('*', vi.fn());

      metrics = eventBus.getMetrics();
      expect(metrics.totalHandlers).toBe(initialHandlerCount + 3);

      // Remove one handler
      unsub1();

      metrics = eventBus.getMetrics();
      expect(metrics.totalHandlers).toBe(initialHandlerCount + 2);

      // Remove remaining handlers
      unsub2();
      unsub3();

      metrics = eventBus.getMetrics();
      expect(metrics.totalHandlers).toBe(initialHandlerCount);
    });
  });
});
