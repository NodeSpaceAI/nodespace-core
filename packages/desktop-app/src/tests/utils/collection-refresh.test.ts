import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const {
  mockLoadCollections,
  mockLoadMembers,
  mockLoadSchemas,
  mockLoadAiChats,
  mockLoadSavedQueries,
  mockCollectionsState
} = vi.hoisted(() => {
  const mockLoadCollections = vi.fn().mockResolvedValue(undefined);
  const mockLoadMembers = vi.fn().mockResolvedValue(undefined);
  const mockLoadSchemas = vi.fn().mockResolvedValue(undefined);
  const mockLoadAiChats = vi.fn().mockResolvedValue(undefined);
  const mockLoadSavedQueries = vi.fn().mockResolvedValue(undefined);

  // Minimal rune-store-like mock: exposes a reactive-style `state` field.
  // `set` is a test helper to configure that field.
  const mockCollectionsState = {
    state: { selectedCollectionId: null as string | null },
    set(newValue: { selectedCollectionId: string | null }) {
      this.state = newValue;
    }
  };

  return {
    mockLoadCollections,
    mockLoadMembers,
    mockLoadSchemas,
    mockLoadAiChats,
    mockLoadSavedQueries,
    mockCollectionsState
  };
});

vi.mock('$lib/stores/collections.svelte', () => ({
  collectionsData: {
    loadCollections: (...args: unknown[]) => mockLoadCollections(...args),
    loadMembers: (...args: unknown[]) => mockLoadMembers(...args)
  },
  collectionsState: mockCollectionsState
}));

vi.mock('$lib/stores/schemas.svelte', () => ({
  schemasData: {
    loadSchemas: (...args: unknown[]) => mockLoadSchemas(...args)
  }
}));

vi.mock('$lib/stores/saved-queries.svelte', () => ({
  savedQueriesData: {
    loadSavedQueries: (...args: unknown[]) => mockLoadSavedQueries(...args)
  }
}));

vi.mock('$lib/stores/ai-chats.svelte', () => ({
  aiChatsData: {
    loadAiChats: (...args: unknown[]) => mockLoadAiChats(...args)
  }
}));

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

import {
  scheduleCollectionRefresh,
  clearCollectionRefreshTimer,
  scheduleSchemaRefresh,
  clearSchemaRefreshTimer,
  scheduleAiChatRefresh,
  clearAiChatRefreshTimer,
  scheduleSavedQueryRefresh,
  clearSavedQueryRefreshTimer
} from '$lib/utils/collection-refresh';

describe('Collection Refresh', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    clearCollectionRefreshTimer();
    mockCollectionsState.set({ selectedCollectionId: null });
  });

  afterEach(() => {
    clearCollectionRefreshTimer();
    vi.useRealTimers();
  });

  describe('scheduleCollectionRefresh', () => {
    it('should refresh collections after debounce delay', async () => {
      scheduleCollectionRefresh();

      expect(mockLoadCollections).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadCollections).toHaveBeenCalledTimes(1);
    });

    it('should debounce multiple calls', async () => {
      scheduleCollectionRefresh();
      scheduleCollectionRefresh();
      scheduleCollectionRefresh();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadCollections).toHaveBeenCalledTimes(1);
    });

    it('should refresh members if affected collection is selected', async () => {
      mockCollectionsState.set({ selectedCollectionId: 'col-1' });

      scheduleCollectionRefresh('col-1');
      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadCollections).toHaveBeenCalledTimes(1);
      expect(mockLoadMembers).toHaveBeenCalledWith('col-1');
    });

    it('should not refresh members if different collection is selected', async () => {
      mockCollectionsState.set({ selectedCollectionId: 'col-2' });

      scheduleCollectionRefresh('col-1');
      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadCollections).toHaveBeenCalledTimes(1);
      expect(mockLoadMembers).not.toHaveBeenCalled();
    });

    it('should not refresh members if no collection is selected', async () => {
      scheduleCollectionRefresh('col-1');
      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadCollections).toHaveBeenCalledTimes(1);
      expect(mockLoadMembers).not.toHaveBeenCalled();
    });
  });

  describe('clearCollectionRefreshTimer', () => {
    it('should cancel pending refresh', async () => {
      scheduleCollectionRefresh();
      clearCollectionRefreshTimer();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadCollections).not.toHaveBeenCalled();
    });

    it('should be safe to call when no timer is pending', () => {
      expect(() => clearCollectionRefreshTimer()).not.toThrow();
    });
  });

  describe('scheduleSchemaRefresh', () => {
    afterEach(() => {
      clearSchemaRefreshTimer();
    });

    it('should refresh schemas after debounce delay', async () => {
      scheduleSchemaRefresh();

      expect(mockLoadSchemas).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadSchemas).toHaveBeenCalledTimes(1);
    });

    it('should debounce multiple calls, resetting the timer', async () => {
      scheduleSchemaRefresh();
      scheduleSchemaRefresh();
      scheduleSchemaRefresh();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadSchemas).toHaveBeenCalledTimes(1);
    });
  });

  describe('clearSchemaRefreshTimer', () => {
    it('should cancel a pending schema refresh', async () => {
      scheduleSchemaRefresh();
      clearSchemaRefreshTimer();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadSchemas).not.toHaveBeenCalled();
    });

    it('should be safe to call when no timer is pending', () => {
      expect(() => clearSchemaRefreshTimer()).not.toThrow();
    });
  });

  // scheduleAiChatRefresh is the counterpart to scheduleCollectionRefresh/
  // scheduleSchemaRefresh above: without it, the AI Chats sidebar list has no
  // node:created/node:updated wiring at all.
  describe('scheduleAiChatRefresh', () => {
    afterEach(() => {
      clearAiChatRefreshTimer();
    });

    it('should refresh AI chats after debounce delay', async () => {
      scheduleAiChatRefresh();

      expect(mockLoadAiChats).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadAiChats).toHaveBeenCalledTimes(1);
    });

    it('should debounce multiple calls, resetting the timer', async () => {
      scheduleAiChatRefresh();
      scheduleAiChatRefresh();
      scheduleAiChatRefresh();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadAiChats).toHaveBeenCalledTimes(1);
    });
  });

  describe('scheduleSavedQueryRefresh', () => {
    afterEach(() => {
      clearSavedQueryRefreshTimer();
    });

    it('refreshes saved queries once after the debounce delay, coalescing bursts', async () => {
      scheduleSavedQueryRefresh();
      scheduleSavedQueryRefresh();
      scheduleSavedQueryRefresh();

      expect(mockLoadSavedQueries).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadSavedQueries).toHaveBeenCalledTimes(1);
    });

    it('clearSavedQueryRefreshTimer cancels a pending refresh', async () => {
      scheduleSavedQueryRefresh();
      clearSavedQueryRefreshTimer();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadSavedQueries).not.toHaveBeenCalled();
    });
  });

  describe('clearAiChatRefreshTimer', () => {
    it('should cancel a pending AI chats refresh', async () => {
      scheduleAiChatRefresh();
      clearAiChatRefreshTimer();

      await vi.advanceTimersByTimeAsync(300);

      expect(mockLoadAiChats).not.toHaveBeenCalled();
    });

    it('should be safe to call when no timer is pending', () => {
      expect(() => clearAiChatRefreshTimer()).not.toThrow();
    });
  });
});
