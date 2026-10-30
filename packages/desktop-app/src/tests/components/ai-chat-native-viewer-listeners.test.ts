/**
 * AiChatNativeNodeViewer releases its Tauri event subscriptions even when it
 * is destroyed while a `listen()` call is still pending.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const pending: Array<(unlisten: () => void) => void> = [];
// Only the viewer's own agent subscriptions are held pending; other
// components' listeners resolve immediately.
const listenMock = vi.fn((event: string) =>
  event.startsWith('local-agent://')
    ? new Promise<() => void>((resolve) => pending.push(resolve))
    : Promise.resolve(() => {})
);
vi.mock('@tauri-apps/api/event', () => ({
  listen: (event: string) => listenMock(event)
}));

function agentListenCalls(): number {
  return listenMock.mock.calls.filter(([e]) => e.startsWith('local-agent://')).length;
}

import AiChatNativeNodeViewer from '$lib/components/viewers/ai-chat-native-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import type { Node } from '$lib/types/node';

const CHAT_ID = 'native-chat-listeners';

function flush(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0));
}

describe('AiChatNativeNodeViewer listener cleanup', () => {
  beforeEach(() => {
    pending.length = 0;
    listenMock.mockClear();
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
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
        model: 'test-model',
        turnStatus: 'idle'
      } as unknown as Node,
      { type: 'database', reason: 'test' }
    );
  });

  afterEach(() => {
    cleanup();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    vi.unstubAllGlobals();
  });

  it('unlistens the chunk subscription resolving after destroy and skips the second listen', async () => {
    const { unmount } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    await flush();
    expect(agentListenCalls()).toBe(1);

    unmount();
    const unlistenChunk = vi.fn();
    pending[0](unlistenChunk);
    await flush();

    expect(unlistenChunk).toHaveBeenCalledTimes(1);
    expect(agentListenCalls()).toBe(1);
  });

  it('unlistens the error subscription resolving after destroy', async () => {
    const { unmount } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    await flush();
    const unlistenChunk = vi.fn();
    pending[0](unlistenChunk);
    await flush();
    expect(agentListenCalls()).toBe(2);

    unmount();
    const unlistenError = vi.fn();
    pending[1](unlistenError);
    await flush();

    expect(unlistenChunk).toHaveBeenCalledTimes(1);
    expect(unlistenError).toHaveBeenCalledTimes(1);
  });
});
