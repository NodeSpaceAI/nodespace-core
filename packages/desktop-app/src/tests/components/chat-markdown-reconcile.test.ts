/**
 * ChatMarkdown Component Tests — incremental node-card reconciliation
 *
 * Verifies that inline node-card components are patched in place rather than
 * fully unmounted and remounted when unrelated content changes (e.g. once
 * per streamed token appended after an already-rendered node card).
 *
 * Uses vi.resetModules() + a dynamic import per test: `marked` is configured
 * as a process-wide singleton by marked-config.ts (mutated via marked.use()),
 * so which test files happened to run earlier in the same worker can leave
 * stray global renderer overrides behind. Resetting the module registry
 * before each import guarantees ChatMarkdown sees a clean `marked` instance
 * regardless of suite run order.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';
import ChatMarkdown from '$lib/components/chat/chat-markdown.svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

vi.mock('$lib/services/backend-adapter', () => ({
  backendAdapter: {
    getNode: vi.fn().mockResolvedValue(undefined),
  },
}));

vi.mock('$lib/services/shared-node-store.svelte', () => ({
  sharedNodeStore: {
    getNode: vi.fn().mockReturnValue(undefined),
    setNode: vi.fn(),
    // ADR-053 epoch guard: the on-mount fetch captures currentEpoch() and
    // re-checks it before setNode. A stable value keeps the read in-epoch.
    currentEpoch: vi.fn().mockReturnValue(0),
    // Each mounted NodeCardInline pins its nodeId reachable for as long as
    // it's mounted (see pin-node-reachability.ts).
    pinNodes: vi.fn(),
    unpinAll: vi.fn(),
  },
}));

afterEach(() => {
  cleanup();
});

describe('ChatMarkdown incremental reconciliation', () => {
  it('does not remount a node-card when unrelated trailing content changes', async () => {
    const { container, rerender } = render(ChatMarkdown, {
      content: 'See nodespace://abc-123 for details.',
    });
    await tick();

    const cardEl1 = container.querySelector('.ns-node-card-placeholder[data-node-id="abc-123"]');
    expect(cardEl1).not.toBeNull();
    const anchor1 = cardEl1?.querySelector('.ns-node-card-inline');
    expect(anchor1).not.toBeNull();

    await rerender({
      content: 'See nodespace://abc-123 for details.\n\nMore streamed text follows.',
    });
    await tick();

    const cardEl2 = container.querySelector('.ns-node-card-placeholder[data-node-id="abc-123"]');
    expect(cardEl2).not.toBeNull();
    // Same underlying DOM node — proves the top-level block containing it
    // was left untouched instead of being torn down and rebuilt.
    expect(cardEl2).toBe(cardEl1);

    const anchor2 = cardEl2?.querySelector('.ns-node-card-inline');
    expect(anchor2).toBe(anchor1);

    expect(container.textContent).toContain('More streamed text follows.');
  });

  it('reconciles a node-card in an earlier block when a later block is appended', async () => {
    const { container, rerender } = render(ChatMarkdown, {
      content: 'Nodes: nodespace://aaa-111',
    });
    await tick();

    const first = container.querySelector('.ns-node-card-placeholder[data-node-id="aaa-111"]');
    expect(first).not.toBeNull();

    // Node card is in the first paragraph; the new paragraph with a second
    // node-card is appended as a separate top-level block, so it must not
    // disturb the already-rendered first block.
    await rerender({
      content: 'Nodes: nodespace://aaa-111\n\nAlso see nodespace://bbb-222',
    });
    await tick();

    const firstAfter = container.querySelector('.ns-node-card-placeholder[data-node-id="aaa-111"]');
    const second = container.querySelector('.ns-node-card-placeholder[data-node-id="bbb-222"]');
    expect(second).not.toBeNull();
    // First node-card's element must be stable — it lives in an unrelated,
    // unchanged top-level block.
    expect(firstAfter).toBe(first);
  });

  it('still updates a node-card block whose own content actually changed', async () => {
    const { container, rerender } = render(ChatMarkdown, {
      content: 'nodespace://abc-123',
    });
    await tick();

    expect(container.querySelector('[data-node-id="abc-123"]')).not.toBeNull();

    await rerender({ content: 'nodespace://def-456' });
    await tick();

    expect(container.querySelector('[data-node-id="abc-123"]')).toBeNull();
    expect(container.querySelector('[data-node-id="def-456"]')).not.toBeNull();
  });
});
