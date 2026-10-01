/**
 * NodeRefPreview controller tests
 *
 * Covers the pure title/snippet builders and the show/hide state machine
 * (delay, cache-first resolution, not-found fallback, and async-race guarding).
 * Follows the singleton-spy pattern (vi.spyOn on the real sharedNodeStore),
 * never vi.mock, so nothing leaks across files under the forks pool.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  nodeRefPreview,
  buildPreviewTitle,
  buildPreviewSnippet,
  PREVIEW_DELAY_MS,
  SNIPPET_MAX_LENGTH,
  PREVIEW_CARD_ID
} from '../../lib/services/node-ref-preview.svelte';
import { sharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import type { Node } from '../../lib/types';

function makeNode(overrides: Partial<Node> = {}): Node {
  return {
    lifecycleStatus: 'active',
    id: 'node-1',
    nodeType: 'text',
    content: 'Body content',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    mentions: [],
    ...overrides
  };
}

function anchor(href: string): HTMLElement {
  const a = document.createElement('a');
  a.className = 'ns-noderef ns-noderef-valid';
  a.setAttribute('href', href);
  return a;
}

describe('buildPreviewTitle', () => {
  it('prefers the indexed title', () => {
    expect(buildPreviewTitle(makeNode({ title: '  My Title  ', content: 'x' }))).toBe('My Title');
  });

  it('falls back to the first content line when no title', () => {
    expect(buildPreviewTitle(makeNode({ title: null, content: 'First line\nSecond line' }))).toBe(
      'First line'
    );
  });

  it('is empty when there is no title and no content', () => {
    expect(buildPreviewTitle(makeNode({ title: null, content: '' }))).toBe('');
  });
});

describe('buildPreviewSnippet', () => {
  it('collapses whitespace of the full content when a title exists', () => {
    const node = makeNode({ title: 'T', content: 'line one\n\n  line   two ' });
    expect(buildPreviewSnippet(node)).toBe('line one line two');
  });

  it('drops the first line (used as title) when the node has no title', () => {
    const node = makeNode({ title: null, content: 'Title line\nreal body here' });
    expect(buildPreviewSnippet(node)).toBe('real body here');
  });

  it('returns empty when a title-less node has only one line', () => {
    expect(buildPreviewSnippet(makeNode({ title: null, content: 'only a title' }))).toBe('');
  });

  it('truncates long content with an ellipsis', () => {
    const long = 'a'.repeat(SNIPPET_MAX_LENGTH + 50);
    const result = buildPreviewSnippet(makeNode({ title: 'T', content: long }));
    expect(result.endsWith('…')).toBe(true);
    expect(result.length).toBe(SNIPPET_MAX_LENGTH + 1);
  });

  it('is empty for empty content', () => {
    expect(buildPreviewSnippet(makeNode({ title: 'T', content: '' }))).toBe('');
  });
});

describe('nodeRefPreview controller', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    nodeRefPreview.hide();
  });

  afterEach(() => {
    nodeRefPreview.hide();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('reveals a cached node after the delay', async () => {
    const node = makeNode({ id: 'abc', title: 'Cached', content: 'Cached\nsnippet body' });
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(node);
    const ensure = vi.spyOn(sharedNodeStore, 'ensureNode');

    nodeRefPreview.requestPreview(anchor('nodespace://abc'));
    expect(nodeRefPreview.state.visible).toBe(false); // still within the delay

    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);

    expect(nodeRefPreview.state.visible).toBe(true);
    expect(nodeRefPreview.state.loading).toBe(false);
    expect(nodeRefPreview.state.notFound).toBe(false);
    expect(nodeRefPreview.state.title).toBe('Cached');
    expect(nodeRefPreview.state.snippet).toBe('snippet body');
    expect(ensure).not.toHaveBeenCalled(); // cache hit skips the fetch
  });

  it('re-confirms a cached-but-possibly-stale node instead of trusting the cache', async () => {
    // A daemon reconnect can leave a cached node's title/snippet stale (a
    // WatchNodes outage silently drops the update that would have kept it
    // current) — the preview must re-fetch, not just serve the cache hit.
    const staleCached = makeNode({ id: 'abc', title: 'Stale', content: 'Stale\nold body' });
    const refreshed = makeNode({ id: 'abc', title: 'Fresh', content: 'Fresh\nnew body' });
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(staleCached);
    vi.spyOn(sharedNodeStore, 'isPossiblyStale').mockReturnValue(true);
    const ensure = vi.spyOn(sharedNodeStore, 'ensureNode').mockResolvedValue(refreshed);

    nodeRefPreview.requestPreview(anchor('nodespace://abc'));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);

    expect(ensure).toHaveBeenCalledWith('abc');
    expect(nodeRefPreview.state.title).toBe('Fresh');
    expect(nodeRefPreview.state.snippet).toBe('new body');
  });

  it('fetches an uncached node then reveals it', async () => {
    const node = makeNode({ id: 'xyz', title: 'Fetched', content: 'Fetched\nfrom store' });
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(undefined);
    vi.spyOn(sharedNodeStore, 'ensureNode').mockResolvedValue(node);

    nodeRefPreview.requestPreview(anchor('nodespace://xyz'));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);

    expect(nodeRefPreview.state.visible).toBe(true);
    expect(nodeRefPreview.state.title).toBe('Fetched');
    expect(nodeRefPreview.state.snippet).toBe('from store');
  });

  it('shows a not-found state when the node cannot be resolved', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(undefined);
    vi.spyOn(sharedNodeStore, 'ensureNode').mockResolvedValue(undefined);

    nodeRefPreview.requestPreview(anchor('nodespace://missing'));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);

    expect(nodeRefPreview.state.visible).toBe(true);
    expect(nodeRefPreview.state.notFound).toBe(true);
    expect(nodeRefPreview.state.nodeId).toBe('missing');
  });

  it('ignores anchors that are not nodespace references', async () => {
    const getNode = vi.spyOn(sharedNodeStore, 'getNode');
    nodeRefPreview.requestPreview(anchor('https://example.com'));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(nodeRefPreview.state.visible).toBe(false);
    expect(getNode).not.toHaveBeenCalled();
  });

  it('ignores deleted (broken) references', async () => {
    const getNode = vi.spyOn(sharedNodeStore, 'getNode');
    nodeRefPreview.requestPreview(anchor('nodespace://gone?deleted=true'));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(nodeRefPreview.state.visible).toBe(false);
    expect(getNode).not.toHaveBeenCalled();
  });

  it('does not reset the delay on repeated requests for the same reference', async () => {
    // Crossing nested spans re-fires requestPreview; the timer must keep counting
    // from the first request, not restart, or the card never appears.
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(makeNode({ id: 'abc', title: 'X' }));
    const a = anchor('nodespace://abc');

    nodeRefPreview.requestPreview(a);
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS - 100);
    nodeRefPreview.requestPreview(a); // repeat mid-delay
    await vi.advanceTimersByTimeAsync(100); // original deadline

    expect(nodeRefPreview.state.visible).toBe(true);
  });

  it('links the anchor to the card via aria-describedby while shown, and clears it on hide', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(makeNode({ id: 'abc', title: 'X' }));
    const a = anchor('nodespace://abc');

    nodeRefPreview.requestPreview(a);
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(a.getAttribute('aria-describedby')).toBe(PREVIEW_CARD_ID);

    nodeRefPreview.hide();
    expect(a.hasAttribute('aria-describedby')).toBe(false);
  });

  it('moves aria-describedby off the old anchor when re-anchoring to the same node', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(makeNode({ id: 'abc', title: 'X' }));
    const a = anchor('nodespace://abc');
    const b = anchor('nodespace://abc');

    nodeRefPreview.requestPreview(a);
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(a.getAttribute('aria-describedby')).toBe(PREVIEW_CARD_ID);

    // Same id, different anchor: re-anchors rather than short-circuiting.
    nodeRefPreview.requestPreview(b);
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(a.hasAttribute('aria-describedby')).toBe(false);
    expect(b.getAttribute('aria-describedby')).toBe(PREVIEW_CARD_ID);
    expect(nodeRefPreview.state.anchor).toBe(b);
  });

  it('cancels a pending reveal when hidden before the delay elapses', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(makeNode());
    nodeRefPreview.requestPreview(anchor('nodespace://abc'));
    nodeRefPreview.hide();
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS);
    expect(nodeRefPreview.state.visible).toBe(false);
  });

  it('does not resurrect a card hidden mid-fetch (async race)', async () => {
    let resolveFetch: (n: Node | undefined) => void = () => {};
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(undefined);
    vi.spyOn(sharedNodeStore, 'ensureNode').mockReturnValue(
      new Promise((resolve) => {
        resolveFetch = resolve;
      })
    );

    nodeRefPreview.requestPreview(anchor('nodespace://slow'));
    await vi.advanceTimersByTimeAsync(PREVIEW_DELAY_MS); // fires reveal → awaits fetch
    // User moves away before the fetch settles.
    nodeRefPreview.hide();
    resolveFetch(makeNode({ id: 'slow', title: 'Too late' }));
    await Promise.resolve();
    await Promise.resolve();

    expect(nodeRefPreview.state.visible).toBe(false);
    expect(nodeRefPreview.state.title).toBe('');
  });
});
