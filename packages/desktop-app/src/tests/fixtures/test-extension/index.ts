/**
 * A test extension that exercises every contribution kind of the extension
 * API without importing anything Pro. Each contribution is gated by a flag in
 * `flags.svelte.ts`, so a test turns surfaces on and off by flipping state.
 *
 * Register it with `uiExtensionRegistry.register(createTestExtension())` and
 * `unregister(TEST_EXTENSION_ID)` in `afterEach`, then call
 * `resetTestExtension()`.
 */
import type { NodespaceExtension } from '$lib/plugins/ui-extensions';
import { testExtensionFlags } from './flags.svelte';

export { testExtensionFlags, testExtensionMounts, resetTestExtension } from './flags.svelte';

export const TEST_EXTENSION_ID = 'test-extension';

export function createTestExtension(overrides: Partial<NodespaceExtension> = {}): NodespaceExtension {
  return {
    id: TEST_EXTENSION_ID,
    apiVersion: 1,
    chrome: [
      {
        id: 'overlay',
        slot: 'app-shell-overlay',
        when: () => testExtensionFlags.overlay,
        load: () => import('./test-chrome.svelte')
      },
      {
        id: 'modal',
        slot: 'app-shell-modal',
        when: () => testExtensionFlags.modal,
        load: () => import('./test-chrome.svelte')
      },
      {
        id: 'modal-secondary',
        slot: 'app-shell-modal',
        priority: 10,
        when: () => testExtensionFlags.modalSecondary,
        load: () => import('./test-chrome-secondary.svelte')
      },
      {
        // `when()` throws while `throwingWhen` is set, and is otherwise false.
        id: 'throwing-when',
        slot: 'app-shell-modal',
        when: () => {
          if (testExtensionFlags.throwingWhen) throw new Error('throwing-when: when() failed');
          return false;
        },
        load: () => import('./test-chrome.svelte')
      },
      {
        id: 'throwing-component',
        slot: 'app-shell-modal',
        when: () => testExtensionFlags.throwingComponent,
        load: () => import('./test-throwing.svelte')
      }
    ],
    viewerTabs: [
      {
        id: 'tab',
        nodeType: 'collection',
        label: 'Test tab',
        when: () => testExtensionFlags.tab,
        load: () => import('./test-viewer-tab.svelte')
      },
      {
        id: 'tab-secondary',
        nodeType: 'collection',
        label: 'Second test tab',
        priority: -1,
        when: () => testExtensionFlags.tabSecondary,
        load: () => import('./test-viewer-tab.svelte')
      }
    ],
    ...overrides
  };
}
