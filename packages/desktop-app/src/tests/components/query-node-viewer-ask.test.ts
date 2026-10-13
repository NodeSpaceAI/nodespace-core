/**
 * QueryNodeViewer "Ask": a saved query offers a chat bound to it (ADR-100 §1),
 * beside the controls the view already has. A type's own page offers one bound
 * to the type (ADR-100 §5), once the type's schema is there to bind to.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const navigateToNodeInOtherPane = vi.fn();
vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({
    focusNodeTab: () => false,
    navigateToNodeInOtherPane: (...args: unknown[]) => navigateToNodeInOtherPane(...args)
  })
}));

import QueryNodeViewer from '$lib/components/viewers/query-node-viewer.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { aiChatsData } from '$lib/stores/ai-chats.svelte';

const QUERY_ID = 'saved-query-1';

const savedQuery = {
  lifecycleStatus: 'active',
  id: QUERY_ID,
  nodeType: 'query',
  content: 'Open tasks',
  createdAt: '2026-01-01T00:00:00Z',
  modifiedAt: '2026-01-01T00:00:00Z',
  version: 1,
  properties: {},
  mentions: [],
  targetType: 'task',
  filters: [],
  generatedBy: 'user',
  executionCount: 0
} as unknown as Node;

describe('QueryNodeViewer — Ask', () => {
  beforeEach(() => {
    localStorage.clear();
    sharedNodeStore.clearAll();
    navigateToNodeInOtherPane.mockClear();
    vi.spyOn(backendAdapter, 'getNode').mockImplementation(async (id: string) =>
      id === QUERY_ID ? savedQuery : null
    );
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(null as never);
    vi.spyOn(backendAdapter, 'queryNodes').mockResolvedValue([]);
    vi.spyOn(backendAdapter, 'executeQuery').mockResolvedValue([]);
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
    vi.restoreAllMocks();
  });

  it('opens a chat bound to the saved query beside the view', async () => {
    const chat = { id: 'chat-1' } as Node;
    const open = vi.spyOn(aiChatsData, 'openBoundChat').mockResolvedValue(chat);
    const { getByRole } = render(QueryNodeViewer, {
      props: { nodeId: QUERY_ID, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByRole('button', { name: 'Ask' })).toBeTruthy());

    await fireEvent.click(getByRole('button', { name: 'Ask' }));

    await waitFor(() => expect(navigateToNodeInOtherPane).toHaveBeenCalledWith('chat-1'));
    expect(open).toHaveBeenCalledWith(QUERY_ID);
  });

  it("offers the type's own page a chat bound to the type", async () => {
    const schema = {
      id: 'task',
      nodeType: 'schema',
      content: 'Task',
      isCore: true,
      schemaVersion: 1,
      fields: [],
      relationships: [],
      version: 1,
      properties: {}
    };
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(schema as never);
    const chat = { id: 'chat-2' } as Node;
    const open = vi.spyOn(aiChatsData, 'openBoundChat').mockResolvedValue(chat);
    const { getByRole } = render(QueryNodeViewer, {
      props: { nodeId: 'task', onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByRole('button', { name: 'Ask' })).toBeTruthy());

    await fireEvent.click(getByRole('button', { name: 'Ask' }));

    await waitFor(() => expect(navigateToNodeInOtherPane).toHaveBeenCalledWith('chat-2'));
    expect(open).toHaveBeenCalledWith('task');
  });

  it('offers a type that has no schema yet no chat', async () => {
    const { queryByRole, getByRole } = render(QueryNodeViewer, {
      props: { nodeId: 'not-yet-a-type', onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByRole('button', { name: 'List' })).toBeTruthy());
    expect(queryByRole('button', { name: 'Ask' })).toBeNull();
  });

  it('keeps the view controls beside it', async () => {
    const { getByRole } = render(QueryNodeViewer, {
      props: { nodeId: QUERY_ID, onNodeIdChange: () => {} }
    });
    await waitFor(() => expect(getByRole('button', { name: 'Ask' })).toBeTruthy());
    for (const name of ['List', 'Table', 'Kanban']) {
      expect(getByRole('button', { name })).toBeTruthy();
    }
  });
});
