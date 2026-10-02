/**
 * AiChatNativeNodeViewer: a chat's messages are `ai-chat-message` child nodes
 * (ADR-088 §3). The viewer derives the conversation from the chat's children in
 * the structure tree, ignores other children, and sends a turn by creating a
 * user message child and then setting the chat's `turn_status`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

vi.mock('$lib/services/tauri-commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/services/tauri-commands')>()),
  ensureModelReady: vi.fn().mockResolvedValue(true)
}));

import AiChatNativeNodeViewer from '$lib/components/viewers/ai-chat-native-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import type { Node } from '$lib/types/node';

const CHAT_ID = 'chat-with-message-children';
const databaseSource = { type: 'database' as const, reason: 'test' };

function seedChat(turnStatus: 'idle' | 'processing' = 'idle'): void {
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
      turnStatus,
      contextTokens: 0
    } as unknown as Node,
    databaseSource
  );
}

/** Seed a child of the chat, ordered after the previously seeded one. */
function seedChild(
  id: string,
  nodeType: string,
  content: string,
  fields: Record<string, unknown>,
  order: number
): void {
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
      mentions: [],
      ...fields
    } as unknown as Node,
    databaseSource
  );
  structureTree.addInMemoryRelationship(CHAT_ID, id, order);
}

function messageIds(container: HTMLElement): (string | null)[] {
  return [...container.querySelectorAll('[data-message-id]')].map((el) =>
    el.getAttribute('data-message-id')
  );
}

describe('AiChatNativeNodeViewer message nodes', () => {
  beforeEach(() => {
    // The header's model selector and the children load go over HTTP in browser mode.
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    seedChat();
  });

  afterEach(() => {
    cleanup();
    structureTree.removeNode(CHAT_ID);
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('renders the message children in conversation order and ignores other children', async () => {
    seedChild('m-1', 'ai-chat-message', 'What is on my plate?', { role: 'user' }, 1);
    seedChild('note-1', 'text', 'A note under the chat', {}, 2);
    seedChild(
      'm-2',
      'ai-chat-message',
      'Two tasks are due.',
      { role: 'assistant', timestamp: '2026-01-01T00:00:05Z' },
      3
    );

    const { container, findByText, queryByText } = render(AiChatNativeNodeViewer, {
      props: { nodeId: CHAT_ID }
    });

    await findByText('What is on my plate?');
    await findByText('Two tasks are due.');
    expect(messageIds(container)).toEqual(['m-1', 'm-2']);
    expect(queryByText('A note under the chat')).toBeNull();
  });

  it('loads the chat children when it mounts', async () => {
    const load = vi.spyOn(sharedNodeStore, 'loadChildrenForParent').mockResolvedValue([]);
    const { findByText } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });

    await findByText('Start a conversation');
    expect(load).toHaveBeenCalledWith(CHAT_ID);
  });

  it('shows a message node that arrives after mount', async () => {
    const { container, findByText } = render(AiChatNativeNodeViewer, {
      props: { nodeId: CHAT_ID }
    });
    await findByText('Start a conversation');

    seedChild('m-late', 'ai-chat-message', 'A late reply', { role: 'assistant' }, 1);

    await findByText('A late reply');
    expect(messageIds(container)).toEqual(['m-late']);
  });

  it('locks the model selector once a user message exists', async () => {
    seedChild('m-1', 'ai-chat-message', 'hello', { role: 'user' }, 1);
    const { findByLabelText } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });

    const select = (await findByLabelText('Select AI model')) as HTMLSelectElement;
    expect(select.disabled).toBe(true);
  });

  it('leaves the model selector open while the chat holds no user message', async () => {
    seedChild('m-1', 'ai-chat-message', 'a stray assistant note', { role: 'assistant' }, 1);
    const { findByLabelText } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });

    const select = (await findByLabelText('Select AI model')) as HTMLSelectElement;
    expect(select.disabled).toBe(false);
  });

  it('sends a turn by creating a user message child, then setting turn_status', async () => {
    vi.spyOn(sharedNodeStore, 'loadChildrenForParent').mockResolvedValue([]);
    vi.spyOn(backendAdapter, 'createNode').mockResolvedValue({ id: 'ignored', placement: null });
    const calls: string[] = [];
    const setNode = vi.spyOn(sharedNodeStore, 'setNode').mockImplementation((node) => {
      calls.push(`setNode:${node.nodeType}`);
      return true;
    });
    const updateNode = vi.spyOn(sharedNodeStore, 'updateNode').mockImplementation(() => {
      calls.push('updateNode');
    });

    const { findByLabelText } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    const input = (await findByLabelText('Chat message input')) as HTMLTextAreaElement;
    await fireEvent.input(input, { target: { value: '  hello there  ' } });
    await fireEvent.click(await findByLabelText('Send message'));

    await waitFor(() => expect(updateNode).toHaveBeenCalledTimes(1));

    expect(calls).toEqual(['setNode:ai-chat-message', 'updateNode']);

    const created = setNode.mock.calls[0][0];
    expect(created.content).toBe('hello there');
    expect(created.properties).toMatchObject({ role: 'user' });
    expect(typeof created.properties.timestamp).toBe('string');
    expect(setNode.mock.calls[0][1]).toEqual({ type: 'viewer', viewerId: 'ai-chat-viewer' });

    // It is the chat's last child, so the daemon sees a user message to answer.
    expect(structureTree.getChildren(CHAT_ID).at(-1)).toBe(created.id);

    const [id, changes] = updateNode.mock.calls[0];
    expect(id).toBe(CHAT_ID);
    expect(changes).toEqual({ properties: { turn_status: 'processing' } });
  });

  it('asks the backend for the turn only after the message is stored', async () => {
    vi.spyOn(sharedNodeStore, 'loadChildrenForParent').mockResolvedValue([]);
    const calls: string[] = [];
    let storeMessage: (() => void) | undefined;
    vi.spyOn(backendAdapter, 'createNode').mockImplementation(async (node) => {
      calls.push(`createNode:${node.nodeType}:${node.parentId}`);
      await new Promise<void>((resolve) => {
        storeMessage = resolve;
      });
      calls.push('createNode stored');
      return { id: node.id, placement: null };
    });
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (id, _version, update) => {
      calls.push(`updateNode:${JSON.stringify(update.properties)}`);
      return { ...sharedNodeStore.getNode(id)!, version: 2 };
    });

    const { findByLabelText } = render(AiChatNativeNodeViewer, { props: { nodeId: CHAT_ID } });
    const input = (await findByLabelText('Chat message input')) as HTMLTextAreaElement;
    await fireEvent.input(input, { target: { value: 'hello' } });
    await fireEvent.click(await findByLabelText('Send message'));

    // The create is in flight and the status has not been written.
    await waitFor(() => expect(storeMessage).toBeDefined());
    expect(calls).toEqual([`createNode:ai-chat-message:${CHAT_ID}`]);

    storeMessage!();
    await waitFor(() => expect(calls).toHaveLength(3));
    expect(calls).toEqual([
      `createNode:ai-chat-message:${CHAT_ID}`,
      'createNode stored',
      'updateNode:{"turn_status":"processing"}'
    ]);
  });

  it('does not ask for a turn when the message cannot be stored', async () => {
    vi.spyOn(sharedNodeStore, 'loadChildrenForParent').mockResolvedValue([]);
    vi.spyOn(backendAdapter, 'createNode').mockRejectedValue(new Error('disk full'));
    const updateNode = vi.spyOn(backendAdapter, 'updateNode');

    const { container, findByLabelText, findByRole } = render(AiChatNativeNodeViewer, {
      props: { nodeId: CHAT_ID }
    });
    const input = (await findByLabelText('Chat message input')) as HTMLTextAreaElement;
    await fireEvent.input(input, { target: { value: 'hello' } });
    await fireEvent.click(await findByLabelText('Send message'));

    const alert = await findByRole('alert');
    expect(alert.textContent).toContain('could not be saved');
    expect(updateNode).not.toHaveBeenCalled();
    // The conversation holds only what was sent: the message that was shown
    // while it was being stored is taken back.
    await waitFor(() => expect(messageIds(container)).toEqual([]));
    expect(structureTree.getChildren(CHAT_ID)).toEqual([]);
  });

  it('puts the optimistic user message after the existing messages', async () => {
    seedChild('m-1', 'ai-chat-message', 'first', { role: 'user' }, 1);
    seedChild('m-2', 'ai-chat-message', 'second', { role: 'assistant' }, 2);
    vi.spyOn(backendAdapter, 'createNode').mockResolvedValue({ id: 'ignored', placement: null });
    vi.spyOn(sharedNodeStore, 'updateNode').mockImplementation(() => {});

    const { container, findByLabelText, findByText } = render(AiChatNativeNodeViewer, {
      props: { nodeId: CHAT_ID }
    });
    const input = (await findByLabelText('Chat message input')) as HTMLTextAreaElement;
    await fireEvent.input(input, { target: { value: 'third' } });
    await fireEvent.click(await findByLabelText('Send message'));

    await findByText('third');
    const ids = messageIds(container);
    expect(ids.slice(0, 2)).toEqual(['m-1', 'm-2']);
    expect(ids).toHaveLength(3);
  });
});
