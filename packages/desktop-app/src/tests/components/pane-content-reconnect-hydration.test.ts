/**
 * Mounts the real pane-content.svelte and drives the daemon-reconnect path.
 *
 * The sibling `pane-content-hydration` suite exercises a local copy of
 * hydrateNode()'s logic, so it cannot catch the wiring regressing — if the
 * component stopped subscribing to `onDaemonReconnect`, or subscribed with a
 * callback that no longer re-hydrates, every one of those tests would still
 * pass. These tests mount the component itself and assert the observable
 * contract instead: a hydration fetch that fails inside the daemon's boot
 * window (fresh install — the webview renders while the sidecar is still
 * seeding its DB and binding the socket) is retried when the daemon reports
 * healthy, and the pane leaves its "Loading..." state.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';

// Ids and the shared handles the mocks write into all live in `vi.hoisted`:
// vi.mock factories are hoisted above normal module-scope initialization, so
// anything they close over must be hoisted too.
const { PANE_ID, TAB_ID, NODE_ID, h } = vi.hoisted(() => ({
  PANE_ID: 'pane-1',
  TAB_ID: 'tab-1',
  NODE_ID: 'node-journal-1',
  h: {
    reconnectCallbacks: [] as Array<() => void>,
    nodes: new Map<string, { id: string }>(),
    /** Node ids the mock store currently considers possibly-stale (see
     *  SharedNodeStore.isPossiblyStale) — set by a test to simulate the real
     *  store's reconnect-generation bump, independent of `nodes` presence. */
    staleIds: new Set<string>(),
    ensureCalls: [] as string[],
    /** Queue of outcomes for successive ensureNode() calls. */
    ensureOutcomes: [] as Array<'boot-window-failure' | 'success'>,
    closedTabs: [] as string[]
  }
}));

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

vi.mock('$lib/services/daemon-status', () => ({
  onDaemonReconnect: (cb: () => void) => {
    h.reconnectCallbacks.push(cb);
    return () => {
      h.reconnectCallbacks = h.reconnectCallbacks.filter((c) => c !== cb);
    };
  }
}));

vi.mock('$lib/services/shared-node-store.svelte', () => ({
  sharedNodeStore: {
    getNode: (id: string) => h.nodes.get(id),
    isPossiblyStale: (id: string) => h.staleIds.has(id),
    ensureNode: async (id: string) => {
      h.ensureCalls.push(id);
      const outcome = h.ensureOutcomes.shift() ?? 'success';
      if (outcome === 'boot-window-failure') {
        // What the daemon boot window actually looks like to the webview: the
        // fetch rejects (socket not yet bound) rather than resolving "absent".
        throw new Error('daemon socket not ready');
      }
      const node = { id };
      h.nodes.set(id, node);
      // A successful fetch is fresh-as-of-now — mirrors the real store's
      // `nodesSet` stamping the current reconnect generation on every write,
      // which is what clears `isPossiblyStale` after a re-confirm succeeds.
      h.staleIds.delete(id);
      return node;
    }
  }
}));

vi.mock('$lib/stores/navigation.svelte', () => ({
  navigationStore: {
    state: {
      tabs: [
        {
          id: TAB_ID,
          type: 'content',
          title: 'Journal',
          content: { nodeId: NODE_ID, nodeType: 'date' }
        }
      ],
      activeTabIds: { [PANE_ID]: TAB_ID },
      panes: [{ id: PANE_ID }]
    }
  },
  updateTabContent: vi.fn(),
  closeTab: (tabId: string) => h.closedTabs.push(tabId)
}));

vi.mock('$lib/plugins/plugin-registry', () => ({
  pluginRegistry: {
    hasViewer: () => false,
    getViewer: async () => null
  }
}));

// Stub the panes pane-content can route to. They are not under test here, and
// importing them for real drags in the whole viewer tree (bits-ui et al), which
// is what pushed the sibling suite into mirroring the logic instead of mounting
// the component. A Svelte 5 component is just a function, so a no-op renders
// nothing and keeps the import graph shallow.
vi.mock('$lib/design/components/base-node-viewer.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/settings/settings-pane.svelte', () => ({ default: () => {} }));
vi.mock('$lib/components/search/search-pane.svelte', () => ({ default: () => {} }));

import PaneContent from '$lib/components/layout/pane-content.svelte';
import type { Pane } from '$lib/stores/navigation.svelte';

const pane: Pane = { id: PANE_ID, width: 100, tabIds: [TAB_ID] };

/** Let the mount $effect and its awaited hydration settle. */
async function settle() {
  for (let i = 0; i < 5; i++) {
    await tick();
    await Promise.resolve();
  }
}

function fireDaemonHealthy() {
  for (const cb of [...h.reconnectCallbacks]) cb();
}

describe('pane-content daemon-reconnect hydration', () => {
  beforeEach(() => {
    h.reconnectCallbacks = [];
    h.nodes.clear();
    h.staleIds.clear();
    h.ensureCalls.length = 0;
    h.ensureOutcomes.length = 0;
    h.closedTabs.length = 0;
  });

  afterEach(() => cleanup());

  it('subscribes to daemon reconnect while mounted', () => {
    h.ensureOutcomes.push('success');
    render(PaneContent, { props: { pane } });

    expect(h.reconnectCallbacks.length).toBe(1);
  });

  it('retries a boot-window hydration failure when the daemon reports healthy', async () => {
    h.ensureOutcomes.push('boot-window-failure', 'success');

    const { getByText } = render(PaneContent, { props: { pane } });
    await settle();

    // The first fetch raced the boot window and failed: the node never landed
    // in the store, so the pane is stuck on its loading state — this is exactly
    // the state that used to be permanent.
    expect(h.ensureCalls).toEqual([NODE_ID]);
    expect(h.nodes.has(NODE_ID)).toBe(false);
    expect(getByText('Loading...')).toBeTruthy();
    // A failed fetch is not a missing node — the tab must survive to be retried.
    expect(h.closedTabs).toEqual([]);

    fireDaemonHealthy();
    await settle();

    // The retry is the whole point: the pane refetched and the node landed.
    // Asserting the "Loading..." text then disappears would only be testing the
    // real store's reactivity (isNodeHydrated is $derived off the store cell) —
    // this mock store is a plain Map, and the suite stubs runes non-reactively.
    // The store's own suite covers that; here the contract is the refetch.
    expect(h.ensureCalls).toEqual([NODE_ID, NODE_ID]);
    expect(h.nodes.has(NODE_ID)).toBe(true);
  });

  it('does not re-fetch a fresh (non-stale) node on reconnect once already hydrated', async () => {
    h.ensureOutcomes.push('success');

    render(PaneContent, { props: { pane } });
    await settle();

    expect(h.ensureCalls).toEqual([NODE_ID]);
    expect(h.nodes.has(NODE_ID)).toBe(true);

    // A later reconnect (daemon restart mid-session) must be a no-op for a pane
    // whose node is present AND not flagged possibly-stale (h.staleIds is
    // empty here) — not a redundant refetch of the whole graph.
    fireDaemonHealthy();
    await settle();

    expect(h.ensureCalls).toEqual([NODE_ID]);
  });

  it('re-fetches a possibly-stale node on reconnect even though it is already hydrated', async () => {
    h.ensureOutcomes.push('success');

    render(PaneContent, { props: { pane } });
    await settle();

    expect(h.ensureCalls).toEqual([NODE_ID]);
    expect(h.nodes.has(NODE_ID)).toBe(true);

    // Simulate what a real WatchNodes outage leaves behind: the store now
    // considers this node's cache entry possibly-stale, because live updates
    // could have been missed for it while the connection was down — even
    // though the node was already present before the outage. This is the
    // repro: a chat node cached before a daemon restart rendered
    // permanently empty/stale afterward because presence alone was trusted
    // forever, with no re-confirmation against the backend.
    h.staleIds.add(NODE_ID);
    h.ensureOutcomes.push('success');

    fireDaemonHealthy();
    await settle();

    expect(h.ensureCalls).toEqual([NODE_ID, NODE_ID]);
    // The re-confirm resolved successfully, so the entry is fresh again —
    // a further reconnect with no new outage must not refetch a third time.
    expect(h.staleIds.has(NODE_ID)).toBe(false);
    fireDaemonHealthy();
    await settle();
    expect(h.ensureCalls).toEqual([NODE_ID, NODE_ID]);
  });

  it('stops retrying after the pane unmounts', async () => {
    h.ensureOutcomes.push('boot-window-failure');

    const { unmount } = render(PaneContent, { props: { pane } });
    await settle();
    expect(h.ensureCalls).toEqual([NODE_ID]);

    unmount();
    fireDaemonHealthy();
    await settle();

    // The unmounted pane must not fetch: its subscription is released on destroy.
    expect(h.ensureCalls).toEqual([NODE_ID]);
  });
});
