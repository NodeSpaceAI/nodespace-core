/**
 * navigation-sidebar.svelte — tree-item actions (ADR-082 §3.2).
 *
 * Every item of the sidebar collection tree hosts the actions extensions
 * contribute, with its own `{ nodeId, nodeType }`, inside its row, where the row
 * reveals them on hover or focus. With the fixture extension, clicking an item's
 * action opens the fixture's `app-shell-modal` contribution for that item, the
 * flow an extension's hover action uses to open a dialog of its own.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';

import NavigationSidebar from '$lib/components/layout/navigation-sidebar.svelte';
import ChromeSlotOutlet from '$lib/plugins/chrome-slot-outlet.svelte';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import { collectionService, type CollectionInfo } from '$lib/services/collection-service';
import { collectionsData, collectionsState } from '$lib/stores/collections.svelte';
import { databaseStore } from '$lib/stores/database.svelte';
import { layoutStore } from '$lib/stores/layout.svelte';
import {
  TEST_EXTENSION_ID,
  TEST_NODE_TYPE,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags
} from '../fixtures/test-extension';

function collection(
  id: string,
  name: string,
  parentCollectionIds: string[] = [],
  nodeType = 'collection'
): CollectionInfo {
  return {
    lifecycleStatus: 'active',
    id,
    content: name,
    nodeType,
    createdAt: '',
    modifiedAt: '',
    version: 1,
    properties: {},
    memberCount: 1,
    parentCollectionIds
  };
}

// Three levels, with a subtype in the middle.
const COLLECTIONS = [
  collection('engineering', 'Engineering'),
  collection('platform', 'Platform', ['engineering'], TEST_NODE_TYPE),
  collection('runtime', 'Runtime', ['platform']),
  collection('design', 'Design')
];

/** The tree row whose name button reads `name`. */
function row(container: HTMLElement, name: string): HTMLElement {
  const button = [...container.querySelectorAll<HTMLElement>('.collection-name-btn')].find(
    (el) => el.textContent?.trim() === name
  );
  const found = button?.closest<HTMLElement>('.collection-item');
  if (!found) throw new Error(`No collection row named "${name}"`);
  return found;
}

/** Each fixture action in the sidebar, as `nodeId:nodeType`, in document order. */
function actions(container: HTMLElement): string[] {
  return [...container.querySelectorAll('[data-testid="test-tree-action"]')].map(
    (el) => `${el.getAttribute('data-node-id')}:${el.getAttribute('data-node-type')}`
  );
}

async function renderSidebar() {
  const view = render(NavigationSidebar);
  await waitFor(() => expect(row(view.container, 'Design')).toBeTruthy());
  return view;
}

describe('NavigationSidebar — tree-item actions', () => {
  beforeEach(() => {
    vi.spyOn(databaseStore, 'load').mockResolvedValue();
    vi.spyOn(collectionService, 'getAllCollections').mockResolvedValue(COLLECTIONS);
    layoutStore.state = { ...layoutStore.state, sidebarCollapsed: false, collectionsExpanded: true };
    collectionsState.toggleCollectionExpanded('engineering');
    collectionsState.toggleCollectionExpanded('platform');
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
    collectionsData.reset();
    collectionsState.reset();
    layoutStore.state = { ...layoutStore.state, collectionsExpanded: false };
    localStorage.clear();
  });

  it('hosts no actions with no extension registered', async () => {
    const { container } = await renderSidebar();

    expect(row(container, 'Runtime')).toBeTruthy();
    expect(container.querySelector('.tree-item-actions')).toBeNull();
  });

  it('gives every item, at each level, the action with its own id and type, in its own row', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.treeAction = true;
    const { container } = await renderSidebar();

    await waitFor(() =>
      expect(actions(container)).toEqual([
        'design:collection',
        'engineering:collection',
        `platform:${TEST_NODE_TYPE}`,
        'runtime:collection'
      ])
    );
    for (const [name, id] of [
      ['Engineering', 'engineering'],
      ['Platform', 'platform'],
      ['Runtime', 'runtime'],
      ['Design', 'design']
    ]) {
      const action = row(container, name).querySelector('[data-testid="test-tree-action"]');
      expect(action?.getAttribute('data-node-id')).toBe(id);
      expect(action?.closest('.tree-item-actions')).not.toBeNull();
    }
  });

  it('leaves the action off an item its when(item) is false for', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.treeAction = true;
    testExtensionFlags.treeActionHiddenFor = ['platform'];
    const { container } = await renderSidebar();

    await waitFor(() =>
      expect(actions(container)).toEqual([
        'design:collection',
        'engineering:collection',
        'runtime:collection'
      ])
    );
    expect(row(container, 'Platform').querySelector('.tree-item-actions')).toBeNull();
  });

  it('gives an optimistic item no action until the database confirms it', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.treeAction = true;
    let confirm: (id: string) => void = () => {};
    vi.spyOn(collectionService, 'createCollection').mockImplementation(
      () => new Promise<string>((resolve) => (confirm = resolve))
    );
    const { container } = await renderSidebar();
    await waitFor(() => expect(actions(container)).toHaveLength(4));

    const created = collectionsData.createCollection('Research');
    await waitFor(() => expect(row(container, 'Research').classList.contains('pending')).toBe(true));
    // Give a (wrongly) mounted action time to load.
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(row(container, 'Research').querySelector('.tree-item-actions')).toBeNull();

    confirm('research');
    await created;
    await waitFor(() =>
      expect(
        row(container, 'Research')
          .querySelector('[data-testid="test-tree-action"]')
          ?.getAttribute('data-node-id')
      ).toBe('research')
    );
  });

  it("opens the extension's modal for the item whose action was clicked", async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.treeAction = true;
    const sidebar = await renderSidebar();
    const modalSlot = render(ChromeSlotOutlet, { props: { name: 'app-shell-modal' } });
    expect(modalSlot.queryByTestId('test-item-modal')).toBeNull();

    const action = await waitFor(() => {
      const found = row(sidebar.container, 'Platform').querySelector<HTMLElement>(
        '[data-testid="test-tree-action"]'
      );
      expect(found).not.toBeNull();
      return found as HTMLElement;
    });
    await fireEvent.mouseEnter(row(sidebar.container, 'Platform'));
    await fireEvent.click(action);

    const modal = await modalSlot.findByTestId('test-item-modal');
    expect(modal.getAttribute('data-node-id')).toBe('platform');
    expect(modal.getAttribute('data-node-type')).toBe(TEST_NODE_TYPE);

    await fireEvent.click(modalSlot.getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(modalSlot.queryByTestId('test-item-modal')).toBeNull());
  });
});
