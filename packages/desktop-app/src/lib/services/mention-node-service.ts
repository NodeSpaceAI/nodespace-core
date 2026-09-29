/**
 * Creates the standalone node an `@mention` autocomplete "create new" choice
 * points at.
 *
 * `create_node` is an echo-suppressed write: the daemon does not send this
 * window its own `node:created`, and this caller does not go through
 * `sharedNodeStore`, so the store would never learn of the node. Loading it
 * into the store explicitly keeps every viewer of the new id (the reference
 * chip, the tab opened for it) consistent.
 */

import { v4 as uuidv4 } from 'uuid';
import { backendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

/** Create a top-level text node titled `title` and cache it; returns its id. */
export async function createMentionTargetNode(title: string): Promise<string> {
  const newNodeId = uuidv4();
  // Sibling ordering is handled by the backend (sibling_order column).
  await backendAdapter.createNode({
    id: newNodeId,
    content: title,
    nodeType: 'text',
    properties: {}
  });
  await sharedNodeStore.ensureNode(newNodeId);
  return newNodeId;
}
