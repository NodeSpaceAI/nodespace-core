/**
 * Component exports for NodeSpace
 *
 * Centralizes component imports for better organization.
 */

export { default as TextNode } from './TextNode.svelte';
export { default as TextNodeDemo } from './TextNodeDemo.svelte';
export { default as NodeTree } from './NodeTree.svelte';
export { default as HierarchyDemo } from './HierarchyDemo.svelte';
export type { TreeNodeData } from '$lib/types/tree';

// Re-export types from services
export type {
  TextNodeData,
  TextSaveResult,
  HierarchicalTextNode
} from '$lib/services/mockTextService';

export type { MarkdownOptions } from '$lib/services/markdownUtils';
