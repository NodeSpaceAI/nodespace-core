/**
 * Relationship viewer + editor service.
 *
 * Thin dual-mode transport for the typed-relationship commands, routed through
 * the `backendAdapter` (Tauri IPC in the desktop app, HTTP dev-proxy in browser
 * dev mode) exactly like the schema/query paths — so the relationship feature is
 * validatable in `dev:browser`, not just the real Tauri app. Kept separate from
 * `relationship-grouping.ts` so the pure grouping/orientation logic stays free of
 * the adapter import and remains unit-testable.
 */

import { createLogger } from '$lib/utils/logger';
import { backendAdapter } from '$lib/services/backend-adapter';
import type { Node } from '$lib/types';
import {
  buildRelationshipsView,
  farEndHolders,
  resolveEdgeEndpoints,
  type NodeRelationshipsView,
  type RawNodeRelationships,
  type RelationshipGroupView,
  type RelationshipRowView,
  type ReplacedEdge
} from './relationship-grouping';

const log = createLogger('RelationshipViewer');

/** Fetch the raw aggregate for a node (grouped relationships + edge data). */
export async function fetchNodeRelationships(nodeId: string): Promise<RawNodeRelationships> {
  log.debug('Fetching relationships', { nodeId });
  return await backendAdapter.getNodeRelationships(nodeId);
}

/** Fetch and normalize a node's relationships into the modal's view model. */
export async function loadNodeRelationshipsView(nodeId: string): Promise<NodeRelationshipsView> {
  const raw = await fetchNodeRelationships(nodeId);
  return buildRelationshipsView(raw);
}

/**
 * Add a typed relationship edge from the modal's node to `targetId` within
 * `group`. Orientation follows the group's direction (see `resolveEdgeEndpoints`):
 * for an outbound group the modal's node is the source; for an inbound group it
 * is the target. `edgeData` carries declared edge-field values (omit for a bare
 * edge).
 *
 * The daemon validates the target type, but a `one` end is enforced by
 * REPLACING the prior edge, not by rejecting the write — so this can silently
 * reassign. Call `findDisplacedHolders` first to confirm with the user; the
 * returned edges are what the daemon actually evicted.
 */
export async function addEdge(
  nodeId: string,
  group: RelationshipGroupView,
  targetId: string,
  edgeData?: Record<string, unknown>
): Promise<ReplacedEdge[]> {
  const { sourceId, targetId: resolvedTarget } = resolveEdgeEndpoints(
    nodeId,
    group.direction,
    targetId
  );
  log.debug('Adding edge', { sourceId, relationshipName: group.relationshipName, targetId: resolvedTarget });
  const { replaced } = await backendAdapter.createRelationship(
    sourceId,
    group.relationshipName,
    resolvedTarget,
    edgeData
  );
  if (replaced.length > 0) log.info('Edge replaced a cardinality-one link', { replaced });
  return replaced;
}

/**
 * The nodes that linking `nodeId` to `targetId` through `group` would unlink
 * from `targetId` (see `farEndHolders`). Only fetches the candidate's
 * relationships when the far end is `one` — otherwise nothing can be displaced.
 */
export async function findDisplacedHolders(
  nodeId: string,
  group: RelationshipGroupView,
  targetId: string
): Promise<RelationshipRowView[]> {
  if (group.farCardinality !== 'one') return [];
  const candidate = await loadNodeRelationshipsView(targetId);
  return farEndHolders(candidate, group, nodeId);
}

/**
 * Remove the typed relationship edge between the modal's node and the related
 * row `rowId` within `group`. The daemon rejects removing the last edge of a
 * `required` relationship — that surfaces here as a thrown error the caller
 * should present.
 */
export async function removeEdge(
  nodeId: string,
  group: RelationshipGroupView,
  rowId: string
): Promise<void> {
  const { sourceId, targetId } = resolveEdgeEndpoints(nodeId, group.direction, rowId);
  log.debug('Removing edge', { sourceId, relationshipName: group.relationshipName, targetId });
  await backendAdapter.deleteRelationship(sourceId, group.relationshipName, targetId);
}

/**
 * Replace the edge attributes on the edge between the modal's node and the
 * related row `rowId` within `group`. Overwrites the stored edge properties
 * wholesale (matching the daemon's replace semantics).
 */
export async function updateEdgeProperties(
  nodeId: string,
  group: RelationshipGroupView,
  rowId: string,
  properties: Record<string, unknown>
): Promise<void> {
  const { sourceId, targetId } = resolveEdgeEndpoints(nodeId, group.direction, rowId);
  log.debug('Updating edge properties', { sourceId, relationshipName: group.relationshipName, targetId });
  await backendAdapter.updateRelationshipProperties(sourceId, group.relationshipName, targetId, properties);
}

/**
 * Type-ahead search for edge targets of the declared `targetType` by title.
 * `targetType` null means the relationship has no declared target type — search
 * across all node types.
 */
export async function searchTargets(targetType: string | null, query: string): Promise<Node[]> {
  return await backendAdapter.searchNodesByTitle(targetType, query, 10);
}
