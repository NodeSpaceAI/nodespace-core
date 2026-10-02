/**
 * AiChatNativeNodeViewer and node lifecycle.
 *
 * Archived means hidden, not read-only (ADR-087): the viewer renders the same
 * message input for a chat whatever its lifecycle, and gives lifecycle no
 * meaning of its own, so it never reads the field.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import viewerSource from '$lib/components/viewers/ai-chat-native-node-viewer.svelte?raw';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import AiChatNativeNodeViewer from '$lib/components/viewers/ai-chat-native-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import type { Node } from '$lib/types/node';

function seedChat(id: string, lifecycleStatus: 'active' | 'archived'): void {
  const node = {
    id,
    nodeType: 'ai-chat-native',
    content: 'Chat',
    version: 1,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    properties: {},
    lifecycleStatus,
    agent: 'nodespace',
    provider: 'native',
    model: 'test-model',
    turnStatus: 'idle'
  } as unknown as Node;
  sharedNodeStore.setNode(node, { type: 'database', reason: 'test' });
}

describe('AiChatNativeNodeViewer lifecycle', () => {
  beforeEach(() => {
    // The header's model selector loads its catalog over HTTP in browser mode.
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it.each(['active', 'archived'] as const)(
    'renders the message input for an %s chat',
    async (lifecycle) => {
      seedChat(`chat-${lifecycle}`, lifecycle);

      const { findByPlaceholderText, queryByText } = render(AiChatNativeNodeViewer, {
        props: { nodeId: `chat-${lifecycle}` }
      });

      const input = await findByPlaceholderText('Type a message...');
      expect((input as HTMLTextAreaElement).disabled).toBe(false);
      expect(queryByText(/read-only/i)).toBeNull();
    }
  );

  it('reads no lifecycle status', () => {
    expect(viewerSource).not.toMatch(/lifecycle_?status/i);
  });
});
