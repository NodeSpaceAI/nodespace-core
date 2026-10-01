/**
 * Lifecycle and debug-dump parts of the test extension. Kept apart from
 * `index.ts` so `createTestExtension()` carries no hooks by default: a test that
 * wants them spreads `createTestLifecycleParts(...)` into the overrides,
 *
 *   uiExtensionRegistry.register(createTestExtension(createTestLifecycleParts().parts));
 */
import type { ExtensionLifecycle, NodespaceExtension } from '@nodespace/extension-api';

/** What `debugDump` returns unless an option overrides it. */
export const TEST_EXTENSION_DUMP = { fixture: 'test-extension' } as const;

export interface TestLifecycleOptions {
  /**
   * `start()` returns a promise that stays pending until `releaseStart()` is
   * called, then resolves with the cleanup. Default: `start()` returns the
   * cleanup directly.
   */
  deferStart?: boolean;
  /** Replaces `onDatabaseActivated`, e.g. with a `vi.fn()` or a throwing hook. */
  onDatabaseActivated?: ExtensionLifecycle['onDatabaseActivated'];
  /** Replaces `debugDump`. Default: returns {@link TEST_EXTENSION_DUMP}. */
  debugDump?: () => unknown;
}

export interface TestLifecycle {
  /** The parts to spread into `createTestExtension`'s overrides. */
  parts: Pick<NodespaceExtension, 'lifecycle' | 'debugDump'>;
  /** Ordered log: `start`, `cleanup` and `activated:<databaseId>`. */
  events: string[];
  /** Resolves a deferred `start()`; does nothing if none is pending. */
  releaseStart: () => void;
}

/** A recording lifecycle for the test extension; see {@link TestLifecycleOptions}. */
export function createTestLifecycleParts(options: TestLifecycleOptions = {}): TestLifecycle {
  const events: string[] = [];
  let release: () => void = () => {};

  const cleanup = (): void => {
    events.push('cleanup');
  };

  const lifecycle: ExtensionLifecycle = {
    start() {
      events.push('start');
      if (!options.deferStart) return cleanup;
      return new Promise<() => void>((resolve) => {
        release = () => resolve(cleanup);
      });
    },
    onDatabaseActivated(databaseId) {
      events.push(`activated:${databaseId}`);
      options.onDatabaseActivated?.(databaseId);
    }
  };

  return {
    parts: { lifecycle, debugDump: options.debugDump ?? (() => TEST_EXTENSION_DUMP) },
    events,
    releaseStart: () => release()
  };
}
