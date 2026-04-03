/**
 * Model Store - Manages local model catalog, downloads, and loading using Svelte 5 runes.
 *
 * Provides mock model data and simulated downloads for development.
 * Real Tauri integration will be wired in #1008.
 */

import { createLogger } from '$lib/utils/logger';
import type { ModelInfo, ModelStatus } from '$lib/types/agent-types';

const log = createLogger('ModelStore');

/** Mock model catalog. */
function createMockModels(): ModelInfo[] {
  return [
    {
      id: 'ministral-8b-q4',
      family: 'ministral',
      name: 'Ministral 8B (Q4)',
      filename: 'ministral-8b-instruct-2410-q4_k_m.gguf',
      size_bytes: 4_920_000_000,
      quantization: 'Q4_K_M',
      url: 'https://huggingface.co/ministral/ministral-8b-instruct-2410-GGUF',
      sha256: 'abc123',
      status: { status: 'not_downloaded' },
    },
    {
      id: 'ministral-3b-q4',
      family: 'ministral',
      name: 'Ministral 3B (Q4)',
      filename: 'ministral-3b-instruct-q4_k_m.gguf',
      size_bytes: 1_800_000_000,
      quantization: 'Q4_K_M',
      url: 'https://huggingface.co/ministral/ministral-3b-instruct-GGUF',
      sha256: 'def456',
      status: { status: 'not_downloaded' },
    },
    {
      id: 'ministral-8b-q8',
      family: 'ministral',
      name: 'Ministral 8B (Q8)',
      filename: 'ministral-8b-instruct-2410-q8_0.gguf',
      size_bytes: 8_540_000_000,
      quantization: 'Q8_0',
      url: 'https://huggingface.co/ministral/ministral-8b-instruct-2410-GGUF',
      sha256: 'ghi789',
      status: { status: 'not_downloaded' },
    },
  ];
}

/** Format bytes into human-readable string. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

class ModelStore {
  models = $state<ModelInfo[]>([]);
  downloadProgress = $state<Record<string, number>>({});
  loadedModelId = $state<string | null>(null);
  isLoading = $state(false);

  private downloadAbortControllers = new Map<string, AbortController>();

  /** Whether at least one model is downloaded and ready. */
  get hasDownloadedModel(): boolean {
    return this.models.some(
      (m) => m.status.status === 'ready' || m.status.status === 'loaded'
    );
  }

  /** Recommend the best model based on available RAM (mock: always recommend smallest). */
  get recommendedModel(): ModelInfo | undefined {
    // In real implementation, this would check system RAM via Tauri command.
    // For mock: recommend the smallest model.
    const available = this.models.filter(
      (m) => m.status.status === 'not_downloaded' || m.status.status === 'ready'
    );
    if (available.length === 0) return this.models[0];
    return available.reduce((smallest, m) =>
      m.size_bytes < smallest.size_bytes ? m : smallest
    );
  }

  /** The currently loaded model. */
  get loadedModel(): ModelInfo | undefined {
    if (!this.loadedModelId) return undefined;
    return this.models.find((m) => m.id === this.loadedModelId);
  }

  /** Refresh model catalog from backend (mock). */
  async refreshModels(): Promise<void> {
    this.isLoading = true;
    try {
      await new Promise((resolve) => setTimeout(resolve, 200));
      if (this.models.length === 0) {
        this.models = createMockModels();
      }
      log.info('Models refreshed', { count: this.models.length });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to refresh models';
      log.error('Failed to refresh models', { error: message });
    } finally {
      this.isLoading = false;
    }
  }

  /** Simulate downloading a model with progress updates. */
  async downloadModel(modelId: string): Promise<void> {
    const modelIndex = this.models.findIndex((m) => m.id === modelId);
    if (modelIndex === -1) {
      log.warn('Model not found for download', { modelId });
      return;
    }

    const model = this.models[modelIndex];
    if (model.status.status !== 'not_downloaded') {
      log.warn('Model already downloaded or downloading', { modelId, status: model.status.status });
      return;
    }

    const abortController = new AbortController();
    this.downloadAbortControllers.set(modelId, abortController);

    try {
      // Set downloading status
      this.updateModelStatus(modelIndex, {
        status: 'downloading',
        progress_pct: 0,
        bytes_downloaded: 0,
        bytes_total: model.size_bytes,
      });

      const totalBytes = model.size_bytes;
      const steps = 20;
      const bytesPerStep = totalBytes / steps;

      for (let i = 1; i <= steps; i++) {
        if (abortController.signal.aborted) break;

        await new Promise<void>((resolve, reject) => {
          const timeout = setTimeout(resolve, 100 + Math.random() * 50);
          abortController.signal.addEventListener('abort', () => {
            clearTimeout(timeout);
            reject(new Error('aborted'));
          }, { once: true });
        });

        const bytesDownloaded = Math.min(bytesPerStep * i, totalBytes);
        const progressPct = (bytesDownloaded / totalBytes) * 100;

        this.downloadProgress = { ...this.downloadProgress, [modelId]: progressPct };
        this.updateModelStatus(modelIndex, {
          status: 'downloading',
          progress_pct: progressPct,
          bytes_downloaded: bytesDownloaded,
          bytes_total: totalBytes,
        });
      }

      // Verify
      this.updateModelStatus(modelIndex, { status: 'verifying' });
      await new Promise((resolve) => setTimeout(resolve, 300));

      // Ready
      this.updateModelStatus(modelIndex, { status: 'ready' });
      const { [modelId]: _removed, ...remaining } = this.downloadProgress;
      this.downloadProgress = remaining;

      log.info('Model download complete', { modelId });
    } catch (err) {
      if (err instanceof Error && err.message === 'aborted') {
        log.info('Download cancelled', { modelId });
        this.updateModelStatus(modelIndex, { status: 'not_downloaded' });
      } else {
        const message = err instanceof Error ? err.message : 'Download failed';
        log.error('Download error', { modelId, error: message });
        this.updateModelStatus(modelIndex, { status: 'error', message });
      }
      const { [modelId]: _removed, ...remaining } = this.downloadProgress;
      this.downloadProgress = remaining;
    } finally {
      this.downloadAbortControllers.delete(modelId);
    }
  }

  /** Cancel an in-progress download. */
  cancelDownload(modelId: string): void {
    const controller = this.downloadAbortControllers.get(modelId);
    if (controller) {
      controller.abort();
    }
  }

  /** Load a downloaded model into memory (mock). */
  async loadModel(modelId: string): Promise<void> {
    const model = this.models.find((m) => m.id === modelId);
    if (!model) {
      log.warn('Model not found for loading', { modelId });
      return;
    }
    if (model.status.status !== 'ready') {
      log.warn('Model not ready for loading', { modelId, status: model.status.status });
      return;
    }

    // Unload current model if any
    if (this.loadedModelId) {
      await this.unloadModel();
    }

    // Simulate loading delay
    await new Promise((resolve) => setTimeout(resolve, 500));

    const modelIndex = this.models.findIndex((m) => m.id === modelId);
    this.updateModelStatus(modelIndex, { status: 'loaded' });
    this.loadedModelId = modelId;
    log.info('Model loaded', { modelId });
  }

  /** Unload the current model from memory (mock). */
  async unloadModel(): Promise<void> {
    if (!this.loadedModelId) return;

    const modelIndex = this.models.findIndex((m) => m.id === this.loadedModelId);
    if (modelIndex !== -1) {
      this.updateModelStatus(modelIndex, { status: 'ready' });
    }

    log.info('Model unloaded', { modelId: this.loadedModelId });
    this.loadedModelId = null;
  }

  /** Delete a downloaded model (mock). */
  async deleteModel(modelId: string): Promise<void> {
    const modelIndex = this.models.findIndex((m) => m.id === modelId);
    if (modelIndex === -1) return;

    if (this.loadedModelId === modelId) {
      await this.unloadModel();
    }

    this.updateModelStatus(modelIndex, { status: 'not_downloaded' });
    log.info('Model deleted', { modelId });
  }

  /** Reset to initial state. */
  reset(): void {
    for (const controller of this.downloadAbortControllers.values()) {
      controller.abort();
    }
    this.downloadAbortControllers.clear();
    this.models = [];
    this.downloadProgress = {};
    this.loadedModelId = null;
    this.isLoading = false;
  }

  /** Internal helper to update a model's status immutably. */
  private updateModelStatus(index: number, status: ModelStatus): void {
    if (index < 0 || index >= this.models.length) return;
    this.models = this.models.map((m, i) =>
      i === index ? { ...m, status } : m
    );
  }
}

export const modelStore = new ModelStore();
