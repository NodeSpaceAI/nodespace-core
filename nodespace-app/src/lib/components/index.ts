/**
 * Component exports for NodeSpace
 *
 * Centralizes component imports for better organization.
 */

export { default as TextNode } from './TextNode.svelte';
export { default as TextNodeDemo } from './TextNodeDemo.svelte';
export { default as NodeTree } from './NodeTree.svelte';
export { default as HierarchyDemo } from './HierarchyDemo.svelte';
export { default as AutocompleteModal } from './AutocompleteModal.svelte';
export { default as AutocompleteModalDemo } from './AutocompleteModalDemo.svelte';
export type { TreeNodeData } from '$lib/types/tree';

// Export AutocompleteModal types - defined inline since they're UI-specific
export interface AutocompleteModalProps {
  visible: boolean;
  position: { x: number; y: number };
  query: string;
  nodeReferenceService: import('$lib/services/NodeReferenceService').NodeReferenceService;
}

export interface NewNodeRequest {
  type: 'create';
  content: string;
  nodeType: string;
}

// Re-export types from services
export type {
  TextNodeData,
  TextSaveResult,
  HierarchicalTextNode
} from '$lib/services/mockTextService';

export type { MarkdownOptions } from '$lib/services/markdownUtils';
