/**
 * Helper for components that display a node WITHOUT it necessarily being a
 * `structureTree` descendant of any open tab's document root — a query/
 * Kanban/table view's matched rows (they live under unrelated parent
 * documents, not as children of the query node), an inline
 * `[[wikilink]]`/mention reference, a relation-field value, a backlink.
 *
 * `SharedNodeStore`'s eviction feature normally determines whether a cached
 * node is still reachable by walking `structureTree` up to an open tab's
 * root. That walk has no way to know about the consumers above, so without
 * this, a node they display could be silently evicted out from under them
 * after the inactivity threshold — indistinguishable, from the component's
 * point of view, from the node having been deleted.
 *
 * Call from `$effect` with the id(s) currently being displayed; the
 * returned cleanup un-pins them (before the effect re-runs on a changed id,
 * and on unmount), which `$effect` calls automatically:
 *
 *   const ownerId = uuidv4(); // once per component instance
 *   $effect(() => pinReachableNodes(ownerId, [id]));
 *
 * For a changing set (e.g. query results), call again with the full current
 * list on every change — `SharedNodeStore.pinNodes` replaces the owner's
 * pin set wholesale, not as a delta.
 */

import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

export function pinReachableNodes(ownerId: string, nodeIds: Iterable<string>): () => void {
  sharedNodeStore.pinNodes(ownerId, nodeIds);
  return () => sharedNodeStore.unpinAll(ownerId);
}
