/**
 * A test extension that exercises every contribution kind of the extension
 * API without importing any edition-specific code. Each contribution is gated by a flag in
 * `flags.svelte.ts`, so a test turns surfaces on and off by flipping state.
 *
 * Like an out-of-tree extension, it takes its types only from
 * `@nodespace/extension-api`, so svelte-check fails on an incompatible change to
 * them.
 *
 * Register it with `uiExtensionRegistry.register(createTestExtension())` and
 * `unregister(TEST_EXTENSION_ID)` in `afterEach`, then call
 * `resetTestExtension()`.
 *
 * Its `collaboration.entry` contributions are registered even while every flag
 * is off, and registration alone keeps core's default out of that slot (the
 * Labs contact card). A test that needs the default with the fixture registered
 * passes `{ replaceableSlots: undefined }` as an override.
 */
import type { NodespaceExtension } from '@nodespace/extension-api';
import { testExtensionFlags } from './flags.svelte';

export { testExtensionFlags, testExtensionMounts, resetTestExtension } from './flags.svelte';

export const TEST_EXTENSION_ID = 'test-extension';

/** How the fixture's settings section is identified and placed; each field defaults as shown. */
export interface TestSectionOptions {
  /** Default `test-section`. Also its navigation id. */
  id?: string;
  /** Default `Test section`. */
  label?: string;
  /** Default: absent, so the section is placed before About. */
  after?: string;
  /** Default: absent. */
  priority?: number;
}

export interface TestExtensionOptions {
  section?: TestSectionOptions;
}

export function createTestExtension(
  overrides: Partial<NodespaceExtension> = {},
  options: TestExtensionOptions = {}
): NodespaceExtension {
  const section = options.section ?? {};
  return {
    id: TEST_EXTENSION_ID,
    apiVersion: 2,
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
    settingsSections: [
      {
        id: section.id ?? 'test-section',
        label: section.label ?? 'Test section',
        ...(section.after !== undefined && { after: section.after }),
        ...(section.priority !== undefined && { priority: section.priority }),
        when: () => testExtensionFlags.section,
        load: () => import('./test-settings-section.svelte')
      }
    ],
    settingsSlots: [
      {
        id: 'database-action',
        slot: 'database.actions',
        when: () => testExtensionFlags.databaseActions,
        load: () => import('./test-database-action.svelte')
      },
      {
        id: 'database-row',
        slot: 'database.row',
        when: () => testExtensionFlags.databaseRow,
        load: () => import('./test-database-row.svelte')
      },
      {
        id: 'database-row-throwing',
        slot: 'database.row',
        when: () => testExtensionFlags.databaseRowThrowing,
        load: () => import('./test-throwing-row.svelte')
      }
    ],
    replaceableSlots: [
      {
        id: 'collaboration-entry',
        slot: 'collaboration.entry',
        when: () => testExtensionFlags.collaborationEntry,
        load: () => import('./test-collaboration-entry.svelte')
      },
      {
        id: 'collaboration-entry-secondary',
        slot: 'collaboration.entry',
        priority: 10,
        when: () => testExtensionFlags.collaborationEntrySecondary,
        load: () => import('./test-collaboration-entry-secondary.svelte')
      },
      {
        // `when()` throws while `collaborationEntryThrowingWhen` is set, and is otherwise false.
        id: 'collaboration-entry-throwing-when',
        slot: 'collaboration.entry',
        when: () => {
          if (testExtensionFlags.collaborationEntryThrowingWhen) {
            throw new Error('collaboration-entry-throwing-when: when() failed');
          }
          return false;
        },
        load: () => import('./test-collaboration-entry.svelte')
      },
      {
        id: 'collaboration-entry-failing-load',
        slot: 'collaboration.entry',
        when: () => testExtensionFlags.collaborationEntryFailingLoad,
        load: () => Promise.reject(new Error('collaboration-entry-failing-load: load() failed'))
      },
      {
        id: 'collaboration-entry-throwing',
        slot: 'collaboration.entry',
        when: () => testExtensionFlags.collaborationEntryThrowing,
        load: () => import('./test-throwing.svelte')
      }
    ],
    // Contributes `testExtensionFlags.collectionTreeRoots`, empty until a test sets it.
    // Pass `{ collectionTreeRoots }` as an override for a contributor of your own.
    collectionTreeRoots: () => testExtensionFlags.collectionTreeRoots,
    ...overrides
  };
}
