/**
 * A chat row in a page outline never unfolds its messages (ADR-088 §3): the
 * messages are owned by the chat viewer, so BaseNodeViewer lists the chat's
 * non-message children but no `ai-chat-message` child, and gives the chat no
 * expand control for messages alone.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import BaseNodeViewerInContext from '../fixtures/base-node-viewer-in-context.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { Node } from '$lib/types/node';

const PAGE_ID = 'outline-page';
const CHAT_ID = 'outline-chat';
const databaseSource = { type: 'database' as const, reason: 'test' };

function seed(id: string, nodeType: string, content: string, parentId: string, order: number): void {
  sharedNodeStore.setNode(
    {
      id,
      nodeType,
      content,
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: {},
      lifecycleStatus: 'active',
      mentions: []
    } as unknown as Node,
    databaseSource
  );
  structureTree.addInMemoryRelationship(parentId, id, order);
}

describe('BaseNodeViewer outline and chat messages', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    sharedNodeStore.setNode(
      {
        id: PAGE_ID,
        nodeType: 'text',
        content: 'A page',
        version: 1,
        createdAt: '2026-01-01T00:00:00Z',
        modifiedAt: '2026-01-01T00:00:00Z',
        properties: {},
        lifecycleStatus: 'active',
        mentions: []
      } as unknown as Node,
      databaseSource
    );
    seed(CHAT_ID, 'ai-chat-native', 'Planning chat', PAGE_ID, 1);
    seed('outline-message-1', 'ai-chat-message', 'Message that stays in the chat', CHAT_ID, 1);
    seed('outline-note', 'text', 'A note kept under the chat', CHAT_ID, 2);
  });

  afterEach(() => {
    cleanup();
    for (const id of ['outline-message-1', 'outline-note', CHAT_ID, PAGE_ID]) {
      structureTree.removeNode(id);
    }
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('lists the chat children that are outline rows and none of its messages', async () => {
    const { container } = render(BaseNodeViewerInContext, { props: { nodeId: PAGE_ID } });

    await waitFor(() => {
      expect(container.querySelector(`[data-node-id="${CHAT_ID}"]`)).not.toBeNull();
    });

    await waitFor(() => {
      expect(container.querySelector('[data-node-id="outline-note"]')).not.toBeNull();
    });
    expect(container.querySelector('[data-node-id="outline-message-1"]')).toBeNull();
    expect(container.textContent).not.toContain('Message that stays in the chat');
  });
});
