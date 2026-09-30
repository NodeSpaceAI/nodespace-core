import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import {
  debugChannelWrite,
  isChannelEnabledSync,
  isChannelEnabled,
  captureDomSnapshot,
  captureStoreDump,
  collectStoreDump
} from '$lib/services/debug-channel';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import { TEST_EXTENSION_ID, createTestExtension } from '../fixtures/test-extension';
import {
  TEST_EXTENSION_DUMP,
  createTestLifecycleParts
} from '../fixtures/test-extension/lifecycle';

describe('debug-channel', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  describe('debugChannelWrite', () => {
    it('never touches the Tauri invoke bridge under VITEST (isTest guard)', () => {
      debugChannelWrite({
        kind: 'console',
        timestamp: new Date().toISOString(),
        level: 'info',
        message: 'test message'
      });

      expect(mockInvoke).not.toHaveBeenCalled();
    });

    it('does not throw for any DebugEvent kind', () => {
      const timestamp = new Date().toISOString();
      expect(() => {
        debugChannelWrite({ kind: 'console', timestamp, level: 'debug', message: 'm' });
        debugChannelWrite({ kind: 'console', timestamp, level: 'error', message: 'm', data: { a: 1 } });
        debugChannelWrite({
          kind: 'invoke',
          timestamp,
          method: 'createNode',
          args: ['a'],
          durationMs: 12,
          status: 'success',
          result: { id: '1' }
        });
        debugChannelWrite({
          kind: 'invoke',
          timestamp,
          method: 'createNode',
          args: ['a'],
          durationMs: 12,
          status: 'error',
          error: 'boom'
        });
        debugChannelWrite({ kind: 'dom_snapshot', timestamp, html: '<html></html>' });
        debugChannelWrite({ kind: 'store_dump', timestamp, stores: { foo: 'bar' } });
      }).not.toThrow();
    });
  });

  describe('isChannelEnabledSync', () => {
    it('reports false before the async probe resolves (default state)', () => {
      expect(isChannelEnabledSync()).toBe(false);
    });
  });

  describe('isChannelEnabled', () => {
    it('resolves based on the frontend_log_enabled probe', async () => {
      mockInvoke.mockResolvedValueOnce(false);
      const enabled = await isChannelEnabled();
      expect(enabled).toBe(false);
      expect(mockInvoke).toHaveBeenCalledWith('frontend_log_enabled');
    });
  });

  describe('captureDomSnapshot', () => {
    it('does not throw and does not invoke the Tauri bridge under VITEST', () => {
      expect(() => captureDomSnapshot()).not.toThrow();
      expect(mockInvoke).not.toHaveBeenCalled();
    });
  });

  describe('captureStoreDump', () => {
    it('resolves without throwing and does not invoke the Tauri bridge under VITEST', async () => {
      await expect(captureStoreDump()).resolves.toBeUndefined();
      expect(mockInvoke).not.toHaveBeenCalled();
    });
  });

  describe('collectStoreDump extensions', () => {
    afterEach(() => {
      uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
      uiExtensionRegistry.unregister('other-extension');
    });

    function registerWithDump(debugDump: () => unknown, id = TEST_EXTENSION_ID): void {
      uiExtensionRegistry.register({
        ...createTestExtension(createTestLifecycleParts().parts),
        id,
        debugDump
      });
    }

    it('holds each extension dump under extensions[<id>]', async () => {
      uiExtensionRegistry.register(createTestExtension(createTestLifecycleParts().parts));

      const stores = await collectStoreDump();

      expect(stores.extensions).toEqual({ [TEST_EXTENSION_ID]: TEST_EXTENSION_DUMP });
    });

    it('is an empty object when no extension has a dump', async () => {
      uiExtensionRegistry.register(createTestExtension());

      const stores = await collectStoreDump();

      expect(stores.extensions).toEqual({});
    });

    it('records a throwing dump as an error string and keeps the rest of the dump', async () => {
      registerWithDump(() => {
        throw new Error('dump failed');
      });
      registerWithDump(() => ({ ok: true }), 'other-extension');

      const stores = await collectStoreDump();

      expect(stores.extensions).toEqual({
        [TEST_EXTENSION_ID]: 'error: dump failed',
        'other-extension': { ok: true }
      });
      expect(stores.databaseStore).toBeDefined();
    });

    it('records a circular dump as an error string that names the cycle, and the dump stays serializable', async () => {
      registerWithDump(() => {
        const cycle: Record<string, unknown> = {};
        cycle.self = cycle;
        return cycle;
      });
      registerWithDump(() => ({ ok: true }), 'other-extension');

      const stores = await collectStoreDump();

      const entries = stores.extensions as Record<string, unknown>;
      expect(entries[TEST_EXTENSION_ID]).toMatch(/^error: .*circular/i);
      expect(entries['other-extension']).toEqual({ ok: true });
      expect(() => JSON.stringify(stores)).not.toThrow();
    });

    it('converts Map and Set inside a dump, like the other stores', async () => {
      registerWithDump(() => ({ byKey: new Map([['k', 1]]), members: new Set(['a', 'b']) }));

      const stores = await collectStoreDump();

      expect(stores.extensions).toEqual({
        [TEST_EXTENSION_ID]: { byKey: { k: 1 }, members: ['a', 'b'] }
      });
    });

    it('records a dump with no JSON form as null', async () => {
      registerWithDump(() => undefined);

      const stores = await collectStoreDump();

      expect(stores.extensions).toEqual({ [TEST_EXTENSION_ID]: null });
    });
  });
});
