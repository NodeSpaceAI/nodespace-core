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

// AutocompleteModal types
export type { AutocompleteModalProps, NewNodeRequest } from './AutocompleteModal.svelte';

// Re-export types from services
export type {
  TextNodeData,
  TextSaveResult,
  HierarchicalTextNode
} from '$lib/services/mockTextService';

export type { MarkdownOptions } from '$lib/services/markdownUtils';
