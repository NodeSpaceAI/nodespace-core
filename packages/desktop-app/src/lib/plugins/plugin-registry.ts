/**
 * Unified Plugin Registry
 *
 * Consolidates functionality from:
 * - ViewerRegistry (viewer components)
 * - NODE_REFERENCE_COMPONENTS (reference components)
 * - BasicNodeTypeRegistry (node definitions + slash commands)
 *
 * Breaking change: This replaces all three systems with a single unified approach.
 */

import type {
  PluginDefinition,
  NodeViewerComponent,
  NodeComponent,
  NodeReferenceComponent,
  SlashCommandDefinition,
  RegistryStats,
  PluginLifecycleEvents
} from './types';

export class PluginRegistry {
  private plugins = new Map<string, PluginDefinition>();
  private loadedViewers = new Map<string, NodeViewerComponent>();
  private loadedNodes = new Map<string, NodeComponent>();
  private loadedReferences = new Map<string, NodeReferenceComponent>();
  private enabledPlugins = new Set<string>();
  private lifecycleEvents: PluginLifecycleEvents = {};

  constructor(events?: PluginLifecycleEvents) {
    this.lifecycleEvents = events || {};
  }

  /**
   * Register a complete plugin definition
   */
  register(plugin: PluginDefinition): void {
    this.plugins.set(plugin.id, plugin);
    this.enabledPlugins.add(plugin.id);

    // Pre-load reference component if provided (they're not lazy)
    if (plugin.reference) {
      this.loadedReferences.set(plugin.id, plugin.reference.component);
    }

    this.lifecycleEvents.onRegister?.(plugin);
  }

  /**
   * Unregister a plugin
   */
  unregister(pluginId: string): void {
    this.plugins.delete(pluginId);
    this.enabledPlugins.delete(pluginId);
    this.loadedViewers.delete(pluginId);
    this.loadedNodes.delete(pluginId);
    this.loadedReferences.delete(pluginId);

    this.lifecycleEvents.onUnregister?.(pluginId);
  }

  /**
   * Enable/disable a plugin without removing it
   */
  setEnabled(pluginId: string, enabled: boolean): void {
    if (!this.plugins.has(pluginId)) {
      throw new Error(`Plugin ${pluginId} is not registered`);
    }

    if (enabled) {
      this.enabledPlugins.add(pluginId);
      this.lifecycleEvents.onEnable?.(pluginId);
    } else {
      this.enabledPlugins.delete(pluginId);
      this.lifecycleEvents.onDisable?.(pluginId);
    }
  }

  /**
   * Get a viewer component for a node type
   * Returns null if no viewer is registered (fallback to base viewer)
   */
  async getViewer(nodeType: string): Promise<NodeViewerComponent | null> {
    // Check if already loaded
    if (this.loadedViewers.has(nodeType)) {
      return this.loadedViewers.get(nodeType)!;
    }

    const plugin = this.plugins.get(nodeType);
    if (!plugin || !this.enabledPlugins.has(nodeType) || !plugin.viewer) {
      return null; // No viewer available
    }

    const registration = plugin.viewer;

    // Load component if lazy loading
    if (registration.lazyLoad) {
      try {
        const module = await registration.lazyLoad();
        this.loadedViewers.set(nodeType, module.default);
        return module.default;
      } catch (error) {
        console.warn(`Failed to lazy load viewer for ${nodeType}:`, error);
        return null;
      }
    }

    // Use direct component reference
    if (registration.component) {
      this.loadedViewers.set(nodeType, registration.component);
      return registration.component;
    }

    return null;
  }

  /**
   * Get a node component for a node type
   * Returns null if no node component is registered (fallback to base node)
   */
  async getNodeComponent(nodeType: string): Promise<NodeComponent | null> {
    // Check if already loaded
    if (this.loadedNodes.has(nodeType)) {
      return this.loadedNodes.get(nodeType)!;
    }

    const plugin = this.plugins.get(nodeType);
    if (!plugin || !this.enabledPlugins.has(nodeType) || !plugin.node) {
      return null; // No node component available
    }

    const registration = plugin.node;

    // Load component if lazy loading
    if (registration.lazyLoad) {
      try {
        const module = await registration.lazyLoad();
        this.loadedNodes.set(nodeType, module.default);
        return module.default;
      } catch (error) {
        console.warn(`Failed to lazy load node component for ${nodeType}:`, error);
        return null;
      }
    }

    // Use direct component reference
    if (registration.component) {
      this.loadedNodes.set(nodeType, registration.component);
      return registration.component;
    }

    return null;
  }

  /**
   * Get a reference component for a node type
   * Returns null if no component is registered (fallback to base reference)
   */
  getReferenceComponent(nodeType: string): NodeReferenceComponent | null {
    if (!this.loadedReferences.has(nodeType)) {
      return null;
    }

    const plugin = this.plugins.get(nodeType);
    if (!plugin || !this.enabledPlugins.has(nodeType)) {
      return null;
    }

    return this.loadedReferences.get(nodeType) || null;
  }

  /**
   * Get all slash commands from enabled plugins
   */
  getAllSlashCommands(): SlashCommandDefinition[] {
    const commands: SlashCommandDefinition[] = [];

    for (const [pluginId, plugin] of this.plugins.entries()) {
      if (this.enabledPlugins.has(pluginId)) {
        commands.push(...plugin.config.slashCommands);
      }
    }

    // Sort by priority (higher priority first), then by name
    return commands.sort((a, b) => {
      const priorityDiff = (b.priority || 0) - (a.priority || 0);
      return priorityDiff !== 0 ? priorityDiff : a.name.localeCompare(b.name);
    });
  }

  /**
   * Find a slash command by id
   */
  findSlashCommand(commandId: string): SlashCommandDefinition | null {
    for (const [pluginId, plugin] of this.plugins.entries()) {
      if (this.enabledPlugins.has(pluginId)) {
        const command = plugin.config.slashCommands.find((cmd) => cmd.id === commandId);
        if (command) {
          return command;
        }
      }
    }
    return null;
  }

  /**
   * Filter slash commands by query
   */
  filterSlashCommands(query: string): SlashCommandDefinition[] {
    const allCommands = this.getAllSlashCommands();

    if (!query.trim()) {
      return allCommands;
    }

    const lowerQuery = query.toLowerCase();
    return allCommands.filter(
      (command) =>
        command.name.toLowerCase().includes(lowerQuery) ||
        command.description.toLowerCase().includes(lowerQuery) ||
        command.shortcut?.toLowerCase().includes(lowerQuery)
    );
  }

  /**
   * Get a plugin definition by id
   */
  getPlugin(pluginId: string): PluginDefinition | null {
    return this.plugins.get(pluginId) || null;
  }

  /**
   * Get all registered plugin definitions
   */
  getAllPlugins(): PluginDefinition[] {
    return Array.from(this.plugins.values());
  }

  /**
   * Get all enabled plugin definitions
   */
  getEnabledPlugins(): PluginDefinition[] {
    return Array.from(this.plugins.values()).filter((plugin) => this.enabledPlugins.has(plugin.id));
  }

  /**
   * Check if a plugin is registered
   */
  hasPlugin(pluginId: string): boolean {
    return this.plugins.has(pluginId);
  }

  /**
   * Check if a plugin is enabled
   */
  isEnabled(pluginId: string): boolean {
    return this.enabledPlugins.has(pluginId);
  }

  /**
   * Check if a viewer is available for a node type
   */
  hasViewer(nodeType: string): boolean {
    const plugin = this.plugins.get(nodeType);
    return !!(plugin && this.enabledPlugins.has(nodeType) && plugin.viewer);
  }

  /**
   * Check if a node component is available for a node type
   */
  hasNodeComponent(nodeType: string): boolean {
    const plugin = this.plugins.get(nodeType);
    return !!(plugin && this.enabledPlugins.has(nodeType) && plugin.node);
  }

  /**
   * Check if a reference component is available for a node type
   */
  hasReferenceComponent(nodeType: string): boolean {
    const plugin = this.plugins.get(nodeType);
    return !!(plugin && this.enabledPlugins.has(nodeType) && plugin.reference);
  }

  /**
   * Get registry statistics for debugging
   */
  getStats(): RegistryStats {
    const enabledPlugins = this.getEnabledPlugins();

    return {
      pluginsCount: this.plugins.size,
      viewersCount: enabledPlugins.filter((p) => p.viewer).length,
      referencesCount: enabledPlugins.filter((p) => p.reference).length,
      slashCommandsCount: this.getAllSlashCommands().length,
      plugins: Array.from(this.plugins.keys())
    };
  }

  /**
   * Clear all plugins (useful for testing)
   */
  clear(): void {
    this.plugins.clear();
    this.enabledPlugins.clear();
    this.loadedViewers.clear();
    this.loadedNodes.clear();
    this.loadedReferences.clear();
  }
}

// Create the global registry instance as a simple module-level export
// This creates a singleton within each module context (Node vs Happy-DOM in tests)
//
// IMPORTANT for testing: In Vitest, register plugins in setup.ts (not global-setup.ts)
// to avoid module duplication between Node and Happy-DOM contexts. Each context gets
// its own module graph, so setup.ts ensures plugins are registered in the same context
// as the components that use them.
export const pluginRegistry = new PluginRegistry({
  onRegister: (plugin) => {
    console.debug(`Plugin registered: ${plugin.name} (${plugin.id})`);
  },
  onUnregister: (pluginId) => {
    console.debug(`Plugin unregistered: ${pluginId}`);
  }
});
