/**
 * E2E: daemon readiness — not-ready → degraded → recovered
 *
 * ADR-044 decision 3 promises daemon-dependent stores "re-run their load
 * automatically when the daemon transitions from unreachable to healthy."
 * That behavior lives entirely in `daemon-status.ts` and the stores that
 * subscribe to it via `onDaemonReconnect` — a real cross-process seam, not
 * something a synchronous mock can honestly stand in for (see ADR-048's
 * rationale on the optimistic-echo regression class, which is the same
 * class of bug: the defect lives in cross-process timing a mock collapses
 * away).
 *
 * Each state below is established BY CONSTRUCTION, not by racing a real
 * process's startup timing:
 *   - "not ready" uses a socket path nothing was ever spawned against —
 *     deterministic, since there is nothing to race.
 *   - "recovered" spawns the real daemon via `DaemonTestHarness.startDeferred()`
 *     and waits on `waitUntilProxyReady()`, which resolves once the daemon's
 *     UDS is reachable. `startDeferred()` starts the dev-proxy before the
 *     daemon binds, so the proxy's gRPC-js client is constructed against a
 *     not-yet-existing socket. That gRPC-js channel used to own its own
 *     ever-growing reconnect backoff, independent of the daemon's socket
 *     becoming reachable moments later — so a socket probe could read the
 *     daemon up while the channel was still mid-backoff ("reconnecting in
 *     2s"), and a store reload triggered right then failed silently.
 *     `packages/dev-tools/src/grpc-client.ts` closes that at the source: it
 *     caps the reconnect backoff to a fixed ~100ms and gates every RPC on the
 *     channel reaching READY, so once the socket is reachable the proxy
 *     self-heals within ~100ms. Waiting on the socket probe is therefore
 *     sufficient now, and the store reload this test drives afterwards
 *     recovers on its own instead of needing a real-RPC poll in the harness.
 *     (Checked separately: production's tonic lazy channel never had this gap
 *     — a call issued right after the daemon binds succeeds immediately, no
 *     cached backoff state. This is specific to the gRPC-js dev-proxy path.)
 *
 * `daemon-status.ts` is Tauri-event-driven in production, and this harness
 * has no Tauri runtime — so both tests bind a harness-backed
 * `DaemonStatusSource` (real UDS probes, not fabricated payloads) instead of
 * faking Tauri IPC. The only simulated piece is the transport binding,
 * which is exactly the piece whose realism doesn't matter here: it is a
 * callback wiring, not a timing seam.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { DaemonTestHarness } from './daemon-harness';
import type { DaemonStatusSource } from '$lib/services/daemon-status';
import type { SchemaNode } from '$lib/types/schema-node';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

// schemasData imports the real backend-adapter module at load time; redirect
// its getAllSchemas() to whatever adapter the active test wires up (a
// harness pointed at a real daemon, or one pointed at nothing) instead of
// the MockAdapter the singleton would otherwise resolve to under
// NODE_ENV=test. Keep the real HttpAdapter export intact (via importActual)
// since daemon-harness.ts imports it from this same module to drive the
// harness's live daemon connection — only the `backendAdapter` singleton
// needs redirecting here.
let getAllSchemasImpl: () => Promise<SchemaNode[]>;
vi.mock('$lib/services/backend-adapter', async (importOriginal) => {
  const actual = await importOriginal<typeof import('$lib/services/backend-adapter')>();
  return {
    ...actual,
    backendAdapter: {
      getAllSchemas: () => getAllSchemasImpl()
    }
  };
});

/** A DaemonStatusSource backed by real UDS probes against a harness's daemon. */
function harnessStatusSource(h: DaemonTestHarness): DaemonStatusSource {
  return {
    async getCurrent() {
      return (await h.isDaemonReachable()) ? 'healthy' : 'not_running';
    },
    // No push transport in this harness (no Tauri event bus) — recovery is
    // observed via refreshDaemonStatus() re-pulling getCurrent().
    subscribe() {
      return () => {};
    }
  };
}

describe('daemon readiness: not-ready -> degraded -> recovered (real daemon)', () => {
  afterEach(() => {
    vi.resetModules();
  });

  it('degraded: a store load genuinely fails against a daemon that was never started', async () => {
    // Deterministic by construction: no process is ever spawned for this
    // socket path, so there is nothing to race.
    getAllSchemasImpl = () => Promise.reject(new Error('connect ECONNREFUSED (no daemon)'));
    const unreachableSource: DaemonStatusSource = {
      async getCurrent() {
        return 'not_running';
      },
      subscribe() {
        return () => {};
      }
    };

    const { startDaemonStatusListener, daemonStatus } = await import('$lib/services/daemon-status');
    const { get } = await import('svelte/store');
    startDaemonStatusListener(unreachableSource);
    // Allow the initial pull in startDaemonStatusListener to settle.
    await Promise.resolve();
    await Promise.resolve();

    const { schemasData, schemasStore } = await import('$lib/stores/schemas.svelte');
    await schemasData.loadSchemas();

    expect(get(daemonStatus).unreachable).toBe(true);
    expect(schemasStore.builtInSchemas.length + schemasStore.customSchemas.length).toBe(0);
  });

  describe('recovered', () => {
    let h: DaemonTestHarness;

    beforeEach(async () => {
      h = await DaemonTestHarness.startDeferred();
      getAllSchemasImpl = () => h.adapter.getAllSchemas();
    }, 15_000);

    afterEach(async () => {
      await h?.stop();
    });

    it(
      'schemasData self-heals with real data once the real daemon becomes reachable',
      async () => {
        const { startDaemonStatusListener, daemonStatus, refreshDaemonStatus } = await import(
          '$lib/services/daemon-status'
        );
        const { get } = await import('svelte/store');
        // schemasData registers onDaemonReconnect(loadSchemas) at module
        // load (in schemas.ts, not here). It must be imported — and its
        // reconnect listener registered — BEFORE starting the daemon-status
        // listener below: startDaemonStatusListener's initial pull of
        // getCurrent() is not awaited by its caller (matching production,
        // where the caller doesn't block startup on it either), so if the
        // real daemon is already reachable by the time that pull resolves,
        // the "just became healthy" transition needs a subscriber in place
        // to observe it — same ordering requirement production has at app
        // startup.
        // Imported for its module-load side effect (registering
        // onDaemonReconnect(loadSchemas) — a reference to schemas.ts's
        // private loadSchemas, not schemasData.loadSchemas, so it cannot be
        // spied on from here to await its exact settlement).
        const { schemasStore } = await import('$lib/stores/schemas.svelte');

        const source = harnessStatusSource(h);
        startDaemonStatusListener(source);

        // Recovered: wait for the daemon's socket to be reachable. The
        // dev-proxy's gRPC channel now self-heals within ~100ms of that (see
        // the module doc comment and grpc-client.ts), so the store reload
        // this drives afterwards no longer needs a real-RPC poll to succeed.
        // Only then re-pull status through the shared contract, which is what
        // fires schemasData's own onDaemonReconnect hook.
        await h.waitUntilProxyReady(30_000);
        await refreshDaemonStatus();

        expect(get(daemonStatus).unreachable).toBe(false);

        // Poll briefly for the auto-retried load to land — the reconnect
        // callback fires loadSchemas() asynchronously and this test has no
        // direct handle on that exact call to await. Short window: the
        // socket is reachable and the proxy's gated RPC path self-heals, so
        // this is only waiting on scheduling plus the ~100ms channel
        // recovery, not on an unbounded backoff.
        const deadline = Date.now() + 5_000;
        let total = 0;
        while (Date.now() < deadline) {
          total = schemasStore.builtInSchemas.length + schemasStore.customSchemas.length;
          if (total > 0) break;
          await new Promise((r) => setTimeout(r, 50));
        }

        expect(total).toBeGreaterThan(0);

        // Confirm it's real data, not a fixture: every schema the store
        // landed is one the harness's own daemon actually reports (core
        // schema seeding can still be completing in the background, so the
        // two counts are not required to match exactly — only that the
        // store's data is a subset of what the real daemon has, not
        // fabricated).
        const schemas = await h.adapter.getAllSchemas();
        const realIds = new Set(schemas.map((s) => s.id));
        for (const s of [...schemasStore.builtInSchemas, ...schemasStore.customSchemas]) {
          expect(realIds.has(s.id)).toBe(true);
        }
      },
      45_000
    );
  });
});
