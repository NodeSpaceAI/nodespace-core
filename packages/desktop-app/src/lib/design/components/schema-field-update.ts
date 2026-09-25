/**
 * Schema-aware property updates for viewer-rendered nodes.
 *
 * A typed core field (`task.status`, `project.start_date`, …) is written as a
 * top-level typed change, which the store routes through the type's typed
 * update; every other field is written flat into `properties`. See
 * `schema-field-resolution.ts`.
 */

import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { buildFieldWrite } from '$lib/components/schema/schema-field-resolution';

/**
 * Extract and transform node properties into component-compatible metadata.
 * Delegates to the plugin registry for type-specific transformations.
 */
export function extractNodeMetadata(node: {
  nodeType: string;
  properties?: Record<string, unknown>;
}): Record<string, unknown> {
  return pluginRegistry.extractNodeMetadata(node);
}

/**
 * Update a schema field value for a node.
 *
 * @param viewerId - Origin viewer id, recorded on the store update for echo suppression
 * @param targetNodeId - Node to update
 * @param fieldName - Schema field name (e.g. 'status', 'due_date')
 * @param value - New value for the field
 */
export function updateSchemaField(
  viewerId: string,
  targetNodeId: string,
  fieldName: string,
  value: unknown
): void {
  const targetNode = sharedNodeStore.getNode(targetNodeId);
  if (!targetNode) return;

  sharedNodeStore.updateNode(targetNodeId, buildFieldWrite(targetNode, fieldName, value), {
    type: 'viewer',
    viewerId
  });
}
