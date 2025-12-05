import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { LayoutPersistenceService } from '$lib/services/layout-persistence-service';
import type { LayoutState } from '$lib/stores/layout';

describe('LayoutPersistenceService', () => {
  beforeEach(() => {
    // Clear localStorage before each test
    localStorage.clear();
    // Clear any pending timers
    LayoutPersistenceService.flush();
    // Use fake timers for debounce testing
    vi.useFakeTimers();
  });

  afterEach(() => {
    // Restore real timers
    vi.useRealTimers();
  });

  describe('save and load', () => {
    const mockState: LayoutState = {
      sidebarCollapsed: true,
      activePane: 'dashboard'
    };

    it('persists and retrieves layout state', () => {
      LayoutPersistenceService.save(mockState);

      // Fast-forward debounce timer
      vi.advanceTimersByTime(500);

      const loaded = LayoutPersistenceService.load();

      expect(loaded).toEqual({
        version: 1,
        sidebarCollapsed: true
      });
    });

    it('returns null when no saved state exists', () => {
      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('handles corrupted JSON gracefully', () => {
      localStorage.setItem('nodespace:layout-state', 'invalid-json{]');

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('persists sidebar collapsed state correctly', () => {
      const collapsedState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.save(collapsedState);
      vi.advanceTimersByTime(500);

      const loaded = LayoutPersistenceService.load();
      expect(loaded?.sidebarCollapsed).toBe(true);
    });

    it('persists sidebar expanded state correctly', () => {
      const expandedState: LayoutState = {
        sidebarCollapsed: false,
        activePane: 'today'
      };

      LayoutPersistenceService.save(expandedState);
      vi.advanceTimersByTime(500);

      const loaded = LayoutPersistenceService.load();
      expect(loaded?.sidebarCollapsed).toBe(false);
    });

    it('only persists sidebarCollapsed, not activePane', () => {
      const state: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'some-custom-pane'
      };

      LayoutPersistenceService.save(state);
      vi.advanceTimersByTime(500);

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toEqual({
        version: 1,
        sidebarCollapsed: true
      });
      // activePane should NOT be in the persisted state
      expect(loaded).not.toHaveProperty('activePane');
    });
  });

  describe('validation', () => {
    it('rejects state with invalid structure', () => {
      localStorage.setItem('nodespace:layout-state', JSON.stringify({ invalid: 'data' }));

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with missing version', () => {
      localStorage.setItem(
        'nodespace:layout-state',
        JSON.stringify({
          sidebarCollapsed: true
        })
      );

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with invalid version type', () => {
      localStorage.setItem(
        'nodespace:layout-state',
        JSON.stringify({
          version: '1', // String instead of number
          sidebarCollapsed: true
        })
      );

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with missing sidebarCollapsed', () => {
      localStorage.setItem(
        'nodespace:layout-state',
        JSON.stringify({
          version: 1
        })
      );

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects state with invalid sidebarCollapsed type', () => {
      localStorage.setItem(
        'nodespace:layout-state',
        JSON.stringify({
          version: 1,
          sidebarCollapsed: 'true' // String instead of boolean
        })
      );

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects null state', () => {
      localStorage.setItem('nodespace:layout-state', 'null');

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects undefined in JSON', () => {
      localStorage.setItem('nodespace:layout-state', 'undefined');

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects non-object state', () => {
      localStorage.setItem('nodespace:layout-state', JSON.stringify('string'));

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('rejects array state', () => {
      localStorage.setItem('nodespace:layout-state', JSON.stringify([1, 2, 3]));

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('accepts valid state with all required fields', () => {
      const validState = {
        version: 1,
        sidebarCollapsed: false
      };

      localStorage.setItem('nodespace:layout-state', JSON.stringify(validState));

      const loaded = LayoutPersistenceService.load();
      expect(loaded).not.toBeNull();
      expect(loaded?.version).toBe(1);
      expect(loaded?.sidebarCollapsed).toBe(false);
    });

    it('accepts state with extra fields (forwards compatibility)', () => {
      const stateWithExtras = {
        version: 1,
        sidebarCollapsed: true,
        futureField: 'some value'
      };

      localStorage.setItem('nodespace:layout-state', JSON.stringify(stateWithExtras));

      const loaded = LayoutPersistenceService.load();
      expect(loaded).not.toBeNull();
      expect(loaded?.version).toBe(1);
      expect(loaded?.sidebarCollapsed).toBe(true);
    });
  });

  describe('debouncing', () => {
    it('debounces rapid saves', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const state1: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'pane-1'
      };

      const state2: LayoutState = {
        sidebarCollapsed: false,
        activePane: 'pane-2'
      };

      const state3: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'pane-3'
      };

      // Rapid saves
      LayoutPersistenceService.save(state1);
      LayoutPersistenceService.save(state2);
      LayoutPersistenceService.save(state3);

      // Should not have saved yet
      expect(spy).not.toHaveBeenCalled();

      // Fast-forward debounce timer
      vi.advanceTimersByTime(500);

      // Should have saved exactly once with the last state
      expect(spy).toHaveBeenCalledOnce();

      const savedData = JSON.parse(spy.mock.calls[0][1] as string);
      expect(savedData.sidebarCollapsed).toBe(true);
    });

    it('resets debounce timer on each save', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.save(mockState);
      vi.advanceTimersByTime(200);

      // Save again before timeout (300ms debounce)
      LayoutPersistenceService.save(mockState);
      vi.advanceTimersByTime(200);

      // Should still not have saved
      expect(spy).not.toHaveBeenCalled();

      // Complete the debounce
      vi.advanceTimersByTime(200);

      // Now should have saved
      expect(spy).toHaveBeenCalledOnce();
    });

    it('uses 300ms debounce timeout', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.save(mockState);

      // Just before debounce timeout
      vi.advanceTimersByTime(299);
      expect(spy).not.toHaveBeenCalled();

      // At debounce timeout
      vi.advanceTimersByTime(1);
      expect(spy).toHaveBeenCalledOnce();
    });
  });

  describe('clear', () => {
    it('removes persisted state', () => {
      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.save(mockState);
      vi.advanceTimersByTime(500);

      // Verify it was saved
      expect(LayoutPersistenceService.load()).not.toBeNull();

      // Clear state
      LayoutPersistenceService.clear();

      // Verify it was removed
      expect(LayoutPersistenceService.load()).toBeNull();
    });

    it('handles clearing non-existent state gracefully', () => {
      // Should not throw when clearing empty storage
      expect(() => {
        LayoutPersistenceService.clear();
      }).not.toThrow();
    });

    it('handles localStorage errors during clear', () => {
      // Spy on removeItem and make it throw error
      const spy = vi.spyOn(window.localStorage, 'removeItem');
      spy.mockImplementationOnce(() => {
        throw new Error('Storage error');
      });

      // Should not throw
      expect(() => {
        LayoutPersistenceService.clear();
      }).not.toThrow();

      // Verify removeItem was called
      expect(spy).toHaveBeenCalledWith('nodespace:layout-state');

      // Restore spy
      spy.mockRestore();
    });
  });

  describe('flush', () => {
    it('saves immediately and clears pending timer', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.save(mockState);

      // Flush before debounce completes - this should save immediately
      LayoutPersistenceService.flush();

      // Should have saved once (from flush)
      expect(spy).toHaveBeenCalledOnce();

      // Advance timers to verify timer was cleared (no second save)
      vi.advanceTimersByTime(1000);

      // Should still be called only once (timer was cleared)
      expect(spy).toHaveBeenCalledOnce();
    });

    it('can be called multiple times safely', () => {
      expect(() => {
        LayoutPersistenceService.flush();
        LayoutPersistenceService.flush();
        LayoutPersistenceService.flush();
      }).not.toThrow();
    });

    it('does nothing when no pending state exists', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      LayoutPersistenceService.flush();

      // Should not have saved anything
      expect(spy).not.toHaveBeenCalled();
    });

    it('clears pending state after flush', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.save(mockState);
      LayoutPersistenceService.flush();

      expect(spy).toHaveBeenCalledOnce();

      // Flush again should not save again
      spy.mockClear();
      LayoutPersistenceService.flush();
      expect(spy).not.toHaveBeenCalled();
    });
  });

  describe('saveNow', () => {
    it('saves immediately without debouncing', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.saveNow(mockState);

      // Should have saved immediately without waiting
      expect(spy).toHaveBeenCalledOnce();

      const savedData = JSON.parse(spy.mock.calls[0][1] as string);
      expect(savedData.version).toBe(1);
      expect(savedData.sidebarCollapsed).toBe(true);
    });

    it('does not affect pending debounced saves', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const state1: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      const state2: LayoutState = {
        sidebarCollapsed: false,
        activePane: 'dashboard'
      };

      // Start a debounced save
      LayoutPersistenceService.save(state1);

      // Immediate save
      LayoutPersistenceService.saveNow(state2);

      // Should have saved once (immediate)
      expect(spy).toHaveBeenCalledOnce();
      expect(JSON.parse(spy.mock.calls[0][1] as string).sidebarCollapsed).toBe(false);

      // Advance timer to trigger debounced save
      vi.advanceTimersByTime(500);

      // Should have saved twice (immediate + debounced)
      expect(spy).toHaveBeenCalledTimes(2);
      expect(JSON.parse(spy.mock.calls[1][1] as string).sidebarCollapsed).toBe(true);
    });
  });

  describe('error handling', () => {
    it('handles localStorage quota exceeded during save', () => {
      // Mock setItem to throw quota exceeded error
      const originalSetItem = window.localStorage.setItem;
      window.localStorage.setItem = vi.fn(() => {
        throw new Error('QuotaExceededError');
      });

      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      // Should not throw
      expect(() => {
        LayoutPersistenceService.save(mockState);
        vi.advanceTimersByTime(500);
      }).not.toThrow();

      // Restore original implementation
      window.localStorage.setItem = originalSetItem;
    });

    it('clears pending state on save error', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');
      spy.mockImplementationOnce(() => {
        throw new Error('Storage error');
      });

      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.save(mockState);
      vi.advanceTimersByTime(500);

      // Should have attempted to save
      expect(spy).toHaveBeenCalledOnce();

      // Flush should not try to save again (pending state was cleared)
      spy.mockClear();
      spy.mockRestore(); // Restore normal behavior
      LayoutPersistenceService.flush();
      expect(spy).not.toHaveBeenCalled();
    });

    it('handles localStorage errors during load', () => {
      // Mock getItem to throw error
      const originalGetItem = window.localStorage.getItem;
      window.localStorage.getItem = vi.fn(() => {
        throw new Error('Storage error');
      });

      // Should return null and not throw
      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();

      // Restore original implementation
      window.localStorage.getItem = originalGetItem;
    });

    it('handles saveNow with storage error', () => {
      const originalSetItem = window.localStorage.setItem;
      window.localStorage.setItem = vi.fn(() => {
        throw new Error('Storage error');
      });

      const mockState: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      // Should not throw
      expect(() => {
        LayoutPersistenceService.saveNow(mockState);
      }).not.toThrow();

      // Restore original implementation
      window.localStorage.setItem = originalSetItem;
    });
  });

  describe('migration', () => {
    it('handles version 1 state without migration', () => {
      const v1State = {
        version: 1,
        sidebarCollapsed: true
      };

      localStorage.setItem('nodespace:layout-state', JSON.stringify(v1State));

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toEqual(v1State);
    });

    it('preserves all valid fields during load', () => {
      const state = {
        version: 1,
        sidebarCollapsed: false
      };

      localStorage.setItem('nodespace:layout-state', JSON.stringify(state));

      const loaded = LayoutPersistenceService.load();
      expect(loaded?.version).toBe(1);
      expect(loaded?.sidebarCollapsed).toBe(false);
    });
  });

  describe('edge cases', () => {
    it('handles empty localStorage key', () => {
      localStorage.setItem('nodespace:layout-state', '');

      const loaded = LayoutPersistenceService.load();
      expect(loaded).toBeNull();
    });

    it('handles multiple rapid save-flush cycles', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const state1: LayoutState = { sidebarCollapsed: true, activePane: 'today' };
      const state2: LayoutState = { sidebarCollapsed: false, activePane: 'today' };
      const state3: LayoutState = { sidebarCollapsed: true, activePane: 'today' };

      LayoutPersistenceService.save(state1);
      LayoutPersistenceService.flush();

      LayoutPersistenceService.save(state2);
      LayoutPersistenceService.flush();

      LayoutPersistenceService.save(state3);
      LayoutPersistenceService.flush();

      // Should have saved 3 times (one for each flush)
      expect(spy).toHaveBeenCalledTimes(3);
    });

    it('handles alternating save and saveNow calls', () => {
      const spy = vi.spyOn(window.localStorage, 'setItem');

      const state1: LayoutState = { sidebarCollapsed: true, activePane: 'today' };
      const state2: LayoutState = { sidebarCollapsed: false, activePane: 'today' };

      LayoutPersistenceService.save(state1);
      LayoutPersistenceService.saveNow(state2);
      vi.advanceTimersByTime(500);

      // Should have saved twice (one immediate, one debounced)
      expect(spy).toHaveBeenCalledTimes(2);
    });

    it('handles state with only boolean false values', () => {
      const state: LayoutState = {
        sidebarCollapsed: false,
        activePane: 'today'
      };

      LayoutPersistenceService.save(state);
      vi.advanceTimersByTime(500);

      const loaded = LayoutPersistenceService.load();
      expect(loaded?.sidebarCollapsed).toBe(false);
    });

    it('handles state with only boolean true values', () => {
      const state: LayoutState = {
        sidebarCollapsed: true,
        activePane: 'today'
      };

      LayoutPersistenceService.save(state);
      vi.advanceTimersByTime(500);

      const loaded = LayoutPersistenceService.load();
      expect(loaded?.sidebarCollapsed).toBe(true);
    });
  });

  describe('persistence lifecycle', () => {
    it('maintains state across save-load cycles', () => {
      const state1: LayoutState = { sidebarCollapsed: true, activePane: 'today' };

      LayoutPersistenceService.save(state1);
      vi.advanceTimersByTime(500);

      const loaded1 = LayoutPersistenceService.load();
      expect(loaded1?.sidebarCollapsed).toBe(true);

      const state2: LayoutState = { sidebarCollapsed: false, activePane: 'dashboard' };

      LayoutPersistenceService.save(state2);
      vi.advanceTimersByTime(500);

      const loaded2 = LayoutPersistenceService.load();
      expect(loaded2?.sidebarCollapsed).toBe(false);
    });

    it('clear() followed by load() returns null', () => {
      const state: LayoutState = { sidebarCollapsed: true, activePane: 'today' };

      LayoutPersistenceService.save(state);
      vi.advanceTimersByTime(500);

      expect(LayoutPersistenceService.load()).not.toBeNull();

      LayoutPersistenceService.clear();

      expect(LayoutPersistenceService.load()).toBeNull();
    });

    it('handles save after clear', () => {
      const state1: LayoutState = { sidebarCollapsed: true, activePane: 'today' };

      LayoutPersistenceService.save(state1);
      vi.advanceTimersByTime(500);

      LayoutPersistenceService.clear();

      const state2: LayoutState = { sidebarCollapsed: false, activePane: 'dashboard' };

      LayoutPersistenceService.save(state2);
      vi.advanceTimersByTime(500);

      const loaded = LayoutPersistenceService.load();
      expect(loaded?.sidebarCollapsed).toBe(false);
    });
  });
});
