/**
 * Node Reference Components - Core Component System
 * 
 * Exports the base node reference component and component registry
 * for the component-based node decoration system.
 */

// Core component imports
import BaseNodeReference from './BaseNodeReference.svelte';
import type { ComponentDecoration } from '../../types/ComponentDecoration';
import type { ComponentType } from 'svelte';

// Component constructor type for Node Reference components
type NodeReferenceComponent = ComponentType;

// Export components
export { BaseNodeReference };

// Core node reference components registry
// Plugin node types can extend this registry at build time
export const NODE_REFERENCE_COMPONENTS: Record<string, NodeReferenceComponent> = {
  // Core node types (built-in)
  BaseNodeReference: BaseNodeReference,
  
  // All node types use BaseNodeReference for now
  // Specific implementations will be added when properly specified
  base: BaseNodeReference,
  text: BaseNodeReference,
  task: BaseNodeReference,
  user: BaseNodeReference,
  date: BaseNodeReference,
  document: BaseNodeReference,
  ai_chat: BaseNodeReference
};

/**
 * Get component constructor by node type
 */
export function getNodeReferenceComponent(nodeType: string): NodeReferenceComponent {
  return NODE_REFERENCE_COMPONENTS[nodeType] || NODE_REFERENCE_COMPONENTS.base;
}

/**
 * Get component name for debugging/inspection
 */
export function getComponentName(component: NodeReferenceComponent): string {
  if (component === BaseNodeReference) return 'BaseNodeReference';
  return 'BaseNodeReference'; // All components are BaseNodeReference for now
}

/**
 * Get component constructor by name (for hydration)
 */
export function getComponentByName(componentName: string): NodeReferenceComponent {
  switch (componentName) {
    case 'BaseNodeReference':
      return BaseNodeReference;
    default:
      return BaseNodeReference; // Default fallback
  }
}

/**
 * Creates a basic component decoration using BaseNodeReference
 */
export function createNodeReferenceDecoration(nodeType: string, props: Record<string, unknown>): ComponentDecoration {
  return {
    component: getNodeReferenceComponent(nodeType) as ComponentDecoration['component'],
    props,
  };
}