/**
 * A chat message is never shown as a standalone node (ADR-088 §3): opening its
 * id, by link, backlink or a restored tab, opens its parent chat scrolled to the
 * message. A message whose parent is not known opens nothing.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import { getNavigationService } from '$lib/services/navigation-service';
import { navigationStore, addTab, DEFAULT_PANE_ID } from '$lib/stores/navigation.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import type { Node } from '$lib/types/node';

const CHAT_ID = 'nav-chat';
const MESSAGE_ID = 'nav-chat-message';
const databaseSource = { type: 'database' as const, reason: 'test' };

function node(id: string, nodeType: string, content: string): Node {
  return {
    id,
    nodeType,
    content,
    version: 1,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    properties: {},
    lifecycleStatus: 'active',
    mentions: []
  } as unknown as Node;
}

function tabNodeIds(): (string | undefined)[] {
  return navigationStore.state.tabs.map((t) => t.content?.nodeId);
}

describe('opening a chat message', () => {
  beforeEach(() => {
    navigationStore.resetTabState();
    sharedNodeStore.setNode(node(CHAT_ID, 'ai-chat-native', 'Planning chat'), databaseSource);
    sharedNodeStore.setNode(node(MESSAGE_ID, 'ai-chat-message', 'Remember the deadline'), databaseSource);
    vi.stubGlobal('requestAnimationFrame', (cb: () => void) => {
      cb();
      return 0;
    });
  });

  afterEach(() => {
    structureTree.removeNode(MESSAGE_ID);
    structureTree.removeNode(CHAT_ID);
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('opens the parent chat in a new tab, never the message', async () => {
    structureTree.addInMemoryRelationship(CHAT_ID, MESSAGE_ID, 1);

    await getNavigationService().navigateToNode(MESSAGE_ID, true);

    const ids = tabNodeIds();
    expect(ids).toContain(CHAT_ID);
    expect(ids).not.toContain(MESSAGE_ID);
    const chatTab = navigationStore.state.tabs.find((t) => t.content?.nodeId === CHAT_ID);
    expect(chatTab?.content?.nodeType).toBe('ai-chat-native');
  });

  it('re-points the current tab at the parent chat on a plain navigation', async () => {
    structureTree.addInMemoryRelationship(CHAT_ID, MESSAGE_ID, 1);
    const activeTabId = navigationStore.state.activeTabIds[navigationStore.state.activePaneId];

    await getNavigationService().navigateToNode(MESSAGE_ID, false);

    const active = navigationStore.state.tabs.find((t) => t.id === activeTabId);
    expect(active?.content?.nodeId).toBe(CHAT_ID);
    expect(tabNodeIds()).not.toContain(MESSAGE_ID);
  });

  it('scrolls to the message element once the chat renders it', async () => {
    structureTree.addInMemoryRelationship(CHAT_ID, MESSAGE_ID, 1);
    const element = document.createElement('div');
    element.setAttribute('data-message-id', MESSAGE_ID);
    element.scrollIntoView = vi.fn();
    document.body.appendChild(element);

    try {
      await getNavigationService().navigateToNode(MESSAGE_ID, true);
      await vi.waitFor(() => expect(element.scrollIntoView).toHaveBeenCalled());
    } finally {
      element.remove();
    }
  });

  it('opens nothing when neither the tree nor the backend knows the parent', async () => {
    const getParent = vi.spyOn(backendAdapter, 'getParent').mockResolvedValue(null);
    const before = tabNodeIds();

    await getNavigationService().navigateToNode(MESSAGE_ID, true);

    expect(getParent).toHaveBeenCalledWith(MESSAGE_ID);
    expect(tabNodeIds()).toEqual(before);
  });

  it('asks the backend for the parent of a message the tree does not know', async () => {
    const getParent = vi
      .spyOn(backendAdapter, 'getParent')
      .mockResolvedValue({ id: CHAT_ID, title: null, nodeType: 'ai-chat-native' });

    await getNavigationService().navigateToNode(MESSAGE_ID, true);

    expect(getParent).toHaveBeenCalledWith(MESSAGE_ID);
    expect(tabNodeIds()).toContain(CHAT_ID);
    expect(tabNodeIds()).not.toContain(MESSAGE_ID);
  });

  it('does not ask the backend when the tree already knows the parent', async () => {
    structureTree.addInMemoryRelationship(CHAT_ID, MESSAGE_ID, 1);
    const getParent = vi.spyOn(backendAdapter, 'getParent');

    await getNavigationService().navigateToNode(MESSAGE_ID, true);

    expect(getParent).not.toHaveBeenCalled();
  });

  it('fetches a message that is not in the store before resolving its parent', async () => {
    structureTree.addInMemoryRelationship(CHAT_ID, 'uncached-message', 1);
    const getNode = vi
      .spyOn(backendAdapter, 'getNode')
      .mockResolvedValue(node('uncached-message', 'ai-chat-message', 'fetched'));

    try {
      await getNavigationService().navigateToNode('uncached-message', true);

      expect(getNode).toHaveBeenCalledWith('uncached-message');
      expect(tabNodeIds()).toContain(CHAT_ID);
      expect(tabNodeIds()).not.toContain('uncached-message');
    } finally {
      structureTree.removeNode('uncached-message');
    }
  });

  it('re-points a tab that already carries a message id at its chat', async () => {
    structureTree.addInMemoryRelationship(CHAT_ID, MESSAGE_ID, 1);
    const tabId = navigationStore.state.activeTabIds[DEFAULT_PANE_ID];
    navigationStore.updateTabContent(tabId, { nodeId: MESSAGE_ID, nodeType: 'ai-chat-message' });

    await getNavigationService().retargetTabToParent(tabId, MESSAGE_ID);

    const tab = navigationStore.state.tabs.find((t) => t.id === tabId);
    expect(tab?.content).toEqual({ nodeId: CHAT_ID, nodeType: 'ai-chat-native' });
  });

  it('closes a tab carrying a message id whose parent is not known', async () => {
    vi.spyOn(backendAdapter, 'getParent').mockResolvedValue(null);
    addTab({
      id: 'message-tab',
      title: 'Message',
      type: 'node',
      content: { nodeId: MESSAGE_ID, nodeType: 'ai-chat-message' },
      closeable: true,
      paneId: DEFAULT_PANE_ID
    });

    await getNavigationService().retargetTabToParent('message-tab', MESSAGE_ID);

    expect(navigationStore.state.tabs.some((t) => t.id === 'message-tab')).toBe(false);
  });
});
