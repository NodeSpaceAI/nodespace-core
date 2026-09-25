/**
 * Schema-aware property updates for viewer-rendered nodes.
 *
 * Routes task node fields through the type-safe task update path and everything
 * else through the generic flat properties path (`properties[fieldName]`).
 */

import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';

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
 * For task nodes, task-specific fields (status, priority, dueDate) route
 * through the type-safe task update path. All other fields — and all non-task
 * nodes — use the generic properties path.
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

  // Route task node property updates through type-safe path
  if (targetNode.nodeType === 'task') {
    // Map field names to TaskNodeUpdate structure
    // The task-specific fields are: status, priority, dueDate
    const taskFields = ['status', 'priority', 'due_date', 'dueDate'];

    if (taskFields.includes(fieldName)) {
      // Use type-safe task node update
      sharedNodeStore.updateTaskNode(
        targetNodeId,
        { [fieldName === 'due_date' ? 'dueDate' : fieldName]: value },
        { type: 'viewer', viewerId }
      );
      return;
    }
  }

  // Generic path: write the field flat — the shape the frontend receives. The
  // backend moves bare keys into the type's storage bucket.
  sharedNodeStore.updateNode(
    targetNodeId,
    { properties: { ...targetNode.properties, [fieldName]: value } },
    { type: 'viewer', viewerId }
  );
}
