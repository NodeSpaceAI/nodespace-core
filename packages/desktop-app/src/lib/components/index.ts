/**
 * Component exports for NodeSpace
 *
 * Centralizes component imports for better organization.
 */

export { default as TextNode } from '$lib/design/components/text-node.svelte';
export { default as MarkdownRenderer } from './markdown-renderer.svelte';
export { default as BaseNodeReference } from './base-node-reference.svelte';
export type { TreeNodeData } from '$lib/types/tree';

export interface NewNodeRequest {
  type: 'create';
  content: string;
  nodeType: string;
}

// Re-export types from services
// Temporarily commented out - mockTextService deleted in Phase 1
// export type {
//   TextNodeData,
//   TextSaveResult,
//   HierarchicalTextNode
// } from '$lib/services/mockTextService';

export type { MarkdownOptions } from '$lib/services/markdown-utils';
