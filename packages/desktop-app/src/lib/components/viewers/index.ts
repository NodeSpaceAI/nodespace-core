/**
 * Node Viewer Registry System
 * 
 * Provides a plugin-style architecture for node viewers where:
 * - Simple node types (like text) use BaseNode directly
 * - Complex node types can register custom viewers
 * - Unknown types gracefully fall back to BaseNode
 * - Supports lazy loading for performance
 */

import type { 
  NodeViewerComponent, 
  ViewerRegistration 
} from '$lib/types/nodeViewers.js';

class ViewerRegistry {
  private viewers = new Map<string, ViewerRegistration>();
  private loadedComponents = new Map<string, NodeViewerComponent>();

  /**
   * Register a viewer for a specific node type
   */
  register(nodeType: string, registration: ViewerRegistration): void {
    this.viewers.set(nodeType, registration);
  }

  /**
   * Get a viewer component for a node type
   * Returns null if no custom viewer is registered (falls back to BaseNode)
   */
  async getViewer(nodeType: string): Promise<NodeViewerComponent | null> {
    // Check if already loaded
    if (this.loadedComponents.has(nodeType)) {
      return this.loadedComponents.get(nodeType)!;
    }

    const registration = this.viewers.get(nodeType);
    if (!registration) {
      return null; // Fallback to BaseNode
    }

    // Load component if lazy loading
    if (registration.lazyLoad) {
      try {
        const module = await registration.lazyLoad();
        this.loadedComponents.set(nodeType, module.default);
        return module.default;
      } catch (error) {
        console.warn(`Failed to lazy load viewer for ${nodeType}:`, error);
        return null;
      }
    }

    // Use direct component reference
    if (registration.component) {
      this.loadedComponents.set(nodeType, registration.component);
      return registration.component;
    }

    return null;
  }

  /**
   * Check if a custom viewer is registered for a node type
   */
  hasViewer(nodeType: string): boolean {
    return this.viewers.has(nodeType);
  }

  /**
   * Get all registered node types
   */
  getRegisteredTypes(): string[] {
    return Array.from(this.viewers.keys());
  }

  /**
   * Clear all registrations (useful for testing)
   */
  clear(): void {
    this.viewers.clear();
    this.loadedComponents.clear();
  }
}

// Create the global registry instance
export const viewerRegistry = new ViewerRegistry();

// Register viewers for all node types including text
// Text nodes now use TextNodeViewer for smart multiline behavior

viewerRegistry.register('text', {
  lazyLoad: () => import('./text-node-viewer.svelte'),
  priority: 1
});

viewerRegistry.register('date', {
  lazyLoad: () => import('./date-node-viewer.svelte'),
  priority: 1
});

viewerRegistry.register('task', {
  lazyLoad: () => import('./task-node-viewer.svelte'),
  priority: 1
});

viewerRegistry.register('ai-chat', {
  lazyLoad: () => import('./ai-chat-node-viewer.svelte'),
  priority: 1
});

export { ViewerRegistry };
export type { ViewerRegistration, NodeViewerComponent };