/**
 * Model Store - Manages local model catalog, downloads, and loading using Svelte 5 runes.
 *
 * Wired to real Tauri invocations for model lifecycle management.
 * Falls back to mock model data and simulated downloads when Tauri is not available.
 */

import { createLogger } from '$lib/utils/logger';
import { toError } from '$lib/types/errors';
import type {
  ModelInfo,
  ModelFamily,
  ModelStatus,
  DownloadEvent,
  ModelDownloadReadyEvent,
} from '$lib/types/agent-types';
import { AGENT_EVENTS } from '$lib/types/agent-types';
import * as tauriCommands from '$lib/services/tauri-commands';
import type { ChatModelEntry, ChatModelStatus } from '$lib/services/tauri-commands';

const log = createLogger('ModelStore');

/** Map a catalog entry's status to the richer tagged ModelStatus union. */
function toModelStatus(s: ChatModelStatus): ModelStatus {
  switch (s.status) {
    case 'downloading':
      return { status: 'downloading', progress_pct: 0, bytes_downloaded: 0, bytes_total: 0 };
    case 'error':
      return { status: 'error', message: 'Model error' };
    default:
      return { status: s.status };
  }
}

/**
 * Adapt a built-in (GGUF) `chat_model_list` catalog entry to the store's
 * `ModelInfo` shape. Callers must pre-filter to `backend === 'gguf'` (this store
 * doesn't manage remotely-served rows). The lean catalog row carries no filename/url/sha256;
 * those aren't surfaced for catalog-sourced models.
 */
function chatEntryToModelInfo(entry: ChatModelEntry): ModelInfo {
  // GGUF family isn't carried by the catalog row; infer from the id where known,
  // defaulting to the predominant built-in family.
  const family: ModelFamily = entry.id.startsWith('gemma') ? 'gemma4' : 'ministral';
  return {
    id: entry.id,
    family,
    name: entry.name,
    filename: '',
    size_bytes: entry.sizeBytes,
    quantization: entry.quantization,
    url: '',
    sha256: '',
    status: toModelStatus(entry.status),
    min_memory_gb: entry.minMemoryGb,
  };
}

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

/** Mock model catalog (used when Tauri is not available). Matches daemon catalog IDs. */
function createMockModels(): ModelInfo[] {
  return [
    {
      id: 'ministral-3b-q4km',
      family: 'ministral',
      name: 'Ministral 3B Instruct Q4_K_M',
      filename: 'Ministral-3-3B-Instruct-2512-Q4_K_M.gguf',
      size_bytes: 2_147_023_008,
      quantization: 'Q4_K_M',
      url: '',
      sha256: '',
      status: { status: 'not_downloaded' },
      min_memory_gb: 8,
    },
    {
      id: 'ministral-8b-q4km',
      family: 'ministral',
      name: 'Ministral 8B Instruct Q4_K_M',
      filename: 'Ministral-3-8B-Instruct-2512-Q4_K_M.gguf',
      size_bytes: 5_198_911_904,
      quantization: 'Q4_K_M',
      url: '',
      sha256: '',
      status: { status: 'not_downloaded' },
      min_memory_gb: 16,
    },
    {
      id: 'gemma-4-e4b-q4km',
      family: 'gemma4',
      name: 'Gemma 4 E4B Instruct Q4_K_M',
      filename: 'gemma-4-E4B-it-Q4_K_M.gguf',
      size_bytes: 5_335_289_824,
      quantization: 'Q4_K_M',
      url: '',
      sha256: '',
      status: { status: 'not_downloaded' },
      min_memory_gb: 16,
    },
    {
      id: 'gemma-4-31b-q4km',
      family: 'gemma4',
      name: 'Gemma 4 31B Instruct Q4_K_M',
      filename: 'gemma-4-31B-it-Q4_K_M.gguf',
      size_bytes: 18_687_061_792,
      quantization: 'Q4_K_M',
      url: '',
      sha256: '',
      status: { status: 'not_downloaded' },
      min_memory_gb: 24,
    },
  ];
}

class ModelStore {
  models = $state<ModelInfo[]>([]);
  downloadProgress = $state<Record<string, number>>({});
  loadedModelId = $state<string | null>(null);
  isLoading = $state(false);
  /** Total system RAM in GiB. 0 means unknown (non-Tauri or not yet loaded). */
  systemRamGb = $state(0);

  private downloadAbortControllers = new Map<string, AbortController>();
  private eventUnlisteners: Array<() => void> = [];

  /** Whether at least one model is downloaded and ready. */
  get hasDownloadedModel(): boolean {
    return this.models.some(
      (m) => m.status.status === 'ready' || m.status.status === 'loaded'
    );
  }

  /** Recommend the largest model that fits within available system RAM. */
  get recommendedModel(): ModelInfo | undefined {
    const available = this.models.filter(
      (m) => m.status.status === 'not_downloaded' || m.status.status === 'ready'
    );
    const candidates = available.length > 0 ? available : this.models;
    if (candidates.length === 0) return undefined;
    const ram = this.systemRamGb;
    const fits = ram > 0 ? candidates.filter((m) => m.min_memory_gb <= ram) : candidates;
    const pool = fits.length > 0 ? fits : candidates;
    return pool.reduce((best, m) => (m.size_bytes > best.size_bytes ? m : best));
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
        const [entries, ram] = await Promise.all([
          tauriCommands.chatModelList(),
          tauriCommands.getSystemRamGb(),
        ]);
        // This store manages the *built-in* (GGUF) download/load lifecycle; its
        // consumers (model-manager, onboarding) download by URL and recommend a
        // model to fetch. Remotely-served models have no URL here and
        // are surfaced separately by AiChatModelSelector, so exclude them — letting
        // them in would render dead download buttons and let `recommendedModel`
        // pick an un-downloadable row.
        this.models = entries
          .filter((e) => e.backend === 'gguf')
          .map(chatEntryToModelInfo);
        this.systemRamGb = ram;
        // Detect which model is loaded
        const loaded = this.models.find((m) => m.status.status === 'loaded');
        this.loadedModelId = loaded?.id ?? null;
      } else {
        await new Promise((resolve) => setTimeout(resolve, 200));
        if (this.models.length === 0) {
          this.models = createMockModels();
          this.systemRamGb = 8; // Simulate a low-RAM machine so the warning chip is visible in dev
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
    if (
      model.status.status !== 'not_downloaded' &&
      model.status.status !== 'error'
    ) {
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

      // Listen for the ready event so the UI can flip to "ready" the moment
      // the daemon confirms completion, rather than waiting on the download
      // stream to fully close before refreshModels() can run.
      const unlistenReady = await listen<ModelDownloadReadyEvent>(
        AGENT_EVENTS.MODEL_DOWNLOAD_READY,
        (event) => {
          if (event.payload.model_id === modelId) {
            const idx = this.models.findIndex((m) => m.id === modelId);
            if (idx !== -1) {
              this.updateModelStatus(idx, { status: 'ready' });
            }
            const { [modelId]: _removed, ...remaining } = this.downloadProgress;
            this.downloadProgress = remaining;
          }
        }
      );
      this.eventUnlisteners.push(unlistenReady);

      // Start the download
      await tauriCommands.chatModelDownload(modelId);

      // Refresh to reconcile with the backend's authoritative status
      // (covers any model whose ready event was missed, e.g. a listener
      // race on app startup).
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
        log.error('Failed to cancel download', { modelId, error: toError(err).message });
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
        log.error('Failed to load model via Tauri', { modelId, error: toError(err).message });
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
        log.error('Failed to unload model via Tauri', { error: toError(err).message });
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
        log.error('Failed to delete model via Tauri', { modelId, error: toError(err).message });
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
    this.systemRamGb = 0;
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
