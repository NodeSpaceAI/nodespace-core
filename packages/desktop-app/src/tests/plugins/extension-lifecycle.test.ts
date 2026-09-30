/**
 * Extension lifecycle dispatch: `start` and its cleanups, `onDatabaseActivated`,
 * and `debugDump` collection. The functions read the process-wide registry, so
 * every test registers its own extensions and `afterEach` empties it.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const log = vi.hoisted(() => ({
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn()
}));

vi.mock('$lib/utils/logger', () => ({ createLogger: () => log }));

import {
  collectExtensionDebugDumps,
  notifyDatabaseActivated,
  startExtensions
} from '$lib/plugins/extension-lifecycle';
import {
  uiExtensionRegistry,
  type ExtensionLifecycle,
  type NodespaceExtension
} from '$lib/plugins/ui-extensions';
import { createTestExtension } from '../fixtures/test-extension';
import { createTestLifecycleParts } from '../fixtures/test-extension/lifecycle';

const srcRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

function ext(id: string, rest: Partial<NodespaceExtension> = {}): NodespaceExtension {
  return { id, apiVersion: 1, ...rest };
}

function register(...extensions: NodespaceExtension[]): void {
  for (const e of extensions) uiExtensionRegistry.register(e);
}

/** Let promise reactions (and the rejections they log) run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

afterEach(() => {
  for (const e of uiExtensionRegistry.all()) uiExtensionRegistry.unregister(e.id);
  vi.clearAllMocks();
});

describe('startExtensions', () => {
  it('starts extensions in registration order and cleans up in reverse', () => {
    const events: string[] = [];
    const recording = (name: string): NodespaceExtension =>
      ext(name, {
        lifecycle: {
          start: () => {
            events.push(`start:${name}`);
            return () => events.push(`cleanup:${name}`);
          }
        }
      });
    register(recording('a'), recording('b'), recording('c'));

    const stop = startExtensions();
    expect(events).toEqual(['start:a', 'start:b', 'start:c']);

    stop();
    expect(events).toEqual([
      'start:a',
      'start:b',
      'start:c',
      'cleanup:c',
      'cleanup:b',
      'cleanup:a'
    ]);
  });

  it('cleans up in reverse registration order even when async starts settle out of order', async () => {
    const events: string[] = [];
    const resolvers: Record<string, () => void> = {};
    const deferred = (name: string): NodespaceExtension =>
      ext(name, {
        lifecycle: {
          start: () =>
            new Promise<() => void>((resolve) => {
              resolvers[name] = () => resolve(() => events.push(`cleanup:${name}`));
            })
        }
      });
    register(deferred('a'), deferred('b'), deferred('c'));

    const stop = startExtensions();
    // Settle in the opposite order to registration.
    resolvers.c();
    resolvers.a();
    resolvers.b();
    await flush();
    stop();

    expect(events).toEqual(['cleanup:c', 'cleanup:b', 'cleanup:a']);
  });

  it('runs the cleanup of a start() that resolves after the disposer ran immediately', async () => {
    const late = createTestLifecycleParts({ deferStart: true });
    register(createTestExtension(late.parts));

    const stop = startExtensions();
    stop();
    expect(late.events).toEqual(['start']);

    late.releaseStart();
    await flush();
    expect(late.events).toEqual(['start', 'cleanup']);
  });

  it('runs a late cleanup once, and not again on a second dispose', async () => {
    const late = createTestLifecycleParts({ deferStart: true });
    register(createTestExtension(late.parts));

    const stop = startExtensions();
    stop();
    late.releaseStart();
    await flush();
    stop();

    expect(late.events).toEqual(['start', 'cleanup']);
  });

  it('runs a sync cleanup once when the disposer is called twice', () => {
    const cleanup = vi.fn();
    register(ext('a', { lifecycle: { start: () => cleanup } }));

    const stop = startExtensions();
    stop();
    stop();

    expect(cleanup).toHaveBeenCalledOnce();
  });

  it('still starts the others when one start throws', () => {
    const cleanupAfter = vi.fn();
    const startBefore = vi.fn();
    register(
      ext('before', { lifecycle: { start: startBefore } }),
      ext('throws', {
        lifecycle: {
          start: () => {
            throw new Error('start failed');
          }
        }
      }),
      ext('after', { lifecycle: { start: () => cleanupAfter } })
    );

    const stop = startExtensions();
    stop();

    expect(startBefore).toHaveBeenCalledOnce();
    expect(cleanupAfter).toHaveBeenCalledOnce();
    expect(log.error).toHaveBeenCalledWith(
      'Extension start threw',
      expect.objectContaining({ extensionId: 'throws' })
    );
  });

  it('still starts the others when one start rejects', async () => {
    const cleanupAfter = vi.fn();
    register(
      ext('rejects', { lifecycle: { start: () => Promise.reject(new Error('start rejected')) } }),
      ext('after', { lifecycle: { start: () => cleanupAfter } })
    );

    const stop = startExtensions();
    await flush();
    stop();

    expect(cleanupAfter).toHaveBeenCalledOnce();
    expect(log.error).toHaveBeenCalledWith(
      'Extension start rejected',
      expect.objectContaining({ extensionId: 'rejects' })
    );
  });

  it('still runs the other cleanups when one throws', () => {
    const first = vi.fn();
    const last = vi.fn();
    register(
      ext('first', { lifecycle: { start: () => first } }),
      ext('throws', {
        lifecycle: {
          start: () => () => {
            throw new Error('cleanup failed');
          }
        }
      }),
      ext('last', { lifecycle: { start: () => last } })
    );

    const stop = startExtensions();
    expect(() => stop()).not.toThrow();

    expect(last).toHaveBeenCalledOnce();
    expect(first).toHaveBeenCalledOnce();
    expect(log.error).toHaveBeenCalledWith(
      'Extension cleanup threw',
      expect.objectContaining({ extensionId: 'throws' })
    );
  });

  it('ignores a start() whose result is not a cleanup function', () => {
    const lifecycle = { start: () => 'not a function' } as unknown as ExtensionLifecycle;
    register(ext('odd', { lifecycle }));

    const stop = startExtensions();

    expect(() => stop()).not.toThrow();
    expect(log.error).not.toHaveBeenCalled();
  });

  it('skips extensions with no lifecycle or no start', () => {
    register(ext('none'), ext('empty', { lifecycle: {} }));

    expect(() => startExtensions()()).not.toThrow();
    expect(log.error).not.toHaveBeenCalled();
  });

  it('is a no-op when nothing is registered', () => {
    expect(() => startExtensions()()).not.toThrow();
  });
});

describe('notifyDatabaseActivated', () => {
  it('calls each hook with the database id, in registration order', () => {
    const calls: string[] = [];
    const hook = (name: string): NodespaceExtension =>
      ext(name, { lifecycle: { onDatabaseActivated: (id) => calls.push(`${name}:${id}`) } });
    register(hook('a'), hook('b'), hook('c'));

    notifyDatabaseActivated('db-1');

    expect(calls).toEqual(['a:db-1', 'b:db-1', 'c:db-1']);
  });

  it('still calls the others when a hook throws, and does not throw itself', () => {
    const after = vi.fn();
    register(
      ext('throws', {
        lifecycle: {
          onDatabaseActivated: () => {
            throw new Error('hook failed');
          }
        }
      }),
      ext('after', { lifecycle: { onDatabaseActivated: after } })
    );

    expect(() => notifyDatabaseActivated('db-1')).not.toThrow();

    expect(after).toHaveBeenCalledWith('db-1');
    expect(log.error).toHaveBeenCalledWith(
      'Extension onDatabaseActivated threw',
      expect.objectContaining({ extensionId: 'throws' })
    );
  });

  it('logs a hook that returns a rejected promise instead of leaving it unhandled', async () => {
    const after = vi.fn();
    const rejecting = (): void => {
      // An async hook breaks the contract, but the rejection must still be handled.
      return Promise.reject(new Error('async hook failed')) as unknown as void;
    };
    register(
      ext('rejects', { lifecycle: { onDatabaseActivated: rejecting } }),
      ext('after', { lifecycle: { onDatabaseActivated: after } })
    );

    notifyDatabaseActivated('db-1');
    await flush();

    expect(after).toHaveBeenCalledWith('db-1');
    expect(log.error).toHaveBeenCalledWith(
      'Extension onDatabaseActivated rejected',
      expect.objectContaining({ extensionId: 'rejects' })
    );
  });

  it('calls the hook as a method of the lifecycle object', () => {
    // The hook reads its own state through `this`; a detached call would throw.
    class RecordingLifecycle implements ExtensionLifecycle {
      received: string[] = [];
      onDatabaseActivated(databaseId: string): void {
        this.received.push(databaseId);
      }
    }
    const lifecycle = new RecordingLifecycle();
    register(ext('a', { lifecycle }));

    notifyDatabaseActivated('db-1');

    expect(lifecycle.received).toEqual(['db-1']);
  });

  it('is a no-op when nothing is registered, or nothing has the hook', () => {
    expect(() => notifyDatabaseActivated('db-1')).not.toThrow();
    register(ext('none'), ext('start-only', { lifecycle: { start: () => undefined } }));
    expect(() => notifyDatabaseActivated('db-1')).not.toThrow();
    expect(log.error).not.toHaveBeenCalled();
  });
});

describe('collectExtensionDebugDumps', () => {
  it('keys each dump by extension id and skips extensions without one', () => {
    register(
      ext('a', { debugDump: () => ({ count: 1 }) }),
      ext('b', { debugDump: () => 'plain' }),
      ext('no-dump')
    );

    expect(collectExtensionDebugDumps()).toEqual({ a: { count: 1 }, b: 'plain' });
  });

  it('records a throwing dump as an error string without losing the others', () => {
    register(
      ext('throws', {
        debugDump: () => {
          throw new Error('dump failed');
        }
      }),
      ext('fine', { debugDump: () => 42 })
    );

    expect(collectExtensionDebugDumps()).toEqual({ throws: 'error: dump failed', fine: 42 });
  });

  it('keeps an extension id of __proto__ as a key rather than a prototype', () => {
    register(ext('__proto__', { debugDump: () => ({ marker: true }) }));

    const dumps = collectExtensionDebugDumps();

    expect(Object.getPrototypeOf(dumps)).toBe(Object.prototype);
    expect(Object.keys(dumps)).toEqual(['__proto__']);
  });

  it('is empty when nothing is registered', () => {
    expect(collectExtensionDebugDumps()).toEqual({});
  });
});

describe('wiring and import graph', () => {
  const read = (relative: string): string => fs.readFileSync(path.join(srcRoot, relative), 'utf8');

  it('mounts extensions from the app shell and disposes them on unmount', () => {
    const shell = read('lib/components/layout/app-shell.svelte');

    expect(shell).toMatch(/stopExtensions\s*=\s*startExtensions\(\)/);
    expect(shell).toContain('stopExtensions?.()');
  });

  it('notifies extensions from the database store', () => {
    const store = read('lib/stores/database.svelte.ts');

    expect(store.match(/notifyDatabaseActivated\(/g)).toHaveLength(2);
  });

  it('imports only the registry and the logger, never the reactive wrapper', () => {
    const source = read('lib/plugins/extension-lifecycle.ts');
    // `from '…'` (import and re-export), a bare `import '…'`, and `import('…')`.
    const specifierPattern = /(?:\bfrom\s+|^import\s+|\bimport\(\s*)'([^']+)'/gm;
    const specifiers = [...source.matchAll(specifierPattern)].map((m) => m[1]);

    expect(specifiers.sort()).toEqual(['$lib/utils/logger', './ui-extensions']);
  });
});
