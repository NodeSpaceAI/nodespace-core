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
    it('should generate unique viewer IDs from tabId and paneId', () => {
      expect(getViewerId('tab-1', 'pane-1')).toBe('tab-1-pane-1');
      expect(getViewerId('tab-2', 'pane-2')).toBe('tab-2-pane-2');
      expect(getViewerId('my-tab', 'my-pane')).toBe('my-tab-my-pane');
    });

    it('should generate different IDs for different tab/pane combinations', () => {
      const id1 = getViewerId('tab-1', 'pane-1');
      const id2 = getViewerId('tab-1', 'pane-2');
      const id3 = getViewerId('tab-2', 'pane-1');

      expect(id1).not.toBe(id2);
      expect(id1).not.toBe(id3);
      expect(id2).not.toBe(id3);
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
      saveScrollPosition('tab-1-pane-1', 100);
      saveScrollPosition('tab-2-pane-1', 200);
      saveScrollPosition('tab-3-pane-1', 300);
      saveScrollPosition('tab-1-pane-2', 400);

      clearPaneScrollPositions('pane-1');

      // All pane-1 positions should be cleared
      expect(getScrollPosition('tab-1-pane-1')).toBe(0);
      expect(getScrollPosition('tab-2-pane-1')).toBe(0);
      expect(getScrollPosition('tab-3-pane-1')).toBe(0);

      // pane-2 should remain
      expect(getScrollPosition('tab-1-pane-2')).toBe(400);
    });

    it('should handle pane IDs that are substrings of other IDs correctly', () => {
      // Edge case: pane-1 is a substring of pane-10, pane-11, etc.
      saveScrollPosition('tab-1-pane-1', 100);
      saveScrollPosition('tab-1-pane-10', 200);
      saveScrollPosition('tab-1-pane-11', 300);

      clearPaneScrollPositions('pane-1');

      // Only exact pane-1 should be cleared (endsWith match)
      expect(getScrollPosition('tab-1-pane-1')).toBe(0);

      // pane-10 and pane-11 should remain (they don't end with 'pane-1')
      expect(getScrollPosition('tab-1-pane-10')).toBe(200);
      expect(getScrollPosition('tab-1-pane-11')).toBe(300);
    });

    it('should handle clearing panes with no scroll positions', () => {
      expect(() => clearPaneScrollPositions('non-existent-pane')).not.toThrow();
    });

    it('should clear multiple tabs in the same pane', () => {
      // Simulate multiple tabs open in same pane
      for (let i = 1; i <= 10; i++) {
        saveScrollPosition(`tab-${i}-pane-main`, i * 100);
      }

      clearPaneScrollPositions('pane-main');

      // All should be cleared
      for (let i = 1; i <= 10; i++) {
        expect(getScrollPosition(`tab-${i}-pane-main`)).toBe(0);
      }
    });
  });

  describe('Real-world usage scenarios', () => {
    it('should handle typical multi-pane workflow', () => {
      // User opens multiple tabs in two panes
      const leftPane = 'pane-left';
      const rightPane = 'pane-right';

      // Left pane tabs
      saveScrollPosition(getViewerId('tab-1', leftPane), 100);
      saveScrollPosition(getViewerId('tab-2', leftPane), 200);

      // Right pane tabs
      saveScrollPosition(getViewerId('tab-1', rightPane), 300);
      saveScrollPosition(getViewerId('tab-3', rightPane), 400);

      // Verify all positions
      expect(getScrollPosition(getViewerId('tab-1', leftPane))).toBe(100);
      expect(getScrollPosition(getViewerId('tab-2', leftPane))).toBe(200);
      expect(getScrollPosition(getViewerId('tab-1', rightPane))).toBe(300);
      expect(getScrollPosition(getViewerId('tab-3', rightPane))).toBe(400);

      // User closes right pane
      clearPaneScrollPositions(rightPane);

      // Left pane should remain
      expect(getScrollPosition(getViewerId('tab-1', leftPane))).toBe(100);
      expect(getScrollPosition(getViewerId('tab-2', leftPane))).toBe(200);

      // Right pane should be cleared
      expect(getScrollPosition(getViewerId('tab-1', rightPane))).toBe(0);
      expect(getScrollPosition(getViewerId('tab-3', rightPane))).toBe(0);
    });

    it('should handle same tab in different panes (split view)', () => {
      const tabId = 'important-doc';
      const leftPane = 'pane-left';
      const rightPane = 'pane-right';

      // Same tab open in two panes, different scroll positions
      saveScrollPosition(getViewerId(tabId, leftPane), 500);
      saveScrollPosition(getViewerId(tabId, rightPane), 1000);

      // Each pane should maintain independent scroll
      expect(getScrollPosition(getViewerId(tabId, leftPane))).toBe(500);
      expect(getScrollPosition(getViewerId(tabId, rightPane))).toBe(1000);
    });

    it('should handle tab close cleanup', () => {
      const tabId = 'temp-tab';
      const panes = ['pane-1', 'pane-2', 'pane-3'];

      // Tab appears in multiple panes
      panes.forEach((paneId, index) => {
        saveScrollPosition(getViewerId(tabId, paneId), (index + 1) * 100);
      });

      // Simulate closing tab (clear from all panes)
      panes.forEach((paneId) => {
        clearScrollPosition(getViewerId(tabId, paneId));
      });

      // All instances should be cleared
      panes.forEach((paneId) => {
        expect(getScrollPosition(getViewerId(tabId, paneId))).toBe(0);
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

      saveScrollPosition('tab-1-pane-1', 100);
      saveScrollPosition('tab-2-pane-1', 200);
      saveScrollPosition('tab-1-pane-2', 300);

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
        saveScrollPosition(`tab-${i}-pane-1`, i * 100);
        saveScrollPosition(`tab-${i}-pane-2`, i * 200);
      }

      expect(getScrollStateSize()).toBe(100);

      // Close all tabs - should clean up properly
      for (let i = 0; i < 50; i++) {
        clearScrollPosition(`tab-${i}-pane-1`);
        clearScrollPosition(`tab-${i}-pane-2`);
      }

      expect(getScrollStateSize()).toBe(0);
    });
  });
});
