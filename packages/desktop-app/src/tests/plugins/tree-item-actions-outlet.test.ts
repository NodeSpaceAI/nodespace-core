/**
 * TreeItemActionsOutlet: the host of one collection-tree item's actions
 * (ADR-082 §3.2). Each action visible for the item renders in its own outlet,
 * with the item's `{ nodeId, nodeType }`, in priority order; a failing action
 * leaves only its own place empty (ADR-082 §3.4).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

const log = vi.hoisted(() => ({
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn()
}));

vi.mock('$lib/utils/logger', () => ({ createLogger: () => log }));

import TreeItemActionsOutlet from '$lib/plugins/tree-item-actions-outlet.svelte';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import {
  TEST_EXTENSION_ID,
  TEST_NODE_TYPE,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags,
  testExtensionMounts
} from '../fixtures/test-extension';

/** The fixture markers in the container, in document order. */
function markers(container: HTMLElement): string[] {
  return [...container.querySelectorAll('[data-testid]')].map(
    (el) => el.getAttribute('data-testid') ?? ''
  );
}

/** Lets a load that was (wrongly) started land before asserting it did not. */
async function settle(): Promise<void> {
  await import('../fixtures/test-extension/test-tree-action.svelte');
  await new Promise((resolve) => setTimeout(resolve, 20));
}

const item = { nodeId: 'engineering', nodeType: 'collection' };

describe('TreeItemActionsOutlet', () => {
  beforeEach(() => {
    log.warn.mockClear();
    log.error.mockClear();
  });

  afterEach(() => {
    cleanup();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
  });

  it('renders nothing, not even its container, with no extension registered', async () => {
    const { container } = render(TreeItemActionsOutlet, { props: item });

    await settle();
    expect(container.querySelector('.tree-item-actions')).toBeNull();
    expect(container.textContent).toBe('');
  });

  it('renders nothing while every action is hidden', async () => {
    uiExtensionRegistry.register(createTestExtension());
    const { container } = render(TreeItemActionsOutlet, { props: item });

    await settle();
    expect(container.querySelector('.tree-item-actions')).toBeNull();
    expect(testExtensionMounts).toEqual({});
  });

  it('mounts a visible action in its container with the item’s id and type', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.treeAction = true;
    const { container, findByTestId } = render(TreeItemActionsOutlet, {
      props: { nodeId: 'engineering', nodeType: TEST_NODE_TYPE }
    });

    const button = await findByTestId('test-tree-action');
    expect(button.closest('.tree-item-actions')).toBe(container.querySelector('.tree-item-actions'));
    expect(button.getAttribute('data-node-id')).toBe('engineering');
    expect(button.getAttribute('data-node-type')).toBe(TEST_NODE_TYPE);
  });

  it('shows an action only on the items its when(item) holds for', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.treeAction = true;
    testExtensionFlags.treeActionHiddenFor = ['design'];

    const shown = render(TreeItemActionsOutlet, { props: item });
    const hidden = render(TreeItemActionsOutlet, {
      props: { nodeId: 'design', nodeType: 'collection' }
    });

    await shown.findByTestId('test-tree-action');
    await settle();
    expect(markers(hidden.container)).toEqual([]);
    expect(hidden.container.querySelector('.tree-item-actions')).toBeNull();
  });

  it('follows the state its when() reads', async () => {
    uiExtensionRegistry.register(createTestExtension());
    const { container, findByTestId } = render(TreeItemActionsOutlet, { props: item });
    await settle();
    expect(markers(container)).toEqual([]);

    testExtensionFlags.treeAction = true;
    await findByTestId('test-tree-action');

    testExtensionFlags.treeActionHiddenFor = ['engineering'];
    await waitFor(() => expect(container.querySelector('.tree-item-actions')).toBeNull());
  });

  it('renders the visible actions in priority order', async () => {
    uiExtensionRegistry.register(createTestExtension());
    testExtensionFlags.treeAction = true;
    testExtensionFlags.treeActionSecondary = true;

    const { container } = render(TreeItemActionsOutlet, { props: item });

    // The secondary action has the higher priority, though it is declared second.
    await waitFor(() =>
      expect(markers(container)).toEqual(['test-tree-action-secondary', 'test-tree-action'])
    );
  });

  describe('contribution failure (ADR-082 §3.4)', () => {
    it('leaves out an action whose when(item) throws, keeps its siblings, and warns once', async () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.treeAction = true;
      testExtensionFlags.treeActionThrowingFor = ['engineering'];

      const { container, findByTestId } = render(TreeItemActionsOutlet, { props: item });

      await findByTestId('test-tree-action');
      expect(markers(container)).toEqual(['test-tree-action']);
      expect(log.warn).toHaveBeenCalledTimes(1);
      expect(log.warn).toHaveBeenCalledWith(
        expect.stringContaining('when() threw'),
        expect.objectContaining({ key: `${TEST_EXTENSION_ID}/tree-action-throwing-when` })
      );
    });

    it('renders nothing for an action whose load() rejects, logs it, and keeps its siblings', async () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.treeAction = true;
      testExtensionFlags.treeActionFailingLoad = true;

      const { container, findByTestId } = render(TreeItemActionsOutlet, { props: item });

      await findByTestId('test-tree-action');
      await waitFor(() =>
        expect(log.error).toHaveBeenCalledWith(
          expect.stringContaining('failed to load'),
          expect.anything()
        )
      );
      expect(markers(container)).toEqual(['test-tree-action']);
    });

    it('removes only the outlet of an action whose component throws, and logs it', async () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.treeAction = true;
      testExtensionFlags.treeActionThrowing = true;

      const { container, findByTestId } = render(TreeItemActionsOutlet, { props: item });

      await findByTestId('test-tree-action');
      await waitFor(() =>
        expect(log.error).toHaveBeenCalledWith(
          expect.stringContaining('component threw'),
          expect.anything()
        )
      );
      expect(markers(container)).toEqual(['test-tree-action']);
    });
  });
});
