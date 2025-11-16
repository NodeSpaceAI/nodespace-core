import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { TabPersistenceService } from '$lib/services/tab-persistence-service';
import type { TabState } from '$lib/stores/navigation';

describe('TabPersistenceService', () => {
  beforeEach(() => {
    // Clear localStorage before each test
    localStorage.clear();
    // Clear any pending timers
    TabPersistenceService.flush();
    // Use fake timers for debounce testing
    vi.useFakeTimers();
  });

  afterEach(() => {
    // Restore real timers
    vi.useRealTimers();
  });

  describe('save and load', () => {
    const mockState: TabState = {
      tabs: [
        {
          id: 'tab-1',
          title: 'Test Tab',
          type: 'node',
          content: { nodeId: 'node-1', nodeType: 'text' },
          closeable: true,
          paneId: 'pane-1'
        }
      ],
      panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
      activePaneId: 'pane-1',
      activeTabIds: { 'pane-1': 'tab-1' }
    };

    it('persists and retrieves tab state', () => {
      TabPersistenceService.save(mockState);

      // Fast-forward debounce timer
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();

      expect(loaded).toEqual({
        version: 1,
        tabs: mockState.tabs,
        panes: mockState.panes,
        activePaneId: mockState.activePaneId,
        activeTabIds: mockState.activeTabIds
      });
    });

    it('returns null when no saved state exists', () => {
      const loaded = TabPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('handles corrupted JSON gracefully', () => {
      localStorage.setItem('nodespace:tab-state', 'invalid-json{]');

      const loaded = TabPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('persists multiple tabs and panes correctly', () => {
      const complexState: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Tab 1',
            type: 'node',
            content: { nodeId: 'node-1' },
            closeable: true,
            paneId: 'pane-1'
          },
          {
            id: 'tab-2',
            title: 'Tab 2',
            type: 'placeholder',
            closeable: false,
            paneId: 'pane-2'
          }
        ],
        panes: [
          { id: 'pane-1', width: 50, tabIds: ['tab-1'] },
          { id: 'pane-2', width: 50, tabIds: ['tab-2'] }
        ],
        activePaneId: 'pane-2',
        activeTabIds: { 'pane-1': 'tab-1', 'pane-2': 'tab-2' }
      };

      TabPersistenceService.save(complexState);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs).toHaveLength(2);
      expect(loaded?.panes).toHaveLength(2);
      expect(loaded?.activePaneId).toBe('pane-2');
    });
  });

  describe('validation', () => {
    it('rejects state with invalid structure', () => {
      localStorage.setItem('nodespace:tab-state', JSON.stringify({ invalid: 'data' }));

      const loaded = TabPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with missing version', () => {
      localStorage.setItem(
        'nodespace:tab-state',
        JSON.stringify({
          tabs: [],
          panes: [],
          activePaneId: 'pane-1',
          activeTabIds: {}
        })
      );

      const loaded = TabPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with invalid tabs array', () => {
      localStorage.setItem(
        'nodespace:tab-state',
        JSON.stringify({
          version: 1,
          tabs: [null, undefined, 'invalid'],
          panes: [],
          activePaneId: 'pane-1',
          activeTabIds: {}
        })
      );

      const loaded = TabPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with malformed tab objects', () => {
      localStorage.setItem(
        'nodespace:tab-state',
        JSON.stringify({
          version: 1,
          tabs: [{ id: 'tab-1' }], // Missing required fields
          panes: [],
          activePaneId: 'pane-1',
          activeTabIds: {}
        })
      );

      const loaded = TabPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with invalid panes array', () => {
      localStorage.setItem(
        'nodespace:tab-state',
        JSON.stringify({
          version: 1,
          tabs: [],
          panes: [{ id: 'pane-1' }], // Missing width and tabIds
          activePaneId: 'pane-1',
          activeTabIds: {}
        })
      );

      const loaded = TabPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with invalid tab type', () => {
      localStorage.setItem(
        'nodespace:tab-state',
        JSON.stringify({
          version: 1,
          tabs: [
            {
              id: 'tab-1',
              title: 'Test',
              type: 'invalid-type', // Invalid type
              closeable: true,
              paneId: 'pane-1'
            }
          ],
          panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
          activePaneId: 'pane-1',
          activeTabIds: { 'pane-1': 'tab-1' }
        })
      );

      const loaded = TabPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('accepts valid state with all required fields', () => {
      const validState = {
        version: 1,
        tabs: [
          {
            id: 'tab-1',
            title: 'Test',
            type: 'node',
            closeable: true,
            paneId: 'pane-1'
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      localStorage.setItem('nodespace:tab-state', JSON.stringify(validState));

      const loaded = TabPersistenceService.load();
      expect(loaded).not.toBeNull();
      expect(loaded?.version).toBe(1);
    });
  });

  describe('debouncing', () => {
    it('debounces rapid saves', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const state1: TabState = {
        tabs: [{ id: 'tab-1', title: 'T1', type: 'node', closeable: true, paneId: 'pane-1' }],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      const state2: TabState = {
        ...state1,
        tabs: [{ id: 'tab-2', title: 'T2', type: 'node', closeable: true, paneId: 'pane-1' }]
      };

      const state3: TabState = {
        ...state1,
        tabs: [{ id: 'tab-3', title: 'T3', type: 'node', closeable: true, paneId: 'pane-1' }]
      };

      // Rapid saves
      TabPersistenceService.save(state1);
      TabPersistenceService.save(state2);
      TabPersistenceService.save(state3);

      // Should not have saved yet
      expect(spy).not.toHaveBeenCalled();

      // Fast-forward debounce timer
      vi.advanceTimersByTime(500);

      // Should have saved exactly once with the last state
      expect(spy).toHaveBeenCalledOnce();

      const savedData = JSON.parse(spy.mock.calls[0][1] as string);
      expect(savedData.tabs[0].id).toBe('tab-3');
    });

    it('resets debounce timer on each save', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const mockState: TabState = {
        tabs: [{ id: 'tab-1', title: 'T1', type: 'node', closeable: true, paneId: 'pane-1' }],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(mockState);
      vi.advanceTimersByTime(300);

      // Save again before timeout
      TabPersistenceService.save(mockState);
      vi.advanceTimersByTime(300);

      // Should still not have saved
      expect(spy).not.toHaveBeenCalled();

      // Complete the debounce
      vi.advanceTimersByTime(200);

      // Now should have saved
      expect(spy).toHaveBeenCalledOnce();
    });
  });

  describe('clear', () => {
    it('removes persisted state', () => {
      const mockState: TabState = {
        tabs: [{ id: 'tab-1', title: 'T1', type: 'node', closeable: true, paneId: 'pane-1' }],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(mockState);
      vi.advanceTimersByTime(500);

      // Verify it was saved
      expect(TabPersistenceService.load()).not.toBeNull();

      // Clear state
      TabPersistenceService.clear();

      // Verify it was removed
      expect(TabPersistenceService.load()).toBeNull();
    });
  });

  describe('flush', () => {
    it('saves immediately and clears pending timer', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const mockState: TabState = {
        tabs: [{ id: 'tab-1', title: 'T1', type: 'node', closeable: true, paneId: 'pane-1' }],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(mockState);

      // Flush before debounce completes - this should save immediately
      TabPersistenceService.flush();

      // Should have saved once (from flush)
      expect(spy).toHaveBeenCalledOnce();

      // Advance timers to verify timer was cleared (no second save)
      vi.advanceTimersByTime(1000);

      // Should still be called only once (timer was cleared)
      expect(spy).toHaveBeenCalledOnce();
    });

    it('can be called multiple times safely', () => {
      TabPersistenceService.flush();
      TabPersistenceService.flush();
      TabPersistenceService.flush();

      // Should not throw
      expect(true).toBe(true);
    });
  });

  describe('edge cases', () => {
    it('handles empty tabs and panes arrays', () => {
      const emptyState: TabState = {
        tabs: [],
        panes: [],
        activePaneId: '',
        activeTabIds: {}
      };

      TabPersistenceService.save(emptyState);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs).toEqual([]);
      expect(loaded?.panes).toEqual([]);
    });

    it('handles localStorage quota exceeded', () => {
      // Mock setItem to throw quota exceeded error
      const originalSetItem = window.localStorage.setItem;
      window.localStorage.setItem = vi.fn(() => {
        throw new Error('QuotaExceededError');
      });

      const mockState: TabState = {
        tabs: [{ id: 'tab-1', title: 'T1', type: 'node', closeable: true, paneId: 'pane-1' }],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      // Should not throw
      expect(() => {
        TabPersistenceService.save(mockState);
        vi.advanceTimersByTime(500);
      }).not.toThrow();

      // Restore original implementation
      window.localStorage.setItem = originalSetItem;
    });

    it('handles special characters in tab titles', () => {
      const specialState: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Test "with" \'quotes\' & <html>',
            type: 'node',
            closeable: true,
            paneId: 'pane-1'
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(specialState);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].title).toBe('Test "with" \'quotes\' & <html>');
    });
  });

  describe('date node title migration', () => {
    it('recomputes date titles on load', () => {
      const today = new Date().toISOString().split('T')[0];
      const stateWithDateNode: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Untitled', // Stale title
            type: 'node',
            content: { nodeId: today, nodeType: 'date' },
            closeable: true,
            paneId: 'pane-1'
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(stateWithDateNode);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].title).toBe('Today'); // Should be recomputed
    });

    it('preserves titles for non-date nodes', () => {
      const stateWithTextNode: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'My Custom Title',
            type: 'node',
            content: { nodeId: 'text-node-123', nodeType: 'text' },
            closeable: true,
            paneId: 'pane-1'
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(stateWithTextNode);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].title).toBe('My Custom Title'); // Should NOT change
    });

    it('handles invalid date strings gracefully', () => {
      const stateWithInvalidDate: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Invalid Date',
            type: 'node',
            content: { nodeId: 'not-a-date', nodeType: 'date' },
            closeable: true,
            paneId: 'pane-1'
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(stateWithInvalidDate);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].title).toBe('Invalid Date'); // Should preserve original
    });

    it('recomputes tomorrow date title correctly', () => {
      const tomorrow = new Date();
      tomorrow.setDate(tomorrow.getDate() + 1);
      const tomorrowStr = tomorrow.toISOString().split('T')[0];

      const stateWithTomorrow: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Some Old Title',
            type: 'node',
            content: { nodeId: tomorrowStr, nodeType: 'date' },
            closeable: true,
            paneId: 'pane-1'
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(stateWithTomorrow);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].title).toBe('Tomorrow'); // Should be recomputed
    });

    it('recomputes yesterday date title correctly', () => {
      const yesterday = new Date();
      yesterday.setDate(yesterday.getDate() - 1);
      const yesterdayStr = yesterday.toISOString().split('T')[0];

      const stateWithYesterday: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Some Old Title',
            type: 'node',
            content: { nodeId: yesterdayStr, nodeType: 'date' },
            closeable: true,
            paneId: 'pane-1'
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(stateWithYesterday);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].title).toBe('Yesterday'); // Should be recomputed
    });

    it('handles date nodes without nodeId gracefully', () => {
      // Test edge case where content.nodeId is missing (should preserve original title)
      // Using type assertion since this is testing malformed data that shouldn't normally occur
      const stateWithMissingNodeId: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Original Title',
            type: 'node',
            content: { nodeType: 'date' } as { nodeId: string; nodeType: string }, // Malformed: missing nodeId
            closeable: true,
            paneId: 'pane-1'
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      TabPersistenceService.save(stateWithMissingNodeId);
      vi.advanceTimersByTime(500);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].title).toBe('Original Title'); // Should preserve original
    });
  });
});
