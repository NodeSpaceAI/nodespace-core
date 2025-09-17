/**
 * Node Reference Components Registry - MIGRATED TO UNIFIED PLUGIN SYSTEM
 *
 * This file now acts as a compatibility layer that forwards to the unified
 * plugin registry. All functionality has been moved to the plugin system.
 */

import BaseNodeReference from '../base-node-reference.svelte';
import { pluginRegistry } from '$lib/plugins/index';
import type { SvelteComponent } from 'svelte';

// Component type for Svelte components
export type NodeReferenceComponent = new (...args: unknown[]) => SvelteComponent;

/**
 * Legacy registry object - now forwards to unified plugin system
 * @deprecated Use pluginRegistry.getReferenceComponent() instead
 */
export const NODE_REFERENCE_COMPONENTS: Record<string, NodeReferenceComponent> = new Proxy(
  {} as Record<string, NodeReferenceComponent>,
  {
    get(target, prop: string) {
      // Handle special base cases
      if (prop === 'base' || prop === 'BaseNodeReference') {
        return BaseNodeReference as NodeReferenceComponent;
      }

      // Try to get from unified plugin registry
      const component = pluginRegistry.getReferenceComponent(prop);
      if (component) {
        return component;
      }

      // Fallback to base component
      return BaseNodeReference as NodeReferenceComponent;
    },

    set(_target, _prop: string, _value: NodeReferenceComponent) {
      console.warn(
        `NODE_REFERENCE_COMPONENTS assignment is deprecated. Use the unified plugin system instead.`
      );
      // We don't support the old API anymore
      return false;
    },

    has(target, prop: string) {
      if (prop === 'base' || prop === 'BaseNodeReference') {
        return true;
      }
      return pluginRegistry.hasReferenceComponent(prop);
    },

    ownKeys(_target) {
      const pluginIds = pluginRegistry
        .getAllPlugins()
        .filter((plugin) => plugin.reference)
        .map((plugin) => plugin.id);
      return ['base', 'BaseNodeReference', ...pluginIds];
    }
  }
);

/**
 * Get the appropriate component for a node type
 * Now forwards to the unified plugin registry
 */
export function getNodeReferenceComponent(nodeType: string): NodeReferenceComponent {
  const component = pluginRegistry.getReferenceComponent(nodeType);
  return component || (BaseNodeReference as NodeReferenceComponent);
}

/**
 * Register a new node reference component (for plugins)
 * @deprecated Use pluginRegistry.register() with full plugin definition instead
 */
export function registerNodeReferenceComponent(
  _nodeType: string,
  _component: NodeReferenceComponent
): void {
  console.warn(
    `registerNodeReferenceComponent() is deprecated. Use the unified plugin system instead.`
  );
  // This is a breaking change - we don't support the old API
  throw new Error(
    'registerNodeReferenceComponent() is no longer supported. Use the unified plugin system.'
  );
}

/**
 * Get all registered component types
 * Now forwards to the unified plugin registry
 */
export function getRegisteredNodeTypes(): string[] {
  return pluginRegistry
    .getAllPlugins()
    .filter((plugin) => plugin.reference)
    .map((plugin) => plugin.id);
}

/**
 * Check if a component is registered for a node type
 * Now forwards to the unified plugin registry
 */
export function hasComponentForNodeType(nodeType: string): boolean {
  return pluginRegistry.hasReferenceComponent(nodeType);
}

// Export the BaseNodeReference as default
export default BaseNodeReference;

// Re-export BaseNodeReference for direct imports
export { BaseNodeReference };
