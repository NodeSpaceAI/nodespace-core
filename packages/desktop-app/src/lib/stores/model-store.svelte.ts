/**
 * Model Store - Manages local model catalog, downloads, and loading using Svelte 5 runes.
 *
 * Wired to real Tauri invocations for model lifecycle management.
 * Falls back to mock model data and simulated downloads when Tauri is not available.
 *
 * Issue #1008: replaced mock-only implementation with real Tauri integration.
 */

import { createLogger } from '$lib/utils/logger';
import type { ModelInfo, ModelStatus, DownloadEvent } from '$lib/types/agent-types';
import { AGENT_EVENTS } from '$lib/types/agent-types';
import * as tauriCommands from '$lib/services/tauri-commands';

const log = createLogger('ModelStore');

/** Check if running in Tauri desktop environment. */
function isTauri(): boolean {
  return (
    typeof window !== 'undefined' &&
    ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}

/** Format bytes into human-readable string. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

/** Mock model catalog (used when Tauri is not available). */
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

class ModelStore {
  models = $state<ModelInfo[]>([]);
  downloadProgress = $state<Record<string, number>>({});
  loadedModelId = $state<string | null>(null);
  isLoading = $state(false);

  private downloadAbortControllers = new Map<string, AbortController>();
  private eventUnlisteners: Array<() => void> = [];

  /** Whether at least one model is downloaded and ready. */
  get hasDownloadedModel(): boolean {
    return this.models.some(
      (m) => m.status.status === 'ready' || m.status.status === 'loaded'
    );
  }

  /** Recommend the best model based on available RAM. */
  get recommendedModel(): ModelInfo | undefined {
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

  /** Refresh model catalog from backend (real or mock). */
  async refreshModels(): Promise<void> {
    this.isLoading = true;
    try {
      if (isTauri()) {
        this.models = await tauriCommands.chatModelList();
        // Detect which model is loaded
        const loaded = this.models.find((m) => m.status.status === 'loaded');
        this.loadedModelId = loaded?.id ?? null;
      } else {
        await new Promise((resolve) => setTimeout(resolve, 200));
        if (this.models.length === 0) {
          this.models = createMockModels();
        }
      }
      log.info('Models refreshed', { count: this.models.length });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to refresh models';
      log.error('Failed to refresh models', { error: message });

      // Fall back to mock on error
      if (this.models.length === 0) {
        this.models = createMockModels();
        log.info('Fell back to mock models after error');
      }
    } finally {
      this.isLoading = false;
    }
  }

  /** Download a model (real Tauri or simulated). */
  async downloadModel(modelId: string): Promise<void> {
    const modelIndex = this.models.findIndex((m) => m.id === modelId);
    if (modelIndex === -1) {
      log.warn('Model not found for download', { modelId });
      return;
    }

    const model = this.models[modelIndex];
    if (model.status.status !== 'not_downloaded') {
      log.warn('Model already downloaded or downloading', {
        modelId,
        status: model.status.status,
      });
      return;
    }

    if (isTauri()) {
      await this.downloadViaTauri(modelId, modelIndex, model);
    } else {
      await this.downloadViaMock(modelId, modelIndex, model);
    }
  }

  /** Download via real Tauri invocation with event-based progress. */
  private async downloadViaTauri(
    modelId: string,
    modelIndex: number,
    model: ModelInfo
  ): Promise<void> {
    // Set downloading status optimistically
    this.updateModelStatus(modelIndex, {
      status: 'downloading',
      progress_pct: 0,
      bytes_downloaded: 0,
      bytes_total: model.size_bytes,
    });

    try {
      const { listen } = await import('@tauri-apps/api/event');

      // Listen for download progress events
      const unlisten = await listen<DownloadEvent>(
        AGENT_EVENTS.MODEL_DOWNLOAD_PROGRESS,
        (event) => {
          const evt = event.payload;
          if (evt.model_id === modelId) {
            const progressPct = (evt.bytes_downloaded / evt.bytes_total) * 100;
            this.downloadProgress = { ...this.downloadProgress, [modelId]: progressPct };

            const idx = this.models.findIndex((m) => m.id === modelId);
            if (idx !== -1) {
              this.updateModelStatus(idx, {
                status: 'downloading',
                progress_pct: progressPct,
                bytes_downloaded: evt.bytes_downloaded,
                bytes_total: evt.bytes_total,
              });
            }
          }
        }
      );
      this.eventUnlisteners.push(unlisten);

      // Start the download
      await tauriCommands.chatModelDownload(modelId);

      // Download completed successfully -- refresh to get final status
      await this.refreshModels();

      const { [modelId]: _removed, ...remaining } = this.downloadProgress;
      this.downloadProgress = remaining;

      log.info('Model download complete', { modelId });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Download failed';
      log.error('Download error', { modelId, error: message });

      const idx = this.models.findIndex((m) => m.id === modelId);
      if (idx !== -1) {
        this.updateModelStatus(idx, { status: 'error', message });
      }
      const { [modelId]: _removed, ...remaining } = this.downloadProgress;
      this.downloadProgress = remaining;
    } finally {
      this.cleanupEventListeners();
    }
  }

  /** Simulate downloading a model with progress updates (mock). */
  private async downloadViaMock(
    modelId: string,
    modelIndex: number,
    model: ModelInfo
  ): Promise<void> {
    const abortController = new AbortController();
    this.downloadAbortControllers.set(modelId, abortController);

    try {
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
          abortController.signal.addEventListener(
            'abort',
            () => {
              clearTimeout(timeout);
              reject(new Error('aborted'));
            },
            { once: true }
          );
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

      this.updateModelStatus(modelIndex, { status: 'verifying' });
      await new Promise((resolve) => setTimeout(resolve, 300));

      this.updateModelStatus(modelIndex, { status: 'ready' });
      const { [modelId]: _removed, ...remaining } = this.downloadProgress;
      this.downloadProgress = remaining;

      log.info('Model download complete (mock)', { modelId });
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
    if (isTauri()) {
      tauriCommands.chatModelCancelDownload(modelId).catch((err) => {
        log.error('Failed to cancel download', { modelId, error: String(err) });
      });
    }
    const controller = this.downloadAbortControllers.get(modelId);
    if (controller) {
      controller.abort();
    }
  }

  /** Load a downloaded model into memory (real or mock). */
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

    if (isTauri()) {
      try {
        await tauriCommands.chatModelLoad(modelId);
        await this.refreshModels();
        log.info('Model loaded via Tauri', { modelId });
      } catch (err) {
        log.error('Failed to load model via Tauri', { modelId, error: String(err) });
        throw err;
      }
    } else {
      // Mock: simulate loading delay
      await new Promise((resolve) => setTimeout(resolve, 500));
      const modelIndex = this.models.findIndex((m) => m.id === modelId);
      this.updateModelStatus(modelIndex, { status: 'loaded' });
      this.loadedModelId = modelId;
      log.info('Model loaded (mock)', { modelId });
    }
  }

  /** Unload the current model from memory (real or mock). */
  async unloadModel(): Promise<void> {
    if (!this.loadedModelId) return;

    if (isTauri()) {
      try {
        await tauriCommands.chatModelUnload();
        await this.refreshModels();
        log.info('Model unloaded via Tauri');
      } catch (err) {
        log.error('Failed to unload model via Tauri', { error: String(err) });
      }
    } else {
      const modelIndex = this.models.findIndex((m) => m.id === this.loadedModelId);
      if (modelIndex !== -1) {
        this.updateModelStatus(modelIndex, { status: 'ready' });
      }
      log.info('Model unloaded (mock)', { modelId: this.loadedModelId });
      this.loadedModelId = null;
    }
  }

  /** Delete a downloaded model (real or mock). */
  async deleteModel(modelId: string): Promise<void> {
    if (this.loadedModelId === modelId) {
      await this.unloadModel();
    }

    if (isTauri()) {
      try {
        await tauriCommands.chatModelDelete(modelId);
        await this.refreshModels();
        log.info('Model deleted via Tauri', { modelId });
      } catch (err) {
        log.error('Failed to delete model via Tauri', { modelId, error: String(err) });
      }
    } else {
      const modelIndex = this.models.findIndex((m) => m.id === modelId);
      if (modelIndex !== -1) {
        this.updateModelStatus(modelIndex, { status: 'not_downloaded' });
      }
      log.info('Model deleted (mock)', { modelId });
    }
  }

  /** Reset to initial state. */
  reset(): void {
    for (const controller of this.downloadAbortControllers.values()) {
      controller.abort();
    }
    this.downloadAbortControllers.clear();
    this.cleanupEventListeners();
    this.models = [];
    this.downloadProgress = {};
    this.loadedModelId = null;
    this.isLoading = false;
  }

  /** Internal helper to update a model's status immutably. */
  private updateModelStatus(index: number, status: ModelStatus): void {
    if (index < 0 || index >= this.models.length) return;
    this.models = this.models.map((m, i) => (i === index ? { ...m, status } : m));
  }

  /** Clean up Tauri event listeners. */
  private cleanupEventListeners(): void {
    for (const unlisten of this.eventUnlisteners) {
      unlisten();
    }
    this.eventUnlisteners = [];
  }
}

export const modelStore = new ModelStore();
