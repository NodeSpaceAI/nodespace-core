/**
 * Choosing a terminal harness in a native chat's header selector turns the
 * node into an `ai-chat-pty` node (ADR-088): `agent` names the harness,
 * `model` is cleared (it only ever holds a model identifier), and the session
 * starts `active`. Selecting a model writes provider + model only.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import AiChatNativeNodeViewer from '$lib/components/viewers/ai-chat-native-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { agentStore } from '$lib/stores/agent-store.svelte';
import type { Node } from '$lib/types/node';

const CHAT_ID = 'native-chat-to-retype';

function seedNativeChat(): void {
  const node = {
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
    messages: [],
    turnStatus: 'idle',
    contextTokens: 0
  } as unknown as Node;
  sharedNodeStore.setNode(node, { type: 'database', reason: 'test' });
}

describe('AiChatNativeNodeViewer harness selection', () => {
  beforeEach(() => {
    // The header's model selector loads its catalog over HTTP in browser mode.
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    // A non-empty agent list keeps the selector from fetching one itself.
    agentStore.agents = [
      {
        id: 'claude-code',
        name: 'Claude Code',
        binary: 'claude',
        args: [],
        auth_method: { method: 'agent_managed' },
        available: true
      }
    ];
    seedNativeChat();
  });

  afterEach(() => {
    cleanup();
    agentStore.agents = [];
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('retypes the node to ai-chat-pty with the harness as agent and model cleared', async () => {
    const updateNode = vi.spyOn(sharedNodeStore, 'updateNode');
    const { findByLabelText } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });

    const select = (await findByLabelText('Select AI model')) as HTMLSelectElement;
    await fireEvent.change(select, { target: { value: 'pty:claude-code' } });

    expect(updateNode).toHaveBeenCalledTimes(1);
    const [id, changes] = updateNode.mock.calls[0];
    expect(id).toBe(CHAT_ID);
    expect(changes).toEqual({
      nodeType: 'ai-chat-pty',
      properties: { agent: 'claude-code', model: null }
    });
    // The old pty-as-provider encoding is gone.
    expect(JSON.stringify(changes)).not.toContain('"provider"');
  });

  it('writes only provider and model when a native model is chosen', async () => {
    const updateNode = vi.spyOn(sharedNodeStore, 'updateNode');
    const { findByLabelText } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });

    const select = (await findByLabelText('Select AI model')) as HTMLSelectElement;
    // Option values are built from the daemon's catalog; add one the way it would arrive.
    const option = document.createElement('option');
    option.value = 'native:other-model';
    select.appendChild(option);
    await fireEvent.change(select, { target: { value: 'native:other-model' } });

    expect(updateNode).toHaveBeenCalledTimes(1);
    expect(updateNode.mock.calls[0][1]).toEqual({
      properties: { provider: 'native', model: 'other-model' }
    });
  });
});
