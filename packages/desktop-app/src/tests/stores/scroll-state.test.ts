import { describe, it, expect, beforeEach } from 'vitest';
import {
  getViewerId,
  saveScrollPosition,
  getScrollPosition,
  clearScrollPosition,
  clearPaneScrollPositions,
  getScrollStateSize,
  getScrollStateKeys,
  clearAllScrollPositions
} from '$lib/stores/scroll-state';

describe('Scroll State Store', () => {
  beforeEach(() => {
    // Clear all scroll positions before each test to ensure test isolation
    clearAllScrollPositions();
  });

  describe('getViewerId', () => {
    it('should generate unique viewer IDs from nodeId, tabId and paneId', () => {
      expect(getViewerId('node-1', 'tab-1', 'pane-1')).toBe('node-1-tab-1-pane-1');
      expect(getViewerId('node-2', 'tab-2', 'pane-2')).toBe('node-2-tab-2-pane-2');
      expect(getViewerId('my-node', 'my-tab', 'my-pane')).toBe('my-node-my-tab-my-pane');
    });

    it('should generate different IDs for different node/tab/pane combinations', () => {
      const id1 = getViewerId('node-1', 'tab-1', 'pane-1');
      const id2 = getViewerId('node-1', 'tab-1', 'pane-2'); // Different pane
      const id3 = getViewerId('node-1', 'tab-2', 'pane-1'); // Different tab
      const id4 = getViewerId('node-2', 'tab-1', 'pane-1'); // Different node

      expect(id1).not.toBe(id2);
      expect(id1).not.toBe(id3);
      expect(id1).not.toBe(id4);
      expect(id2).not.toBe(id3);
      expect(id2).not.toBe(id4);
      expect(id3).not.toBe(id4);
    });

    it('should allow same node in same tab with different panes to have different scroll positions', () => {
      const nodeId = 'shared-node';
      const tabId = 'shared-tab';

      const leftId = getViewerId(nodeId, tabId, 'pane-left');
      const rightId = getViewerId(nodeId, tabId, 'pane-right');

      expect(leftId).not.toBe(rightId);
    });
  });

  describe('saveScrollPosition and getScrollPosition', () => {
    it('should save and retrieve scroll positions', () => {
      saveScrollPosition('viewer-1', 150);
      expect(getScrollPosition('viewer-1')).toBe(150);
    });

    it('should handle multiple scroll positions independently', () => {
      saveScrollPosition('viewer-1', 100);
      saveScrollPosition('viewer-2', 200);
      saveScrollPosition('viewer-3', 300);

      expect(getScrollPosition('viewer-1')).toBe(100);
      expect(getScrollPosition('viewer-2')).toBe(200);
      expect(getScrollPosition('viewer-3')).toBe(300);
    });

    it('should update existing scroll position when saving again', () => {
      saveScrollPosition('viewer-1', 100);
      expect(getScrollPosition('viewer-1')).toBe(100);

      saveScrollPosition('viewer-1', 250);
      expect(getScrollPosition('viewer-1')).toBe(250);
    });

    it('should return 0 for non-existent viewer IDs', () => {
      expect(getScrollPosition('non-existent')).toBe(0);
      expect(getScrollPosition('another-missing-id')).toBe(0);
    });

    it('should handle zero scroll position correctly', () => {
      saveScrollPosition('viewer-top', 0);
      expect(getScrollPosition('viewer-top')).toBe(0);
    });

    it('should handle large scroll positions', () => {
      const largePosition = 999999;
      saveScrollPosition('viewer-long', largePosition);
      expect(getScrollPosition('viewer-long')).toBe(largePosition);
    });
  });

  describe('clearScrollPosition', () => {
    it('should clear individual scroll positions', () => {
      saveScrollPosition('viewer-1', 100);
      expect(getScrollPosition('viewer-1')).toBe(100);

      clearScrollPosition('viewer-1');
      expect(getScrollPosition('viewer-1')).toBe(0);
    });

    it('should not affect other scroll positions when clearing one', () => {
      saveScrollPosition('viewer-1', 100);
      saveScrollPosition('viewer-2', 200);
      saveScrollPosition('viewer-3', 300);

      clearScrollPosition('viewer-2');

      expect(getScrollPosition('viewer-1')).toBe(100);
      expect(getScrollPosition('viewer-2')).toBe(0);
      expect(getScrollPosition('viewer-3')).toBe(300);
    });

    it('should handle clearing non-existent positions gracefully', () => {
      expect(() => clearScrollPosition('non-existent')).not.toThrow();
      expect(getScrollPosition('non-existent')).toBe(0);
    });
  });

  describe('clearPaneScrollPositions', () => {
    it('should clear all scroll positions for a specific pane', () => {
      saveScrollPosition('node-1-tab-1-pane-1', 100);
      saveScrollPosition('node-2-tab-2-pane-1', 200);
      saveScrollPosition('node-3-tab-3-pane-1', 300);
      saveScrollPosition('node-1-tab-1-pane-2', 400);

      clearPaneScrollPositions('pane-1');

      // All pane-1 positions should be cleared
      expect(getScrollPosition('node-1-tab-1-pane-1')).toBe(0);
      expect(getScrollPosition('node-2-tab-2-pane-1')).toBe(0);
      expect(getScrollPosition('node-3-tab-3-pane-1')).toBe(0);

      // pane-2 should remain
      expect(getScrollPosition('node-1-tab-1-pane-2')).toBe(400);
    });

    it('should handle pane IDs that are substrings of other IDs correctly', () => {
      // Edge case: pane-1 is a substring of pane-10, pane-11, etc.
      saveScrollPosition('node-1-tab-1-pane-1', 100);
      saveScrollPosition('node-1-tab-1-pane-10', 200);
      saveScrollPosition('node-1-tab-1-pane-11', 300);

      clearPaneScrollPositions('pane-1');

      // Only exact pane-1 should be cleared (endsWith match)
      expect(getScrollPosition('node-1-tab-1-pane-1')).toBe(0);

      // pane-10 and pane-11 should remain (they don't end with 'pane-1')
      expect(getScrollPosition('node-1-tab-1-pane-10')).toBe(200);
      expect(getScrollPosition('node-1-tab-1-pane-11')).toBe(300);
    });

    it('should handle clearing panes with no scroll positions', () => {
      expect(() => clearPaneScrollPositions('non-existent-pane')).not.toThrow();
    });

    it('should clear multiple nodes/tabs in the same pane', () => {
      // Simulate multiple tabs open in same pane with different nodes
      for (let i = 1; i <= 10; i++) {
        saveScrollPosition(`node-${i}-tab-${i}-pane-main`, i * 100);
      }

      clearPaneScrollPositions('pane-main');

      // All should be cleared
      for (let i = 1; i <= 10; i++) {
        expect(getScrollPosition(`node-${i}-tab-${i}-pane-main`)).toBe(0);
      }
    });
  });

  describe('Real-world usage scenarios', () => {
    it('should handle typical multi-pane workflow', () => {
      // User opens multiple tabs in two panes, viewing different nodes
      const leftPane = 'pane-left';
      const rightPane = 'pane-right';

      // Left pane tabs with different nodes
      saveScrollPosition(getViewerId('node-a', 'tab-1', leftPane), 100);
      saveScrollPosition(getViewerId('node-b', 'tab-2', leftPane), 200);

      // Right pane tabs
      saveScrollPosition(getViewerId('node-a', 'tab-1', rightPane), 300);
      saveScrollPosition(getViewerId('node-c', 'tab-3', rightPane), 400);

      // Verify all positions
      expect(getScrollPosition(getViewerId('node-a', 'tab-1', leftPane))).toBe(100);
      expect(getScrollPosition(getViewerId('node-b', 'tab-2', leftPane))).toBe(200);
      expect(getScrollPosition(getViewerId('node-a', 'tab-1', rightPane))).toBe(300);
      expect(getScrollPosition(getViewerId('node-c', 'tab-3', rightPane))).toBe(400);

      // User closes right pane
      clearPaneScrollPositions(rightPane);

      // Left pane should remain
      expect(getScrollPosition(getViewerId('node-a', 'tab-1', leftPane))).toBe(100);
      expect(getScrollPosition(getViewerId('node-b', 'tab-2', leftPane))).toBe(200);

      // Right pane should be cleared
      expect(getScrollPosition(getViewerId('node-a', 'tab-1', rightPane))).toBe(0);
      expect(getScrollPosition(getViewerId('node-c', 'tab-3', rightPane))).toBe(0);
    });

    it('should handle same node in different panes (split view)', () => {
      const nodeId = 'important-doc';
      const tabId = 'same-tab';
      const leftPane = 'pane-left';
      const rightPane = 'pane-right';

      // Same node open in two panes, different scroll positions
      saveScrollPosition(getViewerId(nodeId, tabId, leftPane), 500);
      saveScrollPosition(getViewerId(nodeId, tabId, rightPane), 1000);

      // Each pane should maintain independent scroll
      expect(getScrollPosition(getViewerId(nodeId, tabId, leftPane))).toBe(500);
      expect(getScrollPosition(getViewerId(nodeId, tabId, rightPane))).toBe(1000);
    });

    it('should handle navigating between nodes in same tab', () => {
      // This is the key scenario: same tab, different nodes should have different scroll positions
      const tabId = 'my-tab';
      const paneId = 'my-pane';

      // User scrolls through node-a
      saveScrollPosition(getViewerId('node-a', tabId, paneId), 500);

      // User navigates to node-b in same tab (should start at 0 since no saved position)
      expect(getScrollPosition(getViewerId('node-b', tabId, paneId))).toBe(0);

      // User scrolls through node-b
      saveScrollPosition(getViewerId('node-b', tabId, paneId), 750);

      // Navigate back to node-a - should restore original position
      expect(getScrollPosition(getViewerId('node-a', tabId, paneId))).toBe(500);
    });

    it('should handle tab close cleanup', () => {
      const nodeId = 'temp-node';
      const tabId = 'temp-tab';
      const panes = ['pane-1', 'pane-2', 'pane-3'];

      // Node appears in multiple panes
      panes.forEach((paneId, index) => {
        saveScrollPosition(getViewerId(nodeId, tabId, paneId), (index + 1) * 100);
      });

      // Simulate closing tab (clear from all panes)
      panes.forEach((paneId) => {
        clearScrollPosition(getViewerId(nodeId, tabId, paneId));
      });

      // All instances should be cleared
      panes.forEach((paneId) => {
        expect(getScrollPosition(getViewerId(nodeId, tabId, paneId))).toBe(0);
      });
    });
  });

  describe('Monitoring utilities', () => {
    it('should track scroll state size correctly', () => {
      // After beforeEach cleanup, size should be 0
      expect(getScrollStateSize()).toBe(0);

      saveScrollPosition('viewer-1', 100);
      expect(getScrollStateSize()).toBe(1);

      saveScrollPosition('viewer-2', 200);
      expect(getScrollStateSize()).toBe(2);

      clearScrollPosition('viewer-1');
      expect(getScrollStateSize()).toBe(1);

      clearScrollPosition('viewer-2');
      expect(getScrollStateSize()).toBe(0);
    });

    it('should track size correctly with clearPaneScrollPositions', () => {
      expect(getScrollStateSize()).toBe(0);

      saveScrollPosition('node-1-tab-1-pane-1', 100);
      saveScrollPosition('node-2-tab-2-pane-1', 200);
      saveScrollPosition('node-1-tab-1-pane-2', 300);

      expect(getScrollStateSize()).toBe(3);

      clearPaneScrollPositions('pane-1');
      expect(getScrollStateSize()).toBe(1); // Only pane-2 remains
    });

    it('should return all viewer IDs in development mode', () => {
      expect(getScrollStateSize()).toBe(0);

      saveScrollPosition('viewer-1', 100);
      saveScrollPosition('viewer-2', 200);

      const keys = getScrollStateKeys();

      // In dev mode, should return array with keys
      if (import.meta.env.DEV) {
        expect(keys).toContain('viewer-1');
        expect(keys).toContain('viewer-2');
        expect(keys.length).toBe(2);
      } else {
        // In production, should return empty array
        expect(keys).toEqual([]);
      }
    });

    it('should help detect memory leaks in tests', () => {
      // Start fresh
      expect(getScrollStateSize()).toBe(0);

      // Open many tabs across multiple panes
      for (let i = 0; i < 50; i++) {
        saveScrollPosition(`node-${i}-tab-${i}-pane-1`, i * 100);
        saveScrollPosition(`node-${i}-tab-${i}-pane-2`, i * 200);
      }

      expect(getScrollStateSize()).toBe(100);

      // Close all tabs - should clean up properly
      for (let i = 0; i < 50; i++) {
        clearScrollPosition(`node-${i}-tab-${i}-pane-1`);
        clearScrollPosition(`node-${i}-tab-${i}-pane-2`);
      }

      expect(getScrollStateSize()).toBe(0);
    });
  });
});
