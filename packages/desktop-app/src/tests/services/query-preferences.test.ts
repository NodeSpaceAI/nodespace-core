import { describe, it, expect, beforeEach, vi } from 'vitest';
import { DEFAULT_QUERY_PREFERENCES } from '$lib/types/query-preferences';

// ---------------------------------------------------------------------------
// Module isolation: re-import the service fresh for each test so the
// in-memory cache Map does not bleed state between tests.
// ---------------------------------------------------------------------------

async function freshService() {
  // Bust the module cache so a new instance is created per test.
  vi.resetModules();
  const mod = await import('$lib/services/query-preferences-service');
  return mod.queryPreferencesService;
}

describe('QueryPreferencesService', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.resetModules();
  });

  // -------------------------------------------------------------------------
  // getPreferences
  // -------------------------------------------------------------------------

  describe('getPreferences', () => {
    it('returns defaults when nothing is stored', async () => {
      const svc = await freshService();
      const prefs = svc.getPreferences('query-1');

      expect(prefs.lastView).toBe(DEFAULT_QUERY_PREFERENCES.lastView);
      expect(prefs.viewConfigs).toEqual({});
    });

    it('default lastView is "table"', async () => {
      const svc = await freshService();
      const prefs = svc.getPreferences('query-new');

      expect(prefs.lastView).toBe('table');
    });

    it('returns stored preferences from localStorage', async () => {
      const stored = {
        lastView: 'list',
        viewConfigs: {
          list: { view: 'list', layout: 'compact' }
        }
      };
      localStorage.setItem('query-prefs-query-2', JSON.stringify(stored));

      const svc = await freshService();
      const prefs = svc.getPreferences('query-2');

      expect(prefs.lastView).toBe('list');
      expect(prefs.viewConfigs.list).toEqual({ view: 'list', layout: 'compact' });
    });

    it('uses in-memory cache on second call without re-reading localStorage', async () => {
      const stored = { lastView: 'kanban', viewConfigs: {} };
      localStorage.setItem('query-prefs-query-3', JSON.stringify(stored));

      const svc = await freshService();
      const getItemSpy = vi.spyOn(window.localStorage, 'getItem');

      svc.getPreferences('query-3'); // first call — populates cache
      svc.getPreferences('query-3'); // second call — should use cache

      // localStorage.getItem should have been called exactly once for this key
      const callsForKey = getItemSpy.mock.calls.filter(
        ([key]) => key === 'query-prefs-query-3'
      );
      expect(callsForKey.length).toBe(1);
    });

    it('returns defaults for corrupt localStorage data', async () => {
      localStorage.setItem('query-prefs-corrupt', 'not-valid-json{]');

      const svc = await freshService();
      const prefs = svc.getPreferences('corrupt');

      expect(prefs.lastView).toBe('table');
      expect(prefs.viewConfigs).toEqual({});
    });

    it('returns defaults for null stored value', async () => {
      localStorage.setItem('query-prefs-null', 'null');

      const svc = await freshService();
      const prefs = svc.getPreferences('null');

      expect(prefs.lastView).toBe('table');
    });

    it('returns defaults when lastView is invalid', async () => {
      const bad = { lastView: 'grid', viewConfigs: {} };
      localStorage.setItem('query-prefs-bad-view', JSON.stringify(bad));

      const svc = await freshService();
      const prefs = svc.getPreferences('bad-view');

      expect(prefs.lastView).toBe('table');
    });
  });

  // -------------------------------------------------------------------------
  // saveViewConfig
  // -------------------------------------------------------------------------

  describe('saveViewConfig', () => {
    it('persists to cache and localStorage', async () => {
      const svc = await freshService();
      svc.saveViewConfig('query-save', 'table');

      const raw = localStorage.getItem('query-prefs-query-save');
      expect(raw).not.toBeNull();

      const parsed = JSON.parse(raw!);
      expect(parsed.lastView).toBe('table');
    });

    it('updates lastView when switching views', async () => {
      const svc = await freshService();

      svc.saveViewConfig('query-switch', 'table');
      expect(svc.getPreferences('query-switch').lastView).toBe('table');

      svc.saveViewConfig('query-switch', 'list');
      expect(svc.getPreferences('query-switch').lastView).toBe('list');
    });

    it('merges view-specific config into existing config', async () => {
      const svc = await freshService();

      // Save initial table config
      svc.saveViewConfig('query-merge', 'table', {
        view: 'table',
        columns: [{ field: 'status', label: 'Status' }]
      });

      // Save again with additional sortBy — should merge, not replace
      svc.saveViewConfig('query-merge', 'table', {
        view: 'table',
        sortBy: { field: 'status', direction: 'asc' }
      });

      const prefs = svc.getPreferences('query-merge');
      expect(prefs.viewConfigs.table?.columns).toEqual([{ field: 'status', label: 'Status' }]);
      expect(prefs.viewConfigs.table?.sortBy).toEqual({ field: 'status', direction: 'asc' });
    });

    it('does not mutate viewConfigs when no config is provided', async () => {
      const svc = await freshService();

      // Store initial config
      svc.saveViewConfig('query-no-cfg', 'table', {
        view: 'table',
        columns: [{ field: 'id', label: 'ID' }]
      });

      // Switch to list without config — table config should be preserved
      svc.saveViewConfig('query-no-cfg', 'list');

      const prefs = svc.getPreferences('query-no-cfg');
      expect(prefs.lastView).toBe('list');
      expect(prefs.viewConfigs.table?.columns).toEqual([{ field: 'id', label: 'ID' }]);
    });

    it('stores kanban config correctly', async () => {
      const svc = await freshService();

      svc.saveViewConfig('query-kanban', 'kanban', {
        view: 'kanban',
        groupBy: 'status',
        cardLayout: 'compact'
      });

      const prefs = svc.getPreferences('query-kanban');
      expect(prefs.lastView).toBe('kanban');
      expect(prefs.viewConfigs.kanban?.groupBy).toBe('status');
    });
  });

  // -------------------------------------------------------------------------
  // clearPreferences
  // -------------------------------------------------------------------------

  describe('clearPreferences', () => {
    it('removes from both cache and localStorage', async () => {
      const svc = await freshService();

      svc.saveViewConfig('query-clear', 'table');

      // Confirm it is stored
      expect(localStorage.getItem('query-prefs-query-clear')).not.toBeNull();

      svc.clearPreferences('query-clear');

      // localStorage should be gone
      expect(localStorage.getItem('query-prefs-query-clear')).toBeNull();

      // Cache should be gone — next getPreferences will return defaults
      const prefs = svc.getPreferences('query-clear');
      expect(prefs.lastView).toBe('table');
      expect(prefs.viewConfigs).toEqual({});
    });

    it('does not throw when clearing non-existent preferences', async () => {
      const svc = await freshService();

      expect(() => svc.clearPreferences('does-not-exist')).not.toThrow();
    });
  });

  // -------------------------------------------------------------------------
  // invalidateCache
  // -------------------------------------------------------------------------

  describe('invalidateCache', () => {
    it('removes from cache but not from localStorage', async () => {
      const svc = await freshService();

      svc.saveViewConfig('query-inv', 'list');

      // Verify localStorage has the entry
      const rawBefore = localStorage.getItem('query-prefs-query-inv');
      expect(rawBefore).not.toBeNull();

      // Invalidate cache — localStorage should remain intact
      svc.invalidateCache('query-inv');

      // localStorage should still have the value
      expect(localStorage.getItem('query-prefs-query-inv')).not.toBeNull();

      // Next getPreferences re-reads from storage (not defaults)
      const prefs = svc.getPreferences('query-inv');
      expect(prefs.lastView).toBe('list');
    });

    it('causes next getPreferences to re-read localStorage', async () => {
      const svc = await freshService();

      svc.saveViewConfig('query-reread', 'table');
      svc.getPreferences('query-reread'); // populate cache

      svc.invalidateCache('query-reread');

      const getItemSpy = vi.spyOn(window.localStorage, 'getItem');
      svc.getPreferences('query-reread');

      const callsForKey = getItemSpy.mock.calls.filter(
        ([key]) => key === 'query-prefs-query-reread'
      );
      expect(callsForKey.length).toBe(1);
    });

    it('does not throw when invalidating a non-cached key', async () => {
      const svc = await freshService();

      expect(() => svc.invalidateCache('never-cached')).not.toThrow();
    });
  });

  // -------------------------------------------------------------------------
  // Multiple queries — independent preferences
  // -------------------------------------------------------------------------

  describe('multiple queries have independent preferences', () => {
    it('separate nodeIds store independent preferences', async () => {
      const svc = await freshService();

      svc.saveViewConfig('qA', 'list');
      svc.saveViewConfig('qB', 'kanban');

      expect(svc.getPreferences('qA').lastView).toBe('list');
      expect(svc.getPreferences('qB').lastView).toBe('kanban');
    });

    it('clearing one query does not affect another', async () => {
      const svc = await freshService();

      svc.saveViewConfig('qC', 'list');
      svc.saveViewConfig('qD', 'table');

      svc.clearPreferences('qC');

      expect(svc.getPreferences('qC').lastView).toBe('table'); // default
      expect(svc.getPreferences('qD').lastView).toBe('table'); // persisted
    });
  });

  // -------------------------------------------------------------------------
  // Defensive defaults — DEFAULT_QUERY_PREFERENCES constant
  // -------------------------------------------------------------------------

  describe('DEFAULT_QUERY_PREFERENCES', () => {
    it('has lastView of "table"', () => {
      expect(DEFAULT_QUERY_PREFERENCES.lastView).toBe('table');
    });

    it('has empty viewConfigs', () => {
      expect(DEFAULT_QUERY_PREFERENCES.viewConfigs).toEqual({});
    });
  });
});
