/**
 * Node Reference Components Registry
 *
 * Central registry for all node reference components used in the universal
 * reference system. Supports both core components and plugin components.
 */

import BaseNodeReference from '../base-node-reference.svelte';
import type { SvelteComponent } from 'svelte';

// Component type for Svelte components
export type NodeReferenceComponent = new (...args: unknown[]) => SvelteComponent;

/**
 * Registry of available node reference components
 * Core components are always available, plugin components are added at build time
 */
export const NODE_REFERENCE_COMPONENTS: Record<string, NodeReferenceComponent> = {
  // Core component (used as base/fallback for all node types)
  BaseNodeReference: BaseNodeReference as NodeReferenceComponent,
  base: BaseNodeReference as NodeReferenceComponent,

  // Node type specific mappings (all use BaseNodeReference for now)
  text: BaseNodeReference as NodeReferenceComponent,
  task: BaseNodeReference as NodeReferenceComponent,
  user: BaseNodeReference as NodeReferenceComponent,
  date: BaseNodeReference as NodeReferenceComponent,
  document: BaseNodeReference as NodeReferenceComponent,
  ai_chat: BaseNodeReference as NodeReferenceComponent

  // Future plugin components will be registered here at build time
  // pdf: PdfNodeReference (example)
  // image: ImageNodeReference (example)
  // code: CodeNodeReference (example)
};

/**
 * Get the appropriate component for a node type
 * Falls back to BaseNodeReference for unknown types
 */
export function getNodeReferenceComponent(nodeType: string): NodeReferenceComponent {
  return NODE_REFERENCE_COMPONENTS[nodeType] || NODE_REFERENCE_COMPONENTS.base;
}

/**
 * Register a new node reference component (for plugins)
 * Used at build time to add plugin components
 */
export function registerNodeReferenceComponent(
  nodeType: string,
  component: NodeReferenceComponent
): void {
  NODE_REFERENCE_COMPONENTS[nodeType] = component;
}

/**
 * Get all registered component types
 */
export function getRegisteredNodeTypes(): string[] {
  return Object.keys(NODE_REFERENCE_COMPONENTS).filter(
    (key) => key !== 'base' && key !== 'BaseNodeReference'
  );
}

/**
 * Check if a component is registered for a node type
 */
export function hasComponentForNodeType(nodeType: string): boolean {
  return nodeType in NODE_REFERENCE_COMPONENTS;
}

// Export the BaseNodeReference as default
export default BaseNodeReference;

// Re-export BaseNodeReference for direct imports
export { BaseNodeReference };
