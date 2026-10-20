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
  return {
    id: entry.id,
    // Every GGUF catalog model is Gemma 4; the lean catalog row doesn't carry it.
    family: 'gemma4',
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

/** "1.2 of 4.5 GB": the downloaded amount in the unit of the total. */
export function formatDownloadSize(downloaded: number, total: number): string {
  const units: Array<[string, number]> = [
    ['GB', 1024 ** 3],
    ['MB', 1024 ** 2],
    ['KB', 1024],
  ];
  const [unit, size] = units.find(([, bytes]) => total >= bytes) ?? ['B', 1];
  const fmt = (n: number) => (unit === 'B' ? `${n}` : (n / size).toFixed(1));
  return `${fmt(downloaded)} of ${fmt(total)} ${unit}`;
}

/** Overlay text for a model preparation: the label and an optional explanatory line. */
export function describePreparation(
  prep: ModelPreparation,
  modelName: string
): { label: string; sublabel?: string } {
  switch (prep.phase) {
    case 'downloading': {
      if (prep.bytesTotal <= 0) return { label: `Downloading ${modelName}…` };
      const pct = Math.min(100, Math.floor((prep.bytesDownloaded / prep.bytesTotal) * 100));
      return {
        label: `Downloading ${modelName}… ${pct}% (${formatDownloadSize(prep.bytesDownloaded, prep.bytesTotal)})`,
      };
    }
    case 'verifying':
      return {
        label: 'Verifying model integrity…',
        sublabel: "This can take a few minutes on a large model's first check.",
      };
    case 'loading':
      return { label: 'Loading model…' };
    default:
      return { label: 'Preparing model…' };
  }
}

/** Mock model catalog (used when Tauri is not available). Matches daemon catalog IDs. */
function createMockModels(): ModelInfo[] {
  return [
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
      id: 'gemma-4-26b-a4b-q8',
      family: 'gemma4',
      name: 'Gemma 4 26B-A4B Instruct Q8_0',
      filename: 'gemma-4-26B-A4B-it-Q8_0.gguf',
      size_bytes: 26_859_860_992,
      quantization: 'Q8_0',
      url: '',
      sha256: '',
      status: { status: 'not_downloaded' },
      min_memory_gb: 32,
    },
  ];
}

/** Phase of an in-flight "make this model ready to chat" run. */
export type ModelPreparationPhase = 'preparing' | 'downloading' | 'verifying' | 'loading';

/** One in-flight ensure-ready run for a model (download, then verify, then load). */
export interface ModelPreparation {
  modelId: string;
  /**
   * The chat that started this run, or null when the download was already running
   * elsewhere (Settings). Only the owner may cancel; the download is shared with the daemon.
   */
  ownerId: string | null;
  phase: ModelPreparationPhase;
  bytesDownloaded: number;
  bytesTotal: number;
  /** True once the user asked to cancel; the run then fails with a cancelled error. */
  cancelRequested: boolean;
}

/** Message of the error `ensureReady` throws after a user-requested cancel. */
export const MODEL_DOWNLOAD_CANCELLED = 'Download cancelled';

class ModelStore {
  models = $state<ModelInfo[]>([]);
  downloadProgress = $state<Record<string, number>>({});
  loadedModelId = $state<string | null>(null);
  isLoading = $state(false);
  /** Total system RAM in GiB. 0 means unknown (non-Tauri or not yet loaded). */
  systemRamGb = $state(0);

  /**
   * Models currently being made ready for chat, keyed by model id. Lives here (not in
   * a viewer) so the state outlives a chat tab being deactivated and reactivated.
   */
  preparations = $state<Record<string, ModelPreparation>>({});

  private inFlightEnsure = new Map<string, Promise<void>>();
  /** Identity of the current run per model, so a superseded run never touches a newer one. */
  private runTokens = new Map<string, object>();
  private downloadAbortControllers = new Map<string, AbortController>();
  private eventUnlisteners: Array<() => void> = [];

  /** Whether at least one model is downloaded and ready. */
  get hasDownloadedModel(): boolean {
    return this.models.some(
      (m) => m.status.status === 'ready' || m.status.status === 'loaded'
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
        const [entries, ram] = await Promise.all([
          tauriCommands.chatModelList(),
          tauriCommands.getSystemRamGb(),
        ]);
        // This store manages the *built-in* (GGUF) download/load lifecycle; its
        // consumers (model-manager, onboarding) download by URL. Remotely-served
        // models have no URL here and are surfaced separately by
        // AiChatModelSelector, so exclude them — letting them in would render
        // dead download buttons.
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

  /**
   * Make a model ready to chat: download it if needed, verify it, load it. Progress is
   * published on `preparations[modelId]`. Concurrent calls for the same model share one
   * run, so a remounted viewer can await the run already in flight. `ownerId` identifies
   * the caller (a chat id); only the run's owner may cancel its download.
   */
  ensureReady(modelId: string, ownerId: string): Promise<void> {
    const existing = this.inFlightEnsure.get(modelId);
    if (existing) return existing;
    const run: Promise<void> = this.runEnsureReady(modelId, ownerId).finally(() => {
      if (this.inFlightEnsure.get(modelId) === run) this.inFlightEnsure.delete(modelId);
    });
    this.inFlightEnsure.set(modelId, run);
    return run;
  }

  /** Cancel the download of a model that `ensureReady` is preparing, if `ownerId` started it. */
  async cancelPreparation(modelId: string, ownerId: string): Promise<void> {
    const prep = this.preparations[modelId];
    if (!prep || prep.phase !== 'downloading' || prep.ownerId !== ownerId) return;
    this.patchPreparation(modelId, { cancelRequested: true });
    try {
      await tauriCommands.chatModelCancelDownload(modelId);
    } catch (err) {
      log.error('Failed to cancel download', { modelId, error: toError(err).message });
      this.patchPreparation(modelId, { cancelRequested: false });
    }
  }

  private async runEnsureReady(modelId: string, ownerId: string): Promise<void> {
    const token = {};
    this.runTokens.set(modelId, token);
    const isCurrent = () => this.runTokens.get(modelId) === token;
    // A download already running (e.g. started from Settings) is not this chat's to cancel.
    const startedElsewhere =
      this.models.find((m) => m.id === modelId)?.status.status === 'downloading';
    this.preparations = {
      ...this.preparations,
      [modelId]: {
        modelId,
        ownerId: startedElsewhere ? null : ownerId,
        phase: 'preparing',
        bytesDownloaded: 0,
        bytesTotal: 0,
        cancelRequested: false,
      },
    };
    const unlisteners: Array<() => void> = [];
    try {
      if (isTauri()) {
        const { listen } = await import('@tauri-apps/api/event');
        // Each unlisten is recorded as soon as its listen resolves, so a failure of the
        // second registration still releases the first.
        unlisteners.push(
          await listen<{ model_id: string; status: string }>(AGENT_EVENTS.MODEL_STATUS, (e) =>
            this.applyPreparationStatus(e.payload.model_id, e.payload.status)
          )
        );
        unlisteners.push(
          await listen<DownloadEvent>(AGENT_EVENTS.MODEL_DOWNLOAD_PROGRESS, (e) =>
            this.applyPreparationProgress(
              e.payload.model_id,
              e.payload.bytes_downloaded,
              e.payload.bytes_total
            )
          )
        );
      } else {
        // Browser mode: the dev-proxy relays the same daemon progress over SSE.
        const { browserSyncService } = await import('$lib/services/browser-sync-service');
        unlisteners.push(
          browserSyncService.onModelLoadProgress((e) => {
            this.applyPreparationStatus(e.modelId, e.status);
            if (e.bytesDownloaded !== undefined && e.bytesTotal !== undefined) {
              this.applyPreparationProgress(e.modelId, e.bytesDownloaded, e.bytesTotal);
            }
          })
        );
      }
      await tauriCommands.ensureModelReady(modelId);
    } catch (err) {
      // Only a failure while still downloading is the cancel; once the download had
      // finished, a verify/load failure is a real error.
      const prep = isCurrent() ? this.preparations[modelId] : undefined;
      if (prep?.cancelRequested && prep.phase === 'downloading') {
        throw new Error(MODEL_DOWNLOAD_CANCELLED);
      }
      throw err;
    } finally {
      for (const unlisten of unlisteners) unlisten();
      if (isCurrent()) {
        this.runTokens.delete(modelId);
        const { [modelId]: _done, ...remaining } = this.preparations;
        this.preparations = remaining;
      }
    }
  }

  private applyPreparationStatus(modelId: string, status: string): void {
    const prep = this.preparations[modelId];
    if (!prep) return;
    if (status === 'downloading' || status === 'verifying' || status === 'loading') {
      this.patchPreparation(modelId, { phase: status });
    }
  }

  private applyPreparationProgress(modelId: string, downloaded: number, total: number): void {
    const prep = this.preparations[modelId];
    if (!prep || (prep.phase !== 'preparing' && prep.phase !== 'downloading')) return;
    this.patchPreparation(modelId, {
      phase: 'downloading',
      bytesDownloaded: downloaded,
      bytesTotal: total,
    });
  }

  private patchPreparation(modelId: string, patch: Partial<ModelPreparation>): void {
    const prep = this.preparations[modelId];
    if (!prep) return;
    this.preparations = { ...this.preparations, [modelId]: { ...prep, ...patch } };
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
    this.preparations = {};
    this.inFlightEnsure.clear();
    this.runTokens.clear();
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
