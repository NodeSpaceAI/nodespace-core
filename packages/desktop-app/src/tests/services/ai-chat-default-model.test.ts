/**
 * New ai-chat-native nodes start on the user's default model (Settings → AI Models).
 * Covers the resolver and the creation path (`createSchemaInstance`, used by
 * the sidebar "+ New chat" and the ai-chat-native type view's "+ New").
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { Node } from '$lib/types';

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: { createNode: vi.fn(), getNode: vi.fn() }
}));

import { backendAdapter } from '$lib/services/backend-adapter';
import { createSchemaInstance } from '$lib/services/schema-authoring';
import { getDefaultAiChatModelProperties } from '$lib/services/ai-chat-default-model';
import { saveDefaultModelSelection, settingsStore } from '$lib/stores/settings.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { shouldSkipStaleAiChatUpdate } from '$lib/services/remote-update-policy';

const createNodeMock = vi.mocked(backendAdapter.createNode);
const getNodeMock = vi.mocked(backendAdapter.getNode);

const CONFIG_ID = '11111111-2222-3333-4444-555555555555';

function seedConfigs(ids: string[]): void {
  settingsStore.openAiConfigs = ids.map((id) => ({
    id,
    name: id,
    base_url: 'http://x',
    api_key: '',
    model: 'm',
    routing_ok: {}
  }));
}

function created(id: string, props: Record<string, unknown>): Node {
  return {
    id,
    nodeType: 'ai-chat-native',
    content: 'Untitled',
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: {},
    ...props
  } as Node;
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  settingsStore.openAiConfigs = [];
  createNodeMock.mockImplementation(async (input) => ({ id: (input as Node).id, placement: null }));
  getNodeMock.mockImplementation(async (id: string) => created(id, {}));
});

describe('getDefaultAiChatModelProperties', () => {
  it('is null with no default set', () => {
    expect(getDefaultAiChatModelProperties()).toBeNull();
  });

  it('maps a native default, downloaded or not', () => {
    saveDefaultModelSelection({ provider: 'native', modelId: 'qwen3-4b' });
    expect(getDefaultAiChatModelProperties()).toEqual({ provider: 'native', model: 'qwen3-4b' });
  });

  it('maps an openai-compat default to the qualified daemon id', () => {
    seedConfigs([CONFIG_ID]);
    saveDefaultModelSelection({
      provider: 'openai-compat',
      modelId: `openai-compat:${CONFIG_ID}:llama3.1:8b`,
      configId: CONFIG_ID
    });
    expect(getDefaultAiChatModelProperties()).toEqual({
      provider: 'openai-compat',
      model: `openai-compat:${CONFIG_ID}:llama3.1:8b`
    });
  });

  it('qualifies a legacy bare-config-id openai-compat default', () => {
    seedConfigs([CONFIG_ID]);
    saveDefaultModelSelection({ provider: 'openai-compat', modelId: CONFIG_ID, configId: CONFIG_ID });
    expect(getDefaultAiChatModelProperties()).toEqual({
      provider: 'openai-compat',
      model: `openai-compat:${CONFIG_ID}`
    });
  });

  it('ignores a native default with an empty model id', () => {
    saveDefaultModelSelection({ provider: 'native', modelId: '' });
    expect(getDefaultAiChatModelProperties()).toBeNull();
  });

  it('treats a default pointing at a removed openai-compat config as no default', () => {
    seedConfigs([]);
    saveDefaultModelSelection({
      provider: 'openai-compat',
      modelId: `openai-compat:${CONFIG_ID}`,
      configId: CONFIG_ID
    });
    expect(getDefaultAiChatModelProperties()).toBeNull();
  });
});

describe('createSchemaInstance for ai-chat-native', () => {
  it('writes the agent and the default provider + model at creation', async () => {
    saveDefaultModelSelection({ provider: 'native', modelId: 'qwen3-4b' });
    await createSchemaInstance('ai-chat-native');
    expect(createNodeMock.mock.calls[0][0]).toEqual(
      expect.objectContaining({
        nodeType: 'ai-chat-native',
        content: 'Untitled',
        properties: { agent: 'nodespace', provider: 'native', model: 'qwen3-4b' }
      })
    );
  });

  it('creates with only the required agent when no default is set', async () => {
    await createSchemaInstance('ai-chat-native');
    expect(createNodeMock.mock.calls[0][0]).toEqual(
      expect.objectContaining({
        nodeType: 'ai-chat-native',
        properties: { agent: 'nodespace' }
      })
    );
  });

  it('does not seed a model onto other node types', async () => {
    saveDefaultModelSelection({ provider: 'native', modelId: 'qwen3-4b' });
    await createSchemaInstance('invoice');
    expect(createNodeMock.mock.calls[0][0]).toEqual(
      expect.objectContaining({ nodeType: 'invoice', properties: {} })
    );
  });

  it('leaves existing chats untouched when the default changes', async () => {
    saveDefaultModelSelection({ provider: 'native', modelId: 'old-model' });
    const existing = created('existing-chat', { provider: 'native', model: 'old-model' });
    sharedNodeStore.setNode(existing, { type: 'database', reason: 'test' });

    saveDefaultModelSelection({ provider: 'native', modelId: 'new-model' });

    const node = sharedNodeStore.getNode('existing-chat') as unknown as { model?: string };
    expect(node.model).toBe('old-model');
    expect(createNodeMock).not.toHaveBeenCalled();
  });

  it('keeps the creation-time model through the first echo of the create', () => {
    // The echo of the chat's own creation carries the model (it was written
    // at creation), so it is neither treated as stale nor able to erase it.
    const local = created('echo-chat', { provider: 'native', model: 'qwen3-4b' });
    sharedNodeStore.setNode(local, { type: 'database', reason: 'ai-chat-created' });

    const echo = created('echo-chat', { provider: 'native', model: 'qwen3-4b' });
    const existing = sharedNodeStore.getNode('echo-chat');
    expect(
      shouldSkipStaleAiChatUpdate(echo, existing, { type: 'database', reason: 'sse' }, false)
    ).toBe(false);
    sharedNodeStore.setNode(echo, { type: 'database', reason: 'sse' });

    const after = sharedNodeStore.getNode('echo-chat') as unknown as {
      provider?: string;
      model?: string;
    };
    expect(after.provider).toBe('native');
    expect(after.model).toBe('qwen3-4b');
  });
});
