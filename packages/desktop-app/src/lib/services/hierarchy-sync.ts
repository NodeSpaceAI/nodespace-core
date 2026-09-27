// services/hierarchy-sync.ts

import type { ReactiveStructureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { ChildPlacement } from './adapter-core';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('HierarchySync');

export interface HasChildPayload {
  parentId: string;
  childId: string;
  order?: unknown; // typed unknown, runtime-checked
}

/**
 * Apply a has_child relationship:created event to the structureTree.
 * Order-fallback contract (single authoritative implementation):
 *  - If incoming order is a real number (typeof === 'number'), use it directly (0/negative count).
 *  - Else if child already exists under this parent, preserve existing order.
 *  - Else append at tail: lastSibling.order + 1 (integer increment, no invented values).
 * Date.now() and Math.random() are NEVER used as fallbacks.
 */
export function applyHasChildCreated(
  structureTree: ReactiveStructureTree,
  payload: HasChildPayload
): void {
  const { parentId, childId } = payload;
  const incomingOrder = typeof payload.order === 'number' ? payload.order : undefined;

  const siblings = structureTree.getChildrenWithOrder(parentId);
  let order: number;

  if (incomingOrder !== undefined) {
    order = incomingOrder;
  } else {
    const existing = siblings.find((c) => c.nodeId === childId);
    if (existing) {
      order = existing.order;
    } else {
      log.warn('relationship:created missing order for has_child — appending', { parentId, childId });
      const lastOrder = siblings[siblings.length - 1]?.order ?? 0;
      order = lastOrder + 1;
    }
  }

  structureTree.addChild({ parentId, childId, order });
}

/**
 * Apply a has_child relationship:updated event to the structureTree.
 * Logs a warning if order is missing (should always be present for updated events).
 */
export function applyHasChildUpdated(
  structureTree: ReactiveStructureTree,
  payload: HasChildPayload
): void {
  const { parentId, childId } = payload;
  const incomingOrder = typeof payload.order === 'number' ? payload.order : undefined;
  if (incomingOrder === undefined) {
    log.warn('relationship:updated missing order for has_child', { parentId, childId });
    return;
  }
  structureTree.updateChildOrder(parentId, childId, incomingOrder);
}

/**
 * Apply a has_child relationship:deleted event to the structureTree.
 */
export function applyHasChildDeleted(
  structureTree: ReactiveStructureTree,
  payload: { parentId: string; childId: string }
): void {
  structureTree.removeChild({ parentId: payload.parentId, childId: payload.childId, order: 0 });
}

/**
 * Apply the placement a create or move returned to its own caller.
 *
 * The client that made a hierarchy write never receives that write's own
 * relationship events (same-origin echo suppression), so its optimistic,
 * locally computed order keys would otherwise never be replaced by the store's.
 * This writes the store's keys for the re-spread siblings and the written edge.
 *
 * Only corrects nodes still under `placement.parentId`: a node the local tree has
 * since moved elsewhere is left alone — the later write's own reply places it.
 *
 * Known limit: replies and the event stream are separate channels, and replies to
 * concurrent writes can resolve out of commit order. A reply applied after a newer
 * re-spread (its own or another client's) writes older-space keys until the next
 * write or refetch corrects them. The window is narrow — it needs two re-spreads of
 * one parent racing — and placements carry no sequence to order them by.
 */
export function applyChildPlacement(
  structureTree: ReactiveStructureTree,
  childId: string,
  placement: ChildPlacement
): void {
  applySiblingOrders(structureTree, placement.parentId, [
    ...placement.respread,
    { nodeId: childId, order: placement.order }
  ]);
}

/**
 * Write the store's order keys for children of `parentId`, as a hierarchy write's
 * reply returned them, in one reactive batch. Children no longer under `parentId`
 * are left alone (see `applyChildPlacement`).
 */
export function applySiblingOrders(
  structureTree: ReactiveStructureTree,
  parentId: string,
  orders: ReadonlyArray<{ nodeId: string; order: number }>
): void {
  structureTree.runBatch(() => {
    for (const { nodeId, order } of orders) {
      structureTree.updateChildOrder(parentId, nodeId, order);
    }
  });
}
