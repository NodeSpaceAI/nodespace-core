/**
 * AiChatNativeNodeViewer model overlay: derived from the model store's preparation
 * state, so it shows the right label per phase and survives the viewer being
 * unmounted and mounted again (tab switch). Store state is set directly; nothing is
 * downloaded or loaded.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import AiChatNativeNodeViewer from '$lib/components/viewers/ai-chat-native-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { modelStore, type ModelPreparation } from '$lib/stores/model-store.svelte';
import * as tauriCommands from '$lib/services/tauri-commands';
import type { Node } from '$lib/types/node';

const CHAT_ID = 'chat-overlay';
const MODEL_ID = 'overlay-model';
const GB = 1024 ** 3;

function seedChat(): void {
  sharedNodeStore.setNode(
    {
      id: CHAT_ID,
      nodeType: 'ai-chat-native',
      content: 'Chat',
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: {},
      lifecycleStatus: 'active',
      agent: 'nodespace',
      provider: 'native',
      model: MODEL_ID,
      turnStatus: 'idle'
    } as unknown as Node,
    { type: 'database', reason: 'test' }
  );
}

function prepare(patch: Partial<ModelPreparation>): void {
  modelStore.preparations = {
    [MODEL_ID]: {
      modelId: MODEL_ID,
      ownerId: CHAT_ID,
      phase: 'preparing',
      bytesDownloaded: 0,
      bytesTotal: 0,
      cancelRequested: false,
      ...patch
    }
  };
}

describe('AI chat model overlay', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    modelStore.reset();
    seedChat();
  });

  afterEach(() => {
    cleanup();
    modelStore.reset();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('shows no overlay when the model is not being prepared', async () => {
    const { queryByRole } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    await tick();
    expect(queryByRole('status')).toBeNull();
  });

  it.each([
    [{ phase: 'preparing' as const }, 'Preparing model…'],
    [
      { phase: 'downloading' as const, bytesDownloaded: 1.2 * GB, bytesTotal: 4.5 * GB },
      `Downloading ${MODEL_ID}… 26% (1.2 of 4.5 GB)`
    ],
    [{ phase: 'verifying' as const }, 'Verifying model integrity…'],
    [{ phase: 'loading' as const }, 'Loading model…']
  ])('labels the %j phase', async (patch, label) => {
    prepare(patch);
    const { getByRole } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    await tick();
    expect(getByRole('status').getAttribute('aria-label')).toBe(label);
  });

  it('updates as the store moves from downloading to verifying to loading', async () => {
    prepare({ phase: 'downloading', bytesDownloaded: GB, bytesTotal: 4 * GB });
    const { getByRole } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    await tick();
    expect(getByRole('status').getAttribute('aria-label')).toContain('Downloading');
    prepare({ phase: 'verifying' });
    await tick();
    expect(getByRole('status').getAttribute('aria-label')).toBe('Verifying model integrity…');
    prepare({ phase: 'loading' });
    await tick();
    expect(getByRole('status').getAttribute('aria-label')).toBe('Loading model…');
    modelStore.preparations = {};
    await tick();
    expect(document.querySelector('[role="status"]')).toBeNull();
  });

  it('still shows the overlay at the current phase after the viewer is remounted', async () => {
    prepare({ phase: 'downloading', bytesDownloaded: 2 * GB, bytesTotal: 4 * GB });
    const first = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    await tick();
    expect(first.getByRole('status')).toBeTruthy();

    // Tab deactivated: the viewer is destroyed. The preparation carries on in the store.
    first.unmount();
    expect(document.querySelector('[role="status"]')).toBeNull();
    prepare({ phase: 'verifying' });

    // Tab reactivated: a fresh viewer reads the store and shows the current phase.
    const second = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    await tick();
    expect(second.getByRole('status').getAttribute('aria-label')).toBe(
      'Verifying model integrity…'
    );
  });

  it.each([
    ['another chat', 'other-chat'],
    ['a download started elsewhere', null]
  ])('offers no cancel when the download belongs to %s', async (_name, ownerId) => {
    prepare({ phase: 'downloading', bytesDownloaded: GB, bytesTotal: 4 * GB, ownerId });
    const { getByRole, queryByRole } = render(AiChatNativeNodeViewer, {
      props: { nodeId: CHAT_ID }
    });
    await tick();
    expect(getByRole('status').getAttribute('aria-label')).toContain('Downloading');
    expect(queryByRole('button', { name: 'Cancel' })).toBeNull();
  });

  it('offers cancel only while downloading and cancels through the store', async () => {
    const cancel = vi
      .spyOn(tauriCommands, 'chatModelCancelDownload')
      .mockResolvedValue(undefined);
    prepare({ phase: 'downloading', bytesDownloaded: GB, bytesTotal: 4 * GB });
    const { getByRole, queryByRole } = render(AiChatNativeNodeViewer, {
      props: { nodeId: CHAT_ID }
    });
    await tick();
    await fireEvent.click(getByRole('button', { name: 'Cancel' }));
    expect(cancel).toHaveBeenCalledWith(MODEL_ID);

    prepare({ phase: 'verifying' });
    await tick();
    expect(queryByRole('button', { name: 'Cancel' })).toBeNull();
  });
});
