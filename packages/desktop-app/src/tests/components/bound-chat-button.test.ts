/**
 * BoundChatButton (ADR-100 §1): the one way a viewer offers the chat bound to
 * its node. A click opens a chat for the node and shows it beside the viewer;
 * a failure is reported through `error` and opens nothing.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import BoundChatButton from '$lib/components/chat/bound-chat-button.svelte';
import { getNavigationService } from '$lib/services/navigation-service';
import { aiChatsData } from '$lib/stores/ai-chats.svelte';
import type { Node } from '$lib/types';

const chat = { id: 'chat-1', nodeType: 'ai-chat-native' } as unknown as Node;

describe('BoundChatButton', () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    aiChatsData.createError = '';
  });

  it('opens a chat bound to its node beside the viewer, under the label given', async () => {
    const openBoundChat = vi.spyOn(aiChatsData, 'openBoundChat').mockResolvedValue(chat);
    const navigate = vi
      .spyOn(getNavigationService(), 'navigateToNodeInOtherPane')
      .mockResolvedValue();
    const { getByRole } = render(BoundChatButton, { nodeId: 'node-1', label: 'Ask' });

    await fireEvent.click(getByRole('button', { name: 'Ask' }));
    await tick();

    expect(openBoundChat).toHaveBeenCalledWith('node-1');
    expect(navigate).toHaveBeenCalledWith('chat-1');
  });

  it('opens nothing when the chat could not be created', async () => {
    vi.spyOn(aiChatsData, 'openBoundChat').mockImplementation(async () => {
      aiChatsData.createError = 'node node-1 is a text, which cannot be opened in a chat';
      return null;
    });
    const navigate = vi
      .spyOn(getNavigationService(), 'navigateToNodeInOtherPane')
      .mockResolvedValue();
    const { getByRole } = render(BoundChatButton, { nodeId: 'node-1' });

    await fireEvent.click(getByRole('button', { name: 'Edit' }));
    await tick();

    expect(navigate).not.toHaveBeenCalled();
  });
});
