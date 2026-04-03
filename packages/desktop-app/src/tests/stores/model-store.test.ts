/**
 * Unit tests for ModelStore - model catalog, downloads, loading
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { modelStore, formatBytes } from '$lib/stores/model-store.svelte';

// Mock the logger
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

describe('ModelStore', () => {
  beforeEach(() => {
    modelStore.reset();
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
        expect(model.family).toBe('ministral');
        expect(model.size_bytes).toBeGreaterThan(0);
        expect(model.quantization).toBeTruthy();
        expect(model.status).toBeDefined();
        expect(model.status.status).toBe('not_downloaded');
      }
    });
  });

  describe('recommendedModel', () => {
    it('recommends the smallest model', async () => {
      const promise = modelStore.refreshModels();
      await vi.runAllTimersAsync();
      await promise;

      const recommended = modelStore.recommendedModel;
      expect(recommended).toBeDefined();

      // Should be the smallest by size_bytes
      const smallest = modelStore.models.reduce((min, m) =>
        m.size_bytes < min.size_bytes ? m : min
      );
      expect(recommended!.id).toBe(smallest.id);
    });

    it('returns undefined when no models', () => {
      expect(modelStore.recommendedModel).toBeUndefined();
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
