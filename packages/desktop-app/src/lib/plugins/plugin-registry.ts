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
  PatternDetectionConfig,
  PatternDetectionResult,
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
   * Check if a node type can have children
   * Returns true by default if plugin not found or canHaveChildren not specified
   */
  canHaveChildren(nodeType: string): boolean {
    const plugin = this.plugins.get(nodeType);
    if (!plugin || !this.enabledPlugins.has(nodeType)) {
      return true; // Default to true for unknown or disabled plugins
    }
    return plugin.config.canHaveChildren ?? true; // Default to true if not specified
  }

  /**
   * Get all pattern detection configs from enabled plugins
   * Used by TextareaController to detect node type conversions
   *
   * Issue #667: Now derives configs from plugin.pattern (new architecture)
   * for backward compatibility with existing code that uses PatternDetectionConfig
   */
  getAllPatternDetectionConfigs(): PatternDetectionConfig[] {
    const patterns: PatternDetectionConfig[] = [];

    for (const [pluginId, plugin] of this.plugins.entries()) {
      if (!this.enabledPlugins.has(pluginId)) continue;

      // Legacy: check config.patternDetection
      if (plugin.config.patternDetection) {
        patterns.push(...plugin.config.patternDetection);
      }

      // New: derive PatternDetectionConfig from plugin.pattern (Issue #667)
      if (plugin.pattern) {
        // Get slash command for additional config (desiredCursorPosition)
        const slashCommand = plugin.config.slashCommands.find(
          cmd => cmd.nodeType === pluginId
        );

        const derivedConfig: PatternDetectionConfig = {
          pattern: plugin.pattern.detect,
          targetNodeType: pluginId,
          cleanContent: !plugin.pattern.canRevert, // canRevert=true means cleanContent=false
          extractMetadata: plugin.pattern.extractMetadata,
          priority: 10, // Default priority
          desiredCursorPosition: slashCommand?.desiredCursorPosition,
          // Only include contentTemplate if desiredCursorPosition is set
          // This indicates auto-completion behavior (e.g., code-block's closing fence)
          contentTemplate: slashCommand?.desiredCursorPosition !== undefined
            ? slashCommand.contentTemplate
            : undefined
        };
        patterns.push(derivedConfig);
      }
    }

    // Sort by priority (higher priority first)
    return patterns.sort((a, b) => (b.priority || 0) - (a.priority || 0));
  }

  /**
   * Detect node type from content using plugin-owned patterns
   * Returns the plugin and match result if a pattern is detected
   *
   * Issue #667: Uses plugin.pattern instead of legacy patternDetection arrays
   *
   * @param content - The content to check for patterns
   * @returns PatternDetectionResult with plugin, pattern config, match result, and metadata, or null if no pattern matches
   */
  detectPatternInContent(content: string): PatternDetectionResult | null {
    // Default priority for pattern detection (consistent with getAllPatternDetectionConfigs)
    const DEFAULT_PATTERN_PRIORITY = 10;

    // Get all plugins with patterns, sorted by explicit priority (higher = checked first)
    // Uses the same priority system as getAllPatternDetectionConfigs for consistency
    const pluginsWithPatterns = Array.from(this.plugins.values())
      .filter(p => this.enabledPlugins.has(p.id) && p.pattern)
      .sort((a, b) => {
        // Use explicit priority from slash command config, fallback to default
        const aSlashCmd = a.config.slashCommands.find(cmd => cmd.nodeType === a.id);
        const bSlashCmd = b.config.slashCommands.find(cmd => cmd.nodeType === b.id);
        const aPriority = aSlashCmd?.priority ?? DEFAULT_PATTERN_PRIORITY;
        const bPriority = bSlashCmd?.priority ?? DEFAULT_PATTERN_PRIORITY;
        return bPriority - aPriority;
      });

    for (const plugin of pluginsWithPatterns) {
      const pattern = plugin.pattern!;
      const match = content.match(pattern.detect);

      if (match) {
        // Extract metadata if extractor function is provided
        const metadata = pattern.extractMetadata ? pattern.extractMetadata(match) : {};

        // Get slash command for additional config (desiredCursorPosition)
        // NOTE: contentTemplate is NOT used for pattern detection when canRevert is true
        // because we want to preserve the user's typed content for bidirectional conversion
        const slashCommand = plugin.config.slashCommands.find(
          cmd => cmd.nodeType === plugin.id
        );

        // Create backward-compatible PatternDetectionConfig
        // NOTE: contentTemplate is only included if desiredCursorPosition is set,
        // indicating auto-completion behavior (e.g., code-block's closing fence).
        // Headers don't have desiredCursorPosition in slash commands, so their
        // content is preserved during pattern detection.
        const config: PatternDetectionConfig = {
          pattern: pattern.detect,
          targetNodeType: plugin.id,
          cleanContent: !pattern.canRevert, // canRevert=true means cleanContent=false
          extractMetadata: pattern.extractMetadata,
          priority: 10,
          desiredCursorPosition: slashCommand?.desiredCursorPosition,
          // Only include contentTemplate if desiredCursorPosition is set
          // This indicates auto-completion behavior (e.g., code-block's closing fence)
          contentTemplate: slashCommand?.desiredCursorPosition !== undefined
            ? slashCommand.contentTemplate
            : undefined
        };

        return { plugin, config, match, metadata };
      }
    }

    return null;
  }

  /**
   * Extract metadata for a node using its plugin's extractMetadata function
   * Falls back to returning properties as-is if plugin doesn't define extractMetadata
   *
   * Issue #698: Type-agnostic BaseNodeViewer refactoring
   *
   * @param node - Node with properties from database
   * @returns Metadata object compatible with node component expectations
   */
  extractNodeMetadata(node: {
    nodeType: string;
    properties?: Record<string, unknown>;
  }): Record<string, unknown> {
    const plugin = this.plugins.get(node.nodeType);
    if (plugin && this.enabledPlugins.has(node.nodeType) && plugin.extractMetadata) {
      return plugin.extractMetadata(node);
    }
    // Default: Return properties as-is
    return node.properties || {};
  }

  /**
   * Map UI state to schema property value using plugin's mapStateToSchema function
   * Falls back to returning state as-is if plugin doesn't define mapStateToSchema
   *
   * Issue #698: Type-agnostic BaseNodeViewer refactoring
   *
   * @param nodeType - Node type to get mapping for
   * @param state - UI state value
   * @param fieldName - Schema field name
   * @returns Schema-compatible property value
   * @example
   * // Task node example:
   * mapStateToSchema('task', 'pending', 'status') // Returns: 'open'
   * mapStateToSchema('task', 'inProgress', 'status') // Returns: 'in_progress'
   */
  mapStateToSchema<T = unknown>(nodeType: string, state: string, fieldName: string): T {
    const plugin = this.plugins.get(nodeType);
    if (plugin && this.enabledPlugins.has(nodeType) && plugin.mapStateToSchema) {
      return plugin.mapStateToSchema(state, fieldName) as T;
    }
    // Default: Return state as-is
    return state as T;
  }

  /**
   * Check if a node type accepts content merges from adjacent nodes
   * Returns true by default if plugin not found or acceptsContentMerge not specified
   *
   * Issue #698: Type-agnostic BaseNodeViewer refactoring
   *
   * @param nodeType - Node type to check
   * @returns true if node type accepts content merges, false otherwise
   */
  acceptsContentMerge(nodeType: string): boolean {
    const plugin = this.plugins.get(nodeType);
    if (!plugin || !this.enabledPlugins.has(nodeType)) {
      return true; // Default to true for unknown or disabled plugins
    }
    return plugin.acceptsContentMerge ?? true; // Default to true if not specified
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
