/**
 * Collection authoring helpers
 *
 * Shared logic behind the "New node" and "Add existing" actions that both the
 * full collection viewer (`collection-node-viewer.svelte`) and the sidebar
 * sub-panel (`collection-sub-panel.svelte`) offer. A collection is a
 * `member_of` grouping over outliner nodes, so both actions ultimately call
 * `collectionService.addNodeToCollection`. Extracting them here keeps the two
 * components in lock-step (no duplicated authoring rules) and makes the
 * behaviour unit-testable without a DOM.
 */

import { invoke } from '@tauri-apps/api/core';
import { v4 as uuidv4 } from 'uuid';
import { backendAdapter } from '$lib/services/backend-adapter';
import { collectionService } from '$lib/services/collection-service';
import { NON_CONTENT_NODE_TYPES } from '$lib/stores/collections.svelte';
import type { Node } from '$lib/types';

/**
 * Mint a fresh, empty `text` node and attach it to the given collection.
 *
 * The node is created as a root (`parentId: null`) with empty content so the
 * caller can immediately open it for editing. Returns the new node's id.
 */
export async function createNodeInCollection(collectionId: string): Promise<string> {
  const newId = uuidv4();
  await backendAdapter.createNode({
    id: newId,
    nodeType: 'text',
    content: '',
    properties: {},
    mentions: [],
    parentId: null,
  });
  await collectionService.addNodeToCollection(newId, collectionId);
  return newId;
}

/**
 * Search the user's root nodes for candidates to add to a collection.
 *
 * Trims the query (an empty/whitespace query yields `[]` without hitting the
 * backend), runs the `search_roots` command, then drops results that cannot be
 * meaningfully added: the collection itself, ids already present (or otherwise
 * excluded), and non-content node types (person/schema/database-settings/
 * collection/horizontal-line).
 *
 * `search_roots` merges title/keyword and semantic matches server-side into one
 * ranked list, so a non-embeddable node (a `task`, say — ADR-029) is offered
 * here when its title matches. The filtering below only ever drops rows; the
 * backend's ranking of the rows it keeps is preserved.
 */
export async function searchAddableNodes(
  query: string,
  collectionId: string,
  excludeIds: Set<string>
): Promise<Node[]> {
  const q = query.trim();
  if (!q) return [];

  const found = await invoke<Node[]>('search_roots', { params: { query: q, limit: 20 } });
  return found.filter(
    (n) =>
      n.id !== collectionId && !excludeIds.has(n.id) && !NON_CONTENT_NODE_TYPES.has(n.nodeType)
  );
}
