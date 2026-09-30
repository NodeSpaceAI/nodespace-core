/**
 * Extension Lifecycle
 * ===================
 *
 * Dispatch for the hooks an extension can carry (ADR-082 §2.5): `lifecycle.start`,
 * `lifecycle.onDatabaseActivated` and the top-level `debugDump`. The registry in
 * `ui-extensions.ts` only stores extensions; this module is what the host calls
 * to run their hooks. It is plain TypeScript (no runes) and imports only the
 * registry and the logger. In particular it never imports
 * `ui-extensions.svelte.ts`, so a store that notifies extensions does not pull
 * the reactive wrapper into its own import graph.
 *
 * Every hook runs in registration order and is isolated: a hook that throws (or,
 * for the async ones, rejects) is logged and never stops the host or the other
 * extensions.
 *
 * Firing points
 * -------------
 * These are part of the versioned extension API. Moving one is a breaking change.
 *
 * `lifecycle.start()`
 *   - Called once per webview load, from the app shell's mount, and only when the
 *     Tauri bridge is present. Never in browser dev mode.
 *   - May return a cleanup, synchronously or through a promise. The cleanups run
 *     when the shell unmounts, in reverse registration order, whichever order the
 *     `start()` calls settled in. A `start()` that settles after that has already
 *     happened has its cleanup run at once.
 *
 * `lifecycle.onDatabaseActivated(databaseId)`
 *   - Synchronous, called once per committed activation of a database:
 *       1. The first resolution of the active database in the database store's
 *          `load()`, in the Tauri app only. For a restored non-default database it
 *          runs after the previous caches were evicted; for the default database
 *          nothing was evicted, so there is nothing to invalidate.
 *       2. Every `switchTo()` that commits, after the previous database's caches
 *          were evicted and before the workspace (open tabs) is reset.
 *   - So when it runs, the new database is the routed one and `sharedNodeStore`
 *     holds no data from the previous one. An extension clears its own
 *     per-database caches here.
 *   - It does not fire in browser dev mode, for a switch a newer switch
 *     superseded, for a `switchTo()` to the database already active, or for a
 *     later `load()` that keeps the current selection.
 *   - A returned promise is ignored apart from logging its rejection: the
 *     extension serializes its own async work.
 *
 * `debugDump()`
 *   - Called on demand when the debug channel captures a store dump. The result
 *     is recorded under `stores.extensions[<extension id>]`.
 */

import { createLogger } from '$lib/utils/logger';
import { uiExtensionRegistry, type NodespaceExtension } from './ui-extensions';

const log = createLogger('ExtensionLifecycle');

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return (
    (typeof value === 'object' || typeof value === 'function') &&
    value !== null &&
    typeof (value as { then?: unknown }).then === 'function'
  );
}

/** Run one extension's cleanup; a throw is logged, never propagated. */
function runCleanup(extensionId: string, cleanup: () => void): void {
  try {
    cleanup();
  } catch (error) {
    log.error('Extension cleanup threw', { extensionId, error });
  }
}

/**
 * Start every registered extension's `lifecycle.start()`, in registration order,
 * and return a disposer that runs the cleanups they returned, in reverse
 * registration order. The registry is read once, when this is called. Calling
 * the disposer again does nothing.
 */
export function startExtensions(): () => void {
  const extensions = uiExtensionRegistry.all();
  // One slot per extension, so cleanups run in registration order's reverse even
  // when async `start()` calls settle out of order.
  const cleanups: (() => void)[] = new Array<() => void>(extensions.length);
  let disposed = false;

  const accept = (index: number, extension: NodespaceExtension, value: unknown): void => {
    if (typeof value !== 'function') return;
    const cleanup = value as () => void;
    if (disposed) runCleanup(extension.id, cleanup);
    else cleanups[index] = cleanup;
  };

  extensions.forEach((extension, index) => {
    const lifecycle = extension.lifecycle;
    if (typeof lifecycle?.start !== 'function') return;
    try {
      const result: unknown = lifecycle.start();
      if (isThenable(result)) {
        result.then(
          (value) => accept(index, extension, value),
          (error: unknown) => log.error('Extension start rejected', { extensionId: extension.id, error })
        );
      } else {
        accept(index, extension, result);
      }
    } catch (error) {
      log.error('Extension start threw', { extensionId: extension.id, error });
    }
  });

  return () => {
    if (disposed) return;
    disposed = true;
    for (let index = extensions.length - 1; index >= 0; index--) {
      const cleanup = cleanups[index];
      if (cleanup !== undefined) runCleanup(extensions[index].id, cleanup);
    }
  };
}

/**
 * Call every registered extension's `lifecycle.onDatabaseActivated`, in
 * registration order. Never throws; see the module doc for when the host calls it.
 */
export function notifyDatabaseActivated(databaseId: string): void {
  for (const extension of uiExtensionRegistry.all()) {
    const lifecycle = extension.lifecycle;
    if (typeof lifecycle?.onDatabaseActivated !== 'function') continue;
    try {
      const result: unknown = lifecycle.onDatabaseActivated(databaseId);
      if (isThenable(result)) {
        result.then(undefined, (error: unknown) =>
          log.error('Extension onDatabaseActivated rejected', { extensionId: extension.id, error })
        );
      }
    } catch (error) {
      log.error('Extension onDatabaseActivated threw', { extensionId: extension.id, error });
    }
  }
}

/**
 * Every registered extension's `debugDump()` result, keyed by extension id.
 * Extensions without a `debugDump` are absent; one whose `debugDump` throws is
 * recorded as the string `error: <message>`.
 */
export function collectExtensionDebugDumps(): Record<string, unknown> {
  const entries: [string, unknown][] = [];
  for (const extension of uiExtensionRegistry.all()) {
    if (typeof extension.debugDump !== 'function') continue;
    try {
      entries.push([extension.id, extension.debugDump()]);
    } catch (error) {
      entries.push([extension.id, `error: ${errorMessage(error)}`]);
    }
  }
  // fromEntries defines own properties, so an extension id such as `__proto__`
  // cannot rewrite the result's prototype the way an assignment would.
  return Object.fromEntries(entries);
}
