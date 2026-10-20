/**
 * ModelStore.ensureReady: the phase/progress state the AI chat overlay reads.
 * No model is downloaded or loaded: the Tauri commands and events are mocked.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  modelStore,
  describePreparation,
  formatDownloadSize,
  MODEL_DOWNLOAD_CANCELLED,
  type ModelPreparation
} from '$lib/stores/model-store.svelte';
import * as tauriCommands from '$lib/services/tauri-commands';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const listeners = new Map<string, (event: { payload: unknown }) => void>();
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return () => listeners.delete(name);
  })
}));

vi.mock('$lib/services/tauri-commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/services/tauri-commands')>()),
  ensureModelReady: vi.fn(),
  chatModelCancelDownload: vi.fn().mockResolvedValue(undefined)
}));

const GB = 1024 ** 3;
const emit = (name: string, payload: unknown) => listeners.get(name)?.({ payload });

type WindowWithTauri = Window & { __TAURI_INTERNALS__?: Record<string, unknown> };

describe('ModelStore.ensureReady', () => {
  let finish: () => void;
  let fail: (e: Error) => void;

  beforeEach(() => {
    vi.clearAllMocks();
    modelStore.reset();
    listeners.clear();
    (window as WindowWithTauri).__TAURI_INTERNALS__ = {};
    vi.mocked(tauriCommands.ensureModelReady).mockImplementation(
      () =>
        new Promise<boolean>((resolve, reject) => {
          finish = () => resolve(true);
          fail = reject;
        })
    );
  });

  afterEach(() => {
    delete (window as WindowWithTauri).__TAURI_INTERNALS__;
  });

  it('moves through downloading, verifying and loading, then clears', async () => {
    const run = modelStore.ensureReady('m1', 'chat-1');
    expect(modelStore.preparations['m1'].phase).toBe('preparing');
    await vi.waitFor(() => expect(listeners.size).toBe(2));

    emit('model://status', { model_id: 'm1', status: 'downloading' });
    emit('model://download-progress', {
      model_id: 'm1',
      bytes_downloaded: 1.2 * GB,
      bytes_total: 4.5 * GB
    });
    expect(modelStore.preparations['m1']).toMatchObject({
      phase: 'downloading',
      bytesDownloaded: 1.2 * GB,
      bytesTotal: 4.5 * GB
    });

    emit('model://status', { model_id: 'm1', status: 'verifying' });
    expect(modelStore.preparations['m1'].phase).toBe('verifying');
    // a late progress event must not drag the phase back to downloading
    emit('model://download-progress', { model_id: 'm1', bytes_downloaded: 1, bytes_total: 2 });
    expect(modelStore.preparations['m1'].phase).toBe('verifying');

    emit('model://status', { model_id: 'm1', status: 'loading' });
    expect(modelStore.preparations['m1'].phase).toBe('loading');

    finish();
    await run;
    expect(modelStore.preparations['m1']).toBeUndefined();
    expect(listeners.size).toBe(0);
  });

  it('skips the download phase for a cached model', async () => {
    const run = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(listeners.size).toBe(2));
    emit('model://status', { model_id: 'm1', status: 'verifying' });
    emit('model://status', { model_id: 'm1', status: 'loading' });
    expect(modelStore.preparations['m1'].bytesTotal).toBe(0);
    finish();
    await run;
  });

  it('ignores events for other models', async () => {
    const run = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(listeners.size).toBe(2));
    emit('model://status', { model_id: 'other', status: 'loading' });
    expect(modelStore.preparations['m1'].phase).toBe('preparing');
    expect(modelStore.preparations['other']).toBeUndefined();
    finish();
    await run;
  });

  it('shares one run between concurrent callers', async () => {
    const a = modelStore.ensureReady('m1', 'chat-1');
    const b = modelStore.ensureReady('m1', 'chat-1');
    expect(a).toBe(b);
    await vi.waitFor(() => expect(tauriCommands.ensureModelReady).toHaveBeenCalled());
    expect(tauriCommands.ensureModelReady).toHaveBeenCalledTimes(1);
    finish();
    await a;
  });

  it('clears the state and rethrows when preparation fails', async () => {
    const run = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(listeners.size).toBe(2));
    fail(new Error('disk full'));
    await expect(run).rejects.toThrow('disk full');
    expect(modelStore.preparations['m1']).toBeUndefined();
  });

  it('cancels a download and reports it as cancelled', async () => {
    const run = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(listeners.size).toBe(2));
    emit('model://download-progress', { model_id: 'm1', bytes_downloaded: 1, bytes_total: 10 });

    await modelStore.cancelPreparation('m1', 'chat-1');
    expect(tauriCommands.chatModelCancelDownload).toHaveBeenCalledWith('m1');
    expect(modelStore.preparations['m1'].cancelRequested).toBe(true);

    fail(new Error('download aborted'));
    await expect(run).rejects.toThrow(MODEL_DOWNLOAD_CANCELLED);
    expect(modelStore.preparations['m1']).toBeUndefined();
    finish();
  });

  it('does not let another chat cancel a download it did not start', async () => {
    const run = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(listeners.size).toBe(2));
    emit('model://download-progress', { model_id: 'm1', bytes_downloaded: 1, bytes_total: 10 });
    await modelStore.cancelPreparation('m1', 'chat-2');
    expect(tauriCommands.chatModelCancelDownload).not.toHaveBeenCalled();
    expect(modelStore.preparations['m1'].cancelRequested).toBe(false);
    finish();
    await run;
  });

  it('has no owner when the download was already running elsewhere', async () => {
    modelStore.models = [
      {
        id: 'm1',
        family: 'gemma4',
        name: 'M1',
        filename: '',
        size_bytes: 10,
        quantization: 'Q4',
        url: '',
        sha256: '',
        status: { status: 'downloading', progress_pct: 5, bytes_downloaded: 1, bytes_total: 10 },
        min_memory_gb: 1
      }
    ];
    const run = modelStore.ensureReady('m1', 'chat-1');
    expect(modelStore.preparations['m1'].ownerId).toBeNull();
    await vi.waitFor(() => expect(listeners.size).toBe(2));
    emit('model://download-progress', { model_id: 'm1', bytes_downloaded: 1, bytes_total: 10 });
    await modelStore.cancelPreparation('m1', 'chat-1');
    expect(tauriCommands.chatModelCancelDownload).not.toHaveBeenCalled();
    finish();
    await run;
  });

  it('releases the first listener when registering the second fails', async () => {
    const { listen } = await import('@tauri-apps/api/event');
    const unlistenFirst = vi.fn();
    vi.mocked(listen)
      .mockResolvedValueOnce(unlistenFirst)
      .mockRejectedValueOnce(new Error('listen failed'));
    await expect(modelStore.ensureReady('m1', 'chat-1')).rejects.toThrow('listen failed');
    expect(unlistenFirst).toHaveBeenCalledTimes(1);
    expect(modelStore.preparations['m1']).toBeUndefined();
  });

  it('does not let a superseded run delete a newer run after reset', async () => {
    const runA = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(tauriCommands.ensureModelReady).toHaveBeenCalledTimes(1));
    const finishA = finish;

    modelStore.reset();
    const runB = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(tauriCommands.ensureModelReady).toHaveBeenCalledTimes(2));
    const finishB = finish;

    finishA();
    await runA;
    expect(modelStore.preparations['m1']).toBeDefined();
    expect(modelStore.ensureReady('m1', 'chat-1')).toBe(runB);

    finishB();
    await runB;
    expect(modelStore.preparations['m1']).toBeUndefined();
  });

  it('reports a real error, not a cancel, when verify fails after a late cancel', async () => {
    const run = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(listeners.size).toBe(2));
    emit('model://download-progress', { model_id: 'm1', bytes_downloaded: 1, bytes_total: 10 });
    await modelStore.cancelPreparation('m1', 'chat-1');
    // the download had already finished: the daemon moved on to verifying
    emit('model://status', { model_id: 'm1', status: 'verifying' });
    fail(new Error('checksum mismatch'));
    await expect(run).rejects.toThrow('checksum mismatch');
  });

  it('does not cancel outside the downloading phase', async () => {
    const run = modelStore.ensureReady('m1', 'chat-1');
    await vi.waitFor(() => expect(listeners.size).toBe(2));
    await modelStore.cancelPreparation('m1', 'chat-1');
    expect(tauriCommands.chatModelCancelDownload).not.toHaveBeenCalled();
    finish();
    await run;
  });
});

describe('describePreparation', () => {
  const prep = (patch: Partial<ModelPreparation>): ModelPreparation => ({
    modelId: 'm1',
    phase: 'preparing',
    bytesDownloaded: 0,
    bytesTotal: 0,
    ownerId: 'chat-1',
    cancelRequested: false,
    ...patch
  });

  it('labels each phase', () => {
    expect(describePreparation(prep({}), 'Gemma').label).toBe('Preparing model…');
    expect(
      describePreparation(
        prep({ phase: 'downloading', bytesDownloaded: 1.2 * GB, bytesTotal: 4.5 * GB }),
        'Gemma'
      ).label
    ).toBe('Downloading Gemma… 26% (1.2 of 4.5 GB)');
    expect(describePreparation(prep({ phase: 'verifying' }), 'Gemma').label).toBe(
      'Verifying model integrity…'
    );
    expect(describePreparation(prep({ phase: 'verifying' }), 'Gemma').sublabel).toBeTruthy();
    expect(describePreparation(prep({ phase: 'loading' }), 'Gemma').label).toBe('Loading model…');
  });

  it('shows no percentage before byte counts arrive', () => {
    expect(describePreparation(prep({ phase: 'downloading' }), 'Gemma').label).toBe(
      'Downloading Gemma…'
    );
  });

  it('uses the total size unit', () => {
    expect(formatDownloadSize(50 * 1024 ** 2, 200 * 1024 ** 2)).toBe('50.0 of 200.0 MB');
  });
});
