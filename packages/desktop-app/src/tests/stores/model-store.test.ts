/**
 * Unit tests for ModelStore - model catalog, downloads, loading
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { modelStore, formatBytes } from '$lib/stores/model-store.svelte';
import * as tauriCommands from '$lib/services/tauri-commands';

// Mock the logger
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

interface MockTauriEvent<T = unknown> {
  payload: T;
}

const mockEventListeners = new Map<string, (event: MockTauriEvent) => void>();

function emitTauriEvent(eventName: string, payload: unknown) {
  const handler = mockEventListeners.get(eventName);
  if (handler) {
    handler({ payload });
  }
}

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (eventName: string, handler: (event: MockTauriEvent) => void) => {
    mockEventListeners.set(eventName, handler);
    return () => {
      mockEventListeners.delete(eventName);
    };
  }),
}));

function mockTauriEnvironment(isTauri: boolean) {
  interface WindowWithTauri extends Window {
    __TAURI__?: Record<string, unknown>;
    __TAURI_INTERNALS__?: Record<string, unknown>;
  }
  if (isTauri) {
    (window as WindowWithTauri).__TAURI_INTERNALS__ = {};
  } else {
    // Clear BOTH markers `isTauri()` checks. Under vitest's `forks` pool the
    // environment is reused across test files, so another file that set either
    // marker (and didn't clear it) would otherwise leak into these mock-path
    // tests and flip them onto the Tauri branch — the cause of the parallel-suite
    // flakiness where refreshModels()/downloadModel() take the unmocked Tauri path.
    delete (window as WindowWithTauri).__TAURI__;
    delete (window as WindowWithTauri).__TAURI_INTERNALS__;
  }
}

describe('ModelStore', () => {
  beforeEach(() => {
    modelStore.reset();
    // Establish the non-Tauri (mock) environment deterministically so these tests
    // don't depend on the absence of Tauri globals a prior test file may have left
    // on the shared window. The Tauri sub-block below opts back into Tauri mode.
    mockTauriEnvironment(false);
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('Initial State', () => {
    it('starts with empty models', () => {
      expect(modelStore.models).toEqual([]);
    });

    it('starts with no download progress', () => {
      expect(modelStore.downloadProgress).toEqual({});
    });

    it('starts with no loaded model', () => {
      expect(modelStore.loadedModelId).toBeNull();
    });

    it('reports no downloaded models', () => {
      expect(modelStore.hasDownloadedModel).toBe(false);
    });
  });

  describe('refreshModels', () => {
    it('loads mock model catalog', async () => {
      const promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      expect(modelStore.models.length).toBeGreaterThan(0);
    });

    it('sets loading state during refresh', async () => {
      const promise = modelStore.refreshModels();
      expect(modelStore.isLoading).toBe(true);

      await vi.runAllTimersAsync();
      await promise;

      expect(modelStore.isLoading).toBe(false);
    });

    it('models have required fields', async () => {
      const promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      for (const model of modelStore.models) {
        expect(model.id).toBeTruthy();
        expect(model.name).toBeTruthy();
        expect(model.family).toBeTruthy();
        expect(model.size_bytes).toBeGreaterThan(0);
        expect(model.quantization).toBeTruthy();
        expect(model.status).toBeDefined();
        expect(model.status.status).toBe('not_downloaded');
        expect(model.min_memory_gb).toBeGreaterThan(0);
      }
    });
  });

  describe('downloadModel', () => {
    it('simulates download with progress', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;
      promise = modelStore.downloadModel(modelId);

      // Advance some time to see progress
      await vi.advanceTimersByTimeAsync(500);

      // Should have progress in flight
      expect(modelStore.downloadProgress[modelId]).toBeDefined();

      await vi.runAllTimersAsync();
      await promise;

      // After complete, model should be ready
      const model = modelStore.models.find((m) => m.id === modelId);
      expect(model!.status.status).toBe('ready');
    });

    it('clears progress after download completes', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      expect(modelStore.downloadProgress[modelId]).toBeUndefined();
    });

    it('ignores download of already-downloaded model', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;

      // Download once
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;
      expect(modelStore.models.find((m) => m.id === modelId)!.status.status).toBe('ready');

      // Try to download again — should be a no-op
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;
      expect(modelStore.models.find((m) => m.id === modelId)!.status.status).toBe('ready');
    });

    it('sets hasDownloadedModel after download', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      expect(modelStore.hasDownloadedModel).toBe(false);

      const modelId = modelStore.models[0].id;
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      expect(modelStore.hasDownloadedModel).toBe(true);
    });
  });

  describe('downloadModel via Tauri (download-ready event)', () => {
    beforeEach(() => {
      mockTauriEnvironment(true);
      mockEventListeners.clear();
    });

    afterEach(() => {
      mockTauriEnvironment(false);
    });

    it('transitions status to ready as soon as the download-ready event fires, without waiting on chatModelDownload to resolve', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;

      // Simulate the daemon hanging: chatModelDownload's
      // returned promise never resolves because a stale progress-callback
      // sender keeps the gRPC stream open. The regression fix is that the
      // ready event alone -- independent of that promise -- must flip the
      // status to "ready".
      let resolveDownload: () => void = () => {};
      vi.spyOn(tauriCommands, 'chatModelDownload').mockImplementation(
        () =>
          new Promise<void>((resolve) => {
            resolveDownload = resolve;
          })
      );

      promise = modelStore.downloadModel(modelId);
      await vi.waitFor(() => {
        expect(mockEventListeners.has('model://download-ready')).toBe(true);
      });

      emitTauriEvent('model://download-ready', { model_id: modelId });

      const model = modelStore.models.find((m) => m.id === modelId);
      expect(model!.status.status).toBe('ready');

      // Clean up the still-pending downloadModel() call so it doesn't leak
      // into other tests.
      resolveDownload();
      vi.spyOn(tauriCommands, 'chatModelList').mockResolvedValue([]);
      await promise;
    });

    it('hides the Cancel affordance once status flips to ready via the download-ready event', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;

      vi.spyOn(tauriCommands, 'chatModelDownload').mockImplementation(
        () => new Promise<void>(() => {})
      );
      vi.spyOn(tauriCommands, 'chatModelList').mockResolvedValue([]);

      void modelStore.downloadModel(modelId);
      await vi.waitFor(() => {
        expect(mockEventListeners.has('model://download-ready')).toBe(true);
      });

      expect(modelStore.models.find((m) => m.id === modelId)!.status.status).toBe('downloading');

      emitTauriEvent('model://download-ready', { model_id: modelId });

      // The Cancel button in model-manager.svelte is rendered purely from
      // `status.status === 'downloading'`, so once status flips to "ready"
      // the button is gone without any separate UI-side fix.
      expect(modelStore.models.find((m) => m.id === modelId)!.status.status).toBe('ready');
    });
  });

  describe('cancelDownload', () => {
    it('cancels an in-progress download', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;
      promise = modelStore.downloadModel(modelId);

      // Let it start
      await vi.advanceTimersByTimeAsync(200);

      // Cancel
      modelStore.cancelDownload(modelId);
      await vi.runAllTimersAsync();
      await promise;

      // Model should be back to not_downloaded
      const model = modelStore.models.find((m) => m.id === modelId);
      expect(model!.status.status).toBe('not_downloaded');
    });
  });

  describe('loadModel', () => {
    it('loads a ready model', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;

      // Download first
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      // Load
      promise = modelStore.loadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      expect(modelStore.loadedModelId).toBe(modelId);
      const model = modelStore.models.find((m) => m.id === modelId);
      expect(model!.status.status).toBe('loaded');
    });

    it('unloads previously loaded model when loading new one', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      // Download two models
      const id1 = modelStore.models[0].id;
      const id2 = modelStore.models[1].id;

      promise = modelStore.downloadModel(id1);
      await vi.runAllTimersAsync();
      await promise;

      promise = modelStore.downloadModel(id2);
      await vi.runAllTimersAsync();
      await promise;

      // Load first
      promise = modelStore.loadModel(id1);
      await vi.runAllTimersAsync();
      await promise;
      expect(modelStore.loadedModelId).toBe(id1);

      // Load second - first should be unloaded
      promise = modelStore.loadModel(id2);
      await vi.runAllTimersAsync();
      await promise;
      expect(modelStore.loadedModelId).toBe(id2);

      const model1 = modelStore.models.find((m) => m.id === id1);
      expect(model1!.status.status).toBe('ready');
    });
  });

  describe('unloadModel', () => {
    it('unloads the current model', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      promise = modelStore.loadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;
      expect(modelStore.loadedModelId).toBe(modelId);

      promise = modelStore.unloadModel();
      await vi.runAllTimersAsync();
      await promise;

      expect(modelStore.loadedModelId).toBeNull();
      const model = modelStore.models.find((m) => m.id === modelId);
      expect(model!.status.status).toBe('ready');
    });

    it('is a no-op when no model loaded', async () => {
      await modelStore.unloadModel();
      expect(modelStore.loadedModelId).toBeNull();
    });
  });

  describe('deleteModel', () => {
    it('deletes a downloaded model', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      promise = modelStore.deleteModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      const model = modelStore.models.find((m) => m.id === modelId);
      expect(model!.status.status).toBe('not_downloaded');
    });

    it('unloads a loaded model before deleting', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      promise = modelStore.loadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;
      expect(modelStore.loadedModelId).toBe(modelId);

      promise = modelStore.deleteModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      expect(modelStore.loadedModelId).toBeNull();
      const model = modelStore.models.find((m) => m.id === modelId);
      expect(model!.status.status).toBe('not_downloaded');
    });
  });

  describe('reset', () => {
    it('resets all state', async () => {
      let promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const modelId = modelStore.models[0].id;
      promise = modelStore.downloadModel(modelId);
      await vi.runAllTimersAsync();
      await promise;

      modelStore.reset();

      expect(modelStore.models).toEqual([]);
      expect(modelStore.downloadProgress).toEqual({});
      expect(modelStore.loadedModelId).toBeNull();
      expect(modelStore.isLoading).toBe(false);
    });
  });
});

describe('formatBytes', () => {
  it('formats bytes', () => {
    expect(formatBytes(500)).toBe('500 B');
  });

  it('formats kilobytes', () => {
    expect(formatBytes(1536)).toBe('1.5 KB');
  });

  it('formats megabytes', () => {
    expect(formatBytes(1_572_864)).toBe('1.5 MB');
  });

  it('formats gigabytes', () => {
    expect(formatBytes(4_920_000_000)).toBe('4.6 GB');
  });
});
