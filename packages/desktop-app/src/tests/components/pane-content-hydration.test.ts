/**
 * Regression tests for the pane-content.svelte hydration behaviour.
 *
 * Full-component-mount tests for BaseNodeViewer-adjacent components require complex setup
 * (NodeServiceContext, plugin registry, database mocking - see merge-prevention.test.ts).
 * Instead these tests exercise the hydration logic extracted from pane-content.svelte's
 * hydrateNode(), driven with real async timing via a controllable mock store, to verify the
 * event-driven conversion (ADR-049): hydration writes only into the store (one cell per
 * nodeId), reads its status back off the store, and closes a not-found tab only if that tab
 * still points at the fetched nodeId.
 *
 * The earlier `latestRequestedNodeId` staleness guard is gone: because hydration state lives
 * on the store keyed by nodeId — not a single shared local mirror — a stale in-flight fetch
 * simply populates its own cell and can no longer overwrite the current node's state.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

interface Node {
  id: string;
}

/**
 * Mirrors hydrateNode() in pane-content.svelte after the ADR-049 conversion:
 * fetch-if-missing into the store; on not-found, close the tab only if it still
 * points at this nodeId. `getNode` reads the store's per-node cell — the single
 * source of truth the component's isNodeHydrated $derived reads too.
 */
function createHydrationHarness(
  ensureNode: (nodeId: string) => Promise<Node | undefined>,
  tabNodeId: (tabId: string) => string | undefined
) {
  const store = new Map<string, Node>();
  const closedTabs: string[] = [];
  const errors: unknown[] = [];

  async function hydrateNode(nodeId: string, tabId: string) {
    if (store.has(nodeId)) return;

    let node: Node | undefined;
    try {
      node = await ensureNode(nodeId);
    } catch (error) {
      errors.push(error);
      return;
    }

    if (node) {
      store.set(node.id, node);
      return;
    }

    // Not found — close the tab only if it still points at this nodeId.
    if (tabNodeId(tabId) === nodeId) {
      closedTabs.push(tabId);
    }
  }

  return {
    hydrateNode,
    isHydrated: (nodeId: string) => store.has(nodeId),
    closedTabs,
    errors
  };
}

describe('pane-content hydration (event-driven)', () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  it('does not throw or leave an unhandled rejection when ensureNode fails', async () => {
    const ensureNode = vi.fn().mockRejectedValue(new TypeError('Load failed'));
    const harness = createHydrationHarness(ensureNode, () => '2026-07-07');

    await expect(harness.hydrateNode('2026-07-07', 'tab-1')).resolves.toBeUndefined();
    expect(harness.errors).toHaveLength(1);
    expect(harness.isHydrated('2026-07-07')).toBe(false);
  });

  it('a stale fetch resolving late populates only its own store cell — it cannot overwrite the current node', async () => {
    const resolvers = new Map<string, (node: Node | undefined) => void>();
    const ensureNode = vi.fn(
      (nodeId: string) =>
        new Promise<Node | undefined>((resolve) => {
          resolvers.set(nodeId, resolve);
        })
    );
    // Tab currently shows the later-requested date.
    const harness = createHydrationHarness(ensureNode, () => '2026-07-07');

    // Rapid prev/next: request 07-08 (slow) then 07-07 (newer).
    const firstCall = harness.hydrateNode('2026-07-08', 'tab-1');
    const secondCall = harness.hydrateNode('2026-07-07', 'tab-1');

    // Resolve newer first, then the stale one — out-of-order, the stale-hydration repro.
    resolvers.get('2026-07-07')?.({ id: '2026-07-07' });
    await secondCall;
    resolvers.get('2026-07-08')?.({ id: '2026-07-08' });
    await firstCall;

    // Both cells populate, but they are independent — the current node (07-07) is hydrated
    // and its state was never clobbered by the late 07-08 resolution. The component reads
    // getNode(currentNodeId), so it always sees the right node regardless of resolve order.
    expect(harness.isHydrated('2026-07-07')).toBe(true);
    expect(harness.isHydrated('2026-07-08')).toBe(true);
  });

  it("closes the tab only if the not-found response is still the tab's current nodeId", async () => {
    const resolvers = new Map<string, (node: Node | undefined) => void>();
    const ensureNode = vi.fn(
      (nodeId: string) =>
        new Promise<Node | undefined>((resolve) => {
          resolvers.set(nodeId, resolve);
        })
    );
    // The tab has navigated on to 2026-07-07 by the time the stale fetch settles.
    const harness = createHydrationHarness(ensureNode, () => '2026-07-07');

    const staleCall = harness.hydrateNode('deleted-node', 'tab-1');
    const freshCall = harness.hydrateNode('2026-07-07', 'tab-1');

    resolvers.get('2026-07-07')?.({ id: '2026-07-07' });
    await freshCall;
    // Stale "not found" resolves after the tab moved on — must NOT close the tab.
    resolvers.get('deleted-node')?.(undefined);
    await staleCall;

    expect(harness.closedTabs).toEqual([]);
    expect(harness.isHydrated('2026-07-07')).toBe(true);
  });

  it('closes the tab when the current node is genuinely not found', async () => {
    const ensureNode = vi.fn().mockResolvedValue(undefined);
    // Tab still points at the missing node.
    const harness = createHydrationHarness(ensureNode, () => 'missing-node');

    await harness.hydrateNode('missing-node', 'tab-1');

    expect(harness.closedTabs).toEqual(['tab-1']);
  });

  it('repeated rapid navigation across many dates hydrates every visited node independently', async () => {
    const resolvers = new Map<string, (node: Node | undefined) => void>();
    const ensureNode = vi.fn(
      (nodeId: string) =>
        new Promise<Node | undefined>((resolve) => {
          resolvers.set(nodeId, resolve);
        })
    );
    const harness = createHydrationHarness(ensureNode, () => '2026-07-09');

    const dates = ['2026-07-05', '2026-07-06', '2026-07-07', '2026-07-08', '2026-07-09'];
    const calls = dates.map((date) => harness.hydrateNode(date, 'tab-1'));

    // Resolve out of order, simulating an unreliable dev-proxy connection.
    for (const date of [...dates].reverse()) {
      resolvers.get(date)?.({ id: date });
    }
    await Promise.all(calls);

    // Every visited date hydrates its own cell; the component displays getNode(currentNodeId),
    // so the last-navigated date (07-09) is what renders — no stale winner.
    expect(dates.every((date) => harness.isHydrated(date))).toBe(true);
  });

  it('skips the fetch entirely when the node is already in the store', async () => {
    const ensureNode = vi.fn().mockResolvedValue({ id: '2026-07-07' });
    const harness = createHydrationHarness(ensureNode, () => '2026-07-07');

    await harness.hydrateNode('2026-07-07', 'tab-1'); // populates the cell
    await harness.hydrateNode('2026-07-07', 'tab-1'); // second call is a no-op

    expect(ensureNode).toHaveBeenCalledTimes(1);
  });

  it('a hydration that failed during the daemon boot window succeeds on the reconnect retry', async () => {
    // First fetch races the daemon's startup (fresh install: the webview renders
    // while the sidecar is still seeding its DB and binding the socket) → the
    // invoke rejects with a transport error, not a clean not-found. Without a
    // retry the pane sits on "Loading..." forever — the round-1 fresh-install
    // hang. pane-content now re-runs hydrateNode via onDaemonReconnect; this
    // mirrors that wiring: retry only when the node is still absent.
    const ensureNode = vi
      .fn()
      .mockRejectedValueOnce(new Error('transport error: connection refused'))
      .mockResolvedValue({ id: '2026-07-16' });
    const harness = createHydrationHarness(ensureNode, () => '2026-07-16');

    await harness.hydrateNode('2026-07-16', 'tab-1');
    expect(harness.isHydrated('2026-07-16')).toBe(false);
    expect(harness.errors).toHaveLength(1);

    // Daemon flips not-healthy → healthy; the reconnect listener retries the
    // active tab's hydration iff its node is still missing from the store.
    const onReconnect = async () => {
      if (!harness.isHydrated('2026-07-16')) {
        await harness.hydrateNode('2026-07-16', 'tab-1');
      }
    };
    await onReconnect();

    expect(harness.isHydrated('2026-07-16')).toBe(true);
    expect(ensureNode).toHaveBeenCalledTimes(2);

    // A second healthy transition is a no-op — the store-presence guard holds.
    await onReconnect();
    expect(ensureNode).toHaveBeenCalledTimes(2);
  });
});
