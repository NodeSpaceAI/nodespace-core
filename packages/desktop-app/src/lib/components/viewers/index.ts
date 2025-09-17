/**
 * Node Viewer Registry System - MIGRATED TO UNIFIED PLUGIN SYSTEM
 *
 * This file now acts as a compatibility layer that forwards to the unified
 * plugin registry. All functionality has been moved to the plugin system.
 */

// Import the unified plugin system
import { pluginRegistry } from '$lib/plugins/index';
import { registerCorePlugins } from '$lib/plugins/corePlugins';
import type { NodeViewerComponent, ViewerRegistration } from '$lib/types/nodeViewers.js';

/**
 * Legacy ViewerRegistry class - forwards to unified plugin system
 * @deprecated Use pluginRegistry directly instead
 */
class ViewerRegistry {
  /**
   * @deprecated Use pluginRegistry.register() with full plugin definition instead
   */
  register(_nodeType: string, _registration: ViewerRegistration): void {
    console.warn(`ViewerRegistry.register() is deprecated. Use pluginRegistry.register() instead.`);
    // This is a breaking change - we don't support the old API
    throw new Error(
      'ViewerRegistry.register() is no longer supported. Use the unified plugin system.'
    );
  }

  /**
   * Get a viewer component for a node type
   * Forwards to the unified plugin registry
   */
  async getViewer(nodeType: string): Promise<NodeViewerComponent | null> {
    return pluginRegistry.getViewer(nodeType);
  }

  /**
   * Check if a custom viewer is registered for a node type
   * Forwards to the unified plugin registry
   */
  hasViewer(nodeType: string): boolean {
    return pluginRegistry.hasViewer(nodeType);
  }

  /**
   * Get all registered node types
   * Forwards to the unified plugin registry
   */
  getRegisteredTypes(): string[] {
    return pluginRegistry
      .getAllPlugins()
      .filter((plugin) => plugin.viewer)
      .map((plugin) => plugin.id);
  }

  /**
   * Clear all registrations (useful for testing)
   * Forwards to the unified plugin registry
   */
  clear(): void {
    pluginRegistry.clear();
  }
}

// Create the legacy registry instance that forwards to the unified system
export const viewerRegistry = new ViewerRegistry();

// Initialize the unified plugin system with core plugins
registerCorePlugins(pluginRegistry);

export { ViewerRegistry };
export type { ViewerRegistration, NodeViewerComponent };

// Re-export the unified plugin system for direct access
export { pluginRegistry } from '$lib/plugins/index';
