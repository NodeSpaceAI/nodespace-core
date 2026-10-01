/**
 * NodeCardInline — on-mount fetch (ADR-049) and the link's text.
 *
 * The card previously fetched a missing node from inside a $effect that watched the
 * derived `node` value. After the ADR-049 conversion it fetches once on mount (the card
 * is mounted imperatively with a fixed nodeId), skipping the fetch entirely when the node
 * is already present in the store. These tests pin that single-call-site behaviour.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const getNode = vi.fn();
const setNode = vi.fn();
const fetchNode = vi.fn();
const currentEpoch = vi.fn().mockReturnValue(0);
const pinNodes = vi.fn();
const unpinAll = vi.fn();

vi.mock('$lib/services/shared-node-store.svelte', () => ({
  sharedNodeStore: {
    getNode: (...a: unknown[]) => getNode(...a),
    setNode: (...a: unknown[]) => setNode(...a),
    // ADR-053 epoch guard: the on-mount fetch captures currentEpoch() and
    // re-checks it before setNode. A stable value keeps the read in-epoch.
    currentEpoch: (...a: unknown[]) => currentEpoch(...a),
    // The card pins its nodeId reachable for as long as it's mounted (see
    // pin-node-reachability.ts) — exercised by these mocks, not asserted on
    // directly; the eviction/pin mechanism itself is unit-tested in
    // shared-node-store-eviction.test.ts.
    pinNodes: (...a: unknown[]) => pinNodes(...a),
    unpinAll: (...a: unknown[]) => unpinAll(...a)
  }
}));

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getNode: (...a: unknown[]) => fetchNode(...a)
  }
}));

import NodeCardInline from '$lib/components/chat/node-card-inline.svelte';

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('NodeCardInline on-mount fetch', () => {
  it('fetches a missing node exactly once on mount and stores the result', async () => {
    const fetched = { id: 'abc-123', nodeType: 'text', content: 'Hello', properties: {} };
    getNode.mockReturnValue(undefined); // not in store
    fetchNode.mockResolvedValue(fetched);

    render(NodeCardInline, { nodeId: 'abc-123' });
    await tick();
    await tick();

    expect(fetchNode).toHaveBeenCalledTimes(1);
    expect(fetchNode).toHaveBeenCalledWith('abc-123');
    expect(setNode).toHaveBeenCalledWith(
      fetched,
      { type: 'database', reason: 'node-card-fetch' },
      true
    );
  });

  it('does not fetch when the node is already in the store', async () => {
    getNode.mockReturnValue({ id: 'abc-123', nodeType: 'text', content: 'Cached', properties: {} });

    render(NodeCardInline, { nodeId: 'abc-123' });
    await tick();
    await tick();

    expect(fetchNode).not.toHaveBeenCalled();
    expect(setNode).not.toHaveBeenCalled();
  });

  it('does not store anything when the backend has no such node', async () => {
    getNode.mockReturnValue(undefined);
    fetchNode.mockResolvedValue(undefined);

    render(NodeCardInline, { nodeId: 'missing' });
    await tick();
    await tick();

    expect(fetchNode).toHaveBeenCalledTimes(1);
    expect(setNode).not.toHaveBeenCalled();
  });
});

describe('NodeCardInline title', () => {
  function cardTitle(container: HTMLElement): string | null | undefined {
    return container.querySelector('.ns-node-card-inline')?.textContent;
  }

  it("shows the node's live title, not the agent's label", async () => {
    getNode.mockReturnValue({
      id: 'abc-123',
      nodeType: 'text',
      content: 'Data Layer',
      properties: {}
    });

    const { container } = render(NodeCardInline, { nodeId: 'abc-123', displayText: 'Old Name' });
    await tick();

    expect(cardTitle(container)).toBe('Data Layer');
  });

  it('shows a header without its leading # markers', async () => {
    getNode.mockReturnValue({
      id: 'abc-123',
      nodeType: 'header',
      content: '## Data Layer',
      properties: {}
    });

    const { container } = render(NodeCardInline, { nodeId: 'abc-123', displayText: 'label' });
    await tick();

    expect(cardTitle(container)).toBe('Data Layer');
  });

  it('shows the computed title of a title-template type', async () => {
    getNode.mockReturnValue({
      id: 'abc-123',
      nodeType: 'person',
      content: '',
      title: 'Ada Lovelace',
      properties: {}
    });

    const { container } = render(NodeCardInline, { nodeId: 'abc-123', displayText: 'Ada' });
    await tick();

    expect(cardTitle(container)).toBe('Ada Lovelace');
  });

  it("does not let the agent's label stand in for a resolved node with no title", async () => {
    getNode.mockReturnValue({ id: 'abc-123', nodeType: 'text', content: '', properties: {} });

    const { container } = render(NodeCardInline, { nodeId: 'abc-123', displayText: 'Made Up' });
    await tick();

    expect(cardTitle(container)).toBe('Untitled');
  });

  it("shows the agent's label while the node is loading", async () => {
    getNode.mockReturnValue(undefined);
    fetchNode.mockReturnValue(new Promise(() => {}));

    const { container } = render(NodeCardInline, { nodeId: 'abc-123', displayText: 'Data Layer' });
    await tick();

    expect(cardTitle(container)).toBe('Data Layer');
    expect(container.querySelector('.ns-node-card-inline--missing')).toBeNull();
  });

  it("keeps the agent's label, marked missing, when the node does not exist", async () => {
    getNode.mockReturnValue(undefined);
    fetchNode.mockResolvedValue(undefined);

    const { container } = render(NodeCardInline, { nodeId: 'missing', displayText: 'Data Layer' });
    await tick();
    await tick();
    await tick();

    expect(cardTitle(container)).toBe('Data Layer');
    expect(container.querySelector('.ns-node-card-inline--missing')).not.toBeNull();
  });

  it('is a plain link: no icon, type badge or task status', async () => {
    getNode.mockReturnValue({
      id: 'abc-123',
      nodeType: 'task',
      content: 'Ship the release',
      status: 'in_progress',
      properties: {}
    });

    const { container } = render(NodeCardInline, { nodeId: 'abc-123' });
    await tick();

    const link = container.querySelector('a.ns-node-card-inline');
    expect(link?.getAttribute('href')).toBe('nodespace://abc-123');
    expect(link?.children).toHaveLength(0);
    expect(link?.textContent).toBe('Ship the release');
  });
});
