import { describe, it, expect, vi, beforeEach } from 'vitest';

const getNode = vi.fn();
vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: { getNode: (...a: unknown[]) => getNode(...a) }
}));
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import { isTabResolvable } from '$lib/services/tab-restore-validation';
import type { Tab } from '$lib/stores/navigation.svelte';

const tab = (over: Partial<Tab>): Tab => ({
  id: 't',
  title: 't',
  type: 'node',
  closeable: true,
  paneId: 'pane-1',
  ...over
});

describe('isTabResolvable', () => {
  beforeEach(() => {
    getNode.mockReset();
  });

  it('drops a node tab whose node does not exist', async () => {
    getNode.mockResolvedValue(null);
    expect(await isTabResolvable(tab({ content: { nodeId: 'gone', nodeType: 'text' } }))).toBe(false);
  });

  it('keeps a node tab whose node exists', async () => {
    getNode.mockResolvedValue({ id: 'n' });
    expect(await isTabResolvable(tab({ content: { nodeId: 'n', nodeType: 'text' } }))).toBe(true);
  });

  it('keeps a type view tab when the type exists and drops it when stale', async () => {
    getNode.mockImplementation(async (id: string) => (id === 'task' ? { id } : null));
    expect(await isTabResolvable(tab({ content: { nodeId: 'task', nodeType: 'query' } }))).toBe(true);
    expect(await isTabResolvable(tab({ content: { nodeId: 'old', nodeType: 'query' } }))).toBe(false);
  });

  it('keeps settings, search and date tabs without asking the daemon', async () => {
    expect(await isTabResolvable(tab({ type: 'settings' }))).toBe(true);
    expect(await isTabResolvable(tab({ type: 'search' }))).toBe(true);
    expect(await isTabResolvable(tab({ content: { nodeId: '2026-01-02', nodeType: 'date' } }))).toBe(true);
    expect(getNode).not.toHaveBeenCalled();
  });

  it('keeps a tab when the daemon cannot answer', async () => {
    getNode.mockImplementation(() => Promise.reject(new Error('no database registered')));
    expect(await isTabResolvable(tab({ content: { nodeId: 'n', nodeType: 'text' } }))).toBe(true);
  });
});
