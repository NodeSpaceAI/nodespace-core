/**
 * Structured NDJSON debug channel for the packaged Tauri (WKWebView) app.
 *
 * When `NS_FRONTEND_LOG` is set (a file path), every call below appends one
 * JSON object per line to that file via the `frontend_log` Tauri command —
 * console messages, backend invoke/network events, on-demand DOM snapshots,
 * and on-demand store dumps all share this one channel. Gated by a one-time
 * `frontend_log_enabled` probe, so normal builds, the browser, and tests pay
 * no cost. An agent (or human) queries the channel by tailing the file the
 * env var names — no daemon or HTTP endpoint involved.
 */

import { isVitest as isTest } from '$lib/utils/is-vitest';

export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

// `args`/`result` are untruncated at the type level — callers are
// responsible for bounding them before constructing the event (the two
// current callers in diagnostic-logger.ts already pass values through
// truncateArgs/truncateResult).
export type DebugEvent =
  | { kind: 'console'; timestamp: string; level: LogLevel; message: string; data?: unknown }
  | {
      kind: 'invoke';
      timestamp: string;
      method: string;
      args: unknown[];
      durationMs: number;
      status: 'success' | 'error';
      result?: unknown;
      error?: string;
    }
  | { kind: 'dom_snapshot'; timestamp: string; html: string }
  | { kind: 'store_dump'; timestamp: string; stores: Record<string, unknown> };

let channelState: 'unknown' | 'on' | 'off' = 'unknown';
let channelInit: Promise<void> | null = null;

/**
 * Synchronous best-effort read of the channel's enabled state. Returns
 * `false` until the async probe (triggered by the first `debugChannelWrite`
 * call) resolves. The only current caller, `logger.ts`'s `shouldLog`, treats
 * a stale `false` as "fall through to normal level-based gating" — so the
 * worst case is a handful of startup log lines evaluated before the channel
 * is known to be on, not any loss of data once the app is running.
 */
export function isChannelEnabledSync(): boolean {
  return channelState === 'on';
}

/** Resolves once the channel-enabled probe completes. Use to gate one-time setup (e.g. `window.__ns_debug`). */
export async function isChannelEnabled(): Promise<boolean> {
  if (channelState === 'unknown') {
    if (!channelInit) {
      channelInit = (async () => {
        try {
          const { invoke } = await import('@tauri-apps/api/core');
          channelState = (await invoke<boolean>('frontend_log_enabled')) ? 'on' : 'off';
        } catch {
          channelState = 'off';
        }
      })();
    }
    await channelInit;
  }
  return channelState === 'on';
}

/**
 * Serialize and append a debug event to the NDJSON channel. Best-effort —
 * never throws. Tests pay no cost: never touch the Tauri `invoke` bridge
 * under VITEST, or the one-time probe pollutes invoke-call assertions
 * elsewhere.
 */
export function debugChannelWrite(event: DebugEvent): void {
  if (isTest) return;
  void (async () => {
    try {
      if (!(await isChannelEnabled())) return;
      const { invoke } = await import('@tauri-apps/api/core');
      const line = JSON.stringify(event);
      await invoke('frontend_log', { line });
    } catch {
      /* best-effort diagnostic only */
    }
  })();
}

/**
 * Capture the current DOM as one debug-channel event. On-demand only.
 *
 * May include user content (node text, journal entries) verbatim in the
 * NDJSON file — acceptable because the channel is dev-only, opt-in via
 * `NS_FRONTEND_LOG`, and local-file-only (never transmitted). Do not enable
 * this channel in shipped builds.
 */
export function captureDomSnapshot(): void {
  debugChannelWrite({
    kind: 'dom_snapshot',
    timestamp: new Date().toISOString(),
    html: document.documentElement.outerHTML
  });
}

/**
 * Serialize a value for the store dump, converting Map/Set (common in
 * Svelte 5 reactive stores) to plain JSON-representable shapes.
 */
function toDumpable(value: unknown): unknown {
  if (value instanceof Map) return Object.fromEntries(value);
  if (value instanceof Set) return Array.from(value);
  return value;
}

/**
 * Serialize one extension's `debugDump()` result on its own, so a value that
 * cannot be serialized (a cycle, a BigInt) costs only that extension's entry and
 * never the rest of the dump. `undefined` has no JSON form and becomes `null`.
 */
function toExtensionDumpEntry(dump: unknown): unknown {
  try {
    const json = JSON.stringify(dump, (_k, v) => toDumpable(v));
    return json === undefined ? null : JSON.parse(json);
  } catch (error) {
    return `error: ${error instanceof Error ? error.message : String(error)}`;
  }
}

/**
 * Capture a snapshot of the highest-value global stores as one debug-channel
 * event. On-demand only — dynamic imports keep this from pulling every store
 * into the eagerly-loaded module graph.
 *
 * May include user content and PII (node content in the node store, and
 * anything an extension's `debugDump` reports under `extensions`) verbatim — same
 * dev-only/opt-in/local-file-only tradeoff as `captureDomSnapshot` above. Never
 * enable this channel in shipped builds.
 */
export async function captureStoreDump(): Promise<void> {
  const stores = await collectStoreDump();
  debugChannelWrite({ kind: 'store_dump', timestamp: new Date().toISOString(), stores });
}

/** Gather the store snapshot that `captureStoreDump` writes. */
export async function collectStoreDump(): Promise<Record<string, unknown>> {
  const stores: Record<string, unknown> = {};
  try {
    // sharedNodeStore is node-graph-shaped and much larger than the other
    // stores below — pluck just the node map instead of blanket-stringifying
    // the whole singleton (which also carries internal bookkeeping fields
    // like subscription maps that aren't useful in a dump).
    const { sharedNodeStore } = await import('./shared-node-store.svelte');
    stores.sharedNodeStore = {
      nodes: toDumpable(sharedNodeStore.nodes)
    };
  } catch {
    /* store unavailable in this context */
  }
  try {
    const { databaseStore } = await import('$lib/stores/database.svelte');
    stores.databaseStore = JSON.parse(JSON.stringify(databaseStore, (_k, v) => toDumpable(v)));
  } catch {
    /* store unavailable in this context */
  }
  try {
    const { navigationStore } = await import('$lib/stores/navigation.svelte');
    stores.navigationStore = JSON.parse(JSON.stringify(navigationStore, (_k, v) => toDumpable(v)));
  } catch {
    /* store unavailable in this context */
  }
  try {
    const { agentStore } = await import('$lib/stores/agent-store.svelte');
    stores.agentStore = JSON.parse(JSON.stringify(agentStore, (_k, v) => toDumpable(v)));
  } catch {
    /* store unavailable in this context */
  }
  try {
    const { collectExtensionDebugDumps } = await import('$lib/plugins/extension-lifecycle');
    // fromEntries defines own properties, so an extension id such as `__proto__`
    // stays a key instead of rewriting the object's prototype.
    stores.extensions = Object.fromEntries(
      Object.entries(collectExtensionDebugDumps()).map(([id, dump]) => [
        id,
        toExtensionDumpEntry(dump)
      ])
    );
  } catch {
    /* extension registry unavailable in this context */
  }

  return stores;
}
