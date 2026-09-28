/**
 * Regression test: pressing Enter on a brand-new, empty document/date page must not
 * synchronously mutate the shared node store during the keystroke's template render.
 *
 * Full-component-mount tests for BaseNodeViewer require complex setup (NodeServiceContext,
 * plugin registry, database mocking — see merge-prevention.test.ts and
 * pane-content-hydration.test.ts, which document this and use the same extracted-harness
 * approach). Instead this test drives a harness that mirrors the placeholder-promotion
 * branch of handleCreateNewNode() in base-node-viewer.svelte: on Enter, when the target node
 * is the viewer-local placeholder (the only content on a brand-new page), the handler
 * promotes it to a real node and must defer `sharedNodeStore.setNode()` +
 * `reactiveStructureTree.addChild()` to the next tick — calling them synchronously triggers
 * `notifySubscribers()`, which invokes wildcard subscription callbacks that mutate `$state`;
 * doing that during template render throws Svelte's `state_unsafe_mutation` and wedges the
 * reactive flush (see the three sibling handlers — handleContentChanged, handleNodeTypeChanged,
 * handleSlashCommandSelected — which use the identical `tick().then()` pattern for the same
 * reason).
 *
 * The continuation that actually creates the new sibling node reads the promoted node back
 * out of nodeManager/sharedNodeStore, so it must not run until after that deferred write —
 * this is the ordering hazard fixed alongside the crash itself.
 */

import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';

interface PromotedNode {
  id: string;
  content: string;
}

interface PlaceholderPromotionDeps {
  sharedNodeStore: {
    setNode: (node: PromotedNode, source: { type: 'viewer'; viewerId: string }, persist: boolean) => void;
  };
  reactiveStructureTree: {
    addChild: (edge: { parentId: string; childId: string; order: number }) => void;
  };
  promotePlaceholderToNode: (
    placeholder: PromotedNode,
    nodeId: string,
    opts: { content: string }
  ) => PromotedNode;
  proceedWithNodeCreation: (afterNodeId: string) => void;
  setIsPromoting: (value: boolean) => void;
  viewerId: string;
}

/**
 * Mirrors the placeholder-promotion branch of handleCreateNewNode() in
 * base-node-viewer.svelte: promote synchronously, then defer the store write + structure-tree
 * edge + promotion-flag clear + node-creation continuation to the next tick.
 */
function handleEnterOnEmptyPagePlaceholder(
  currentPlaceholder: PromotedNode,
  nodeId: string,
  currentContent: string | undefined,
  deps: PlaceholderPromotionDeps
): Promise<void> {
  deps.setIsPromoting(true);

  const promotedNode = deps.promotePlaceholderToNode(currentPlaceholder, nodeId, {
    content: currentContent ?? ''
  });
  const promotionParentId = nodeId;

  return tick().then(() => {
    deps.sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId: deps.viewerId }, false);
    deps.reactiveStructureTree.addChild({
      parentId: promotionParentId,
      childId: promotedNode.id,
      order: Date.now()
    });
    deps.setIsPromoting(false);
    deps.proceedWithNodeCreation(promotedNode.id);
  });
}

describe('BaseNodeViewer — Enter on a brand-new empty page', () => {
  it('does not touch the store synchronously, and creates the promoted node after a tick', async () => {
    const setNode = vi.fn();
    const addChild = vi.fn();
    const proceedWithNodeCreation = vi.fn();
    const setIsPromoting = vi.fn();
    const promotedNode: PromotedNode = { id: 'promoted-1', content: '' };
    const promotePlaceholderToNode = vi.fn().mockReturnValue(promotedNode);

    const promise = handleEnterOnEmptyPagePlaceholder(
      { id: 'placeholder-1', content: '' },
      'page-1',
      '',
      {
        sharedNodeStore: { setNode },
        reactiveStructureTree: { addChild },
        promotePlaceholderToNode,
        proceedWithNodeCreation,
        setIsPromoting,
        viewerId: 'test-viewer'
      }
    );

    // Synchronous portion (the part that runs during the keystroke's template render):
    // must not call setNode/addChild — those are what trigger notifySubscribers() and can
    // mutate $state — and must not yet attempt to create the new sibling node, since that
    // reads the promoted node back out of the store.
    expect(setNode).not.toHaveBeenCalled();
    expect(addChild).not.toHaveBeenCalled();
    expect(proceedWithNodeCreation).not.toHaveBeenCalled();
    expect(setIsPromoting).toHaveBeenCalledTimes(1);
    expect(setIsPromoting).toHaveBeenNthCalledWith(1, true);

    await promise;

    // After the tick: the promoted node is persisted, wired into the structure tree, the
    // promotion flag is cleared, and node creation proceeds now that afterNodeId resolves.
    expect(setNode).toHaveBeenCalledTimes(1);
    expect(setNode).toHaveBeenCalledWith(
      promotedNode,
      { type: 'viewer', viewerId: 'test-viewer' },
      false
    );
    expect(addChild).toHaveBeenCalledTimes(1);
    expect(addChild).toHaveBeenCalledWith({
      parentId: 'page-1',
      childId: 'promoted-1',
      order: expect.any(Number)
    });
    expect(setIsPromoting).toHaveBeenCalledTimes(2);
    expect(setIsPromoting).toHaveBeenNthCalledWith(2, false);
    expect(proceedWithNodeCreation).toHaveBeenCalledTimes(1);
    expect(proceedWithNodeCreation).toHaveBeenCalledWith('promoted-1');
  });

  it('promotes with the current (possibly empty) content, matching Enter on a truly blank page', async () => {
    const promotePlaceholderToNode = vi.fn().mockReturnValue({ id: 'promoted-2', content: '' });

    await handleEnterOnEmptyPagePlaceholder({ id: 'placeholder-1', content: '' }, 'page-1', undefined, {
      sharedNodeStore: { setNode: vi.fn() },
      reactiveStructureTree: { addChild: vi.fn() },
      promotePlaceholderToNode,
      proceedWithNodeCreation: vi.fn(),
      setIsPromoting: vi.fn(),
      viewerId: 'test-viewer'
    });

    // currentContent is undefined when Enter fires on a page with no typed content —
    // the handler must fall back to '' rather than passing undefined through.
    expect(promotePlaceholderToNode).toHaveBeenCalledTimes(1);
    expect(promotePlaceholderToNode).toHaveBeenCalledWith(
      { id: 'placeholder-1', content: '' },
      'page-1',
      { content: '' }
    );
  });
});
