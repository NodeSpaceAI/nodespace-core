/**
 * Component exports for NodeSpace
 *
 * Centralizes component imports for better organization.
 */

export { default as TextNode } from './text-node.svelte';
export { default as NodeTree } from './node-tree.svelte';
export { default as AutocompleteModal } from './autocomplete-modal.svelte';
export { default as MarkdownRenderer } from './markdown-renderer.svelte';
export { default as BaseNodeReference } from './base-node-reference.svelte';
export type { TreeNodeData } from '$lib/types/tree';

// Export AutocompleteModal types - defined inline since they're UI-specific
export interface AutocompleteModalProps {
  visible: boolean;
  position: { x: number; y: number };
  query: string;
  nodeReferenceService: import('$lib/services/nodeReferenceService').NodeReferenceService;
}

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

export type { MarkdownOptions } from '$lib/services/markdownUtils';
