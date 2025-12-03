/**
 * PatternState - Centralized Pattern Lifecycle Management
 *
 * Issue #664: Replaces the scattered `nodeTypeSetViaPattern` flag with an explicit
 * state machine that owns all pattern-related lifecycle.
 *
 * The key insight is that pattern detection behavior depends on HOW a node was created:
 * - 'user': Created by user (Enter key on blank node) - pattern detection enabled
 * - 'pattern': Created by pattern detection (# → header) - reversion enabled if plugin allows
 * - 'inherited': Created by inheriting parent's type (Enter on header → header) - reversion controlled by plugin
 *
 * Issue #667: Pattern behavior is now owned by plugins via the `pattern` property.
 * Each plugin can define:
 * - `detect`: Regex to detect the pattern
 * - `canRevert`: Whether the node can revert to text when pattern is deleted
 * - `revert`: Regex to check if pattern was deleted (triggers reversion)
 *
 * Example: Header plugin has canRevert: true (# Hello → #Hello reverts to text)
 * Example: Task plugin has canRevert: false (checkbox syntax is removed from content)
 *
 * Architecture:
 * - Single source of truth for pattern lifecycle state
 * - Explicit creation source (no inference from focus manager)
 * - Plugin-owned pattern behavior
 * - Svelte 5 runes for reactivity
 * - Clean API for checking capabilities (canRevert, shouldDetectPatterns)
 */

import type { PluginDefinition } from '$lib/plugins/types';

/**
 * How a node was created, which determines pattern detection behavior
 */
export type NodeCreationSource =
  | 'user' // User created (blank node, Enter key) - patterns can be detected
  | 'pattern' // Created via pattern detection (# → header) - can revert to text if plugin allows
  | 'inherited'; // Inherited from parent (Enter on header → header) - reversion controlled by plugin

/**
 * PatternState manages the lifecycle of pattern detection for a single TextareaController.
 *
 * State Machine:
 * 1. On creation: Set source based on how node was created
 *    - 'user' for blank nodes, Enter on blank
 *    - 'pattern' for pattern-triggered type conversions
 *    - 'inherited' for Enter key on typed nodes (header → header)
 *
 * 2. During editing:
 *    - If source is 'user': Watch for patterns, convert on match
 *    - If source is 'pattern' and canRevert: Watch for pattern deletion, revert on mismatch
 *    - If source is 'inherited' and canRevert: Also watch for pattern deletion
 *
 * 3. On pattern match:
 *    - Record plugin info
 *    - Change source to 'pattern'
 *    - Enable reversion capability if plugin allows
 *
 * 4. On pattern deletion (source is 'pattern' or 'inherited' with canRevert: true):
 *    - Revert node type to 'text'
 *    - Reset source to 'user'
 *    - Continue watching for new patterns
 */
export class PatternState {
  /**
   * How this node was created - determines pattern detection behavior
   * Using $state for Svelte 5 reactivity
   */
  private _creationSource = $state<NodeCreationSource>('user');

  /**
   * The plugin that owns this node's pattern behavior (Issue #667)
   * Contains detect, canRevert, and revert settings
   */
  private _plugin = $state<PluginDefinition | null>(null);

  /**
   * Create a new PatternState with explicit creation source
   *
   * @param source - How the node was created
   * @param plugin - The plugin that owns the pattern behavior
   */
  constructor(source: NodeCreationSource, plugin?: PluginDefinition) {
    this._creationSource = source;
    if (plugin) {
      this._plugin = plugin;
    }
  }

  // ============================================================================
  // Public Getters - Reactive properties for external access
  // ============================================================================

  /**
   * Get the current creation source
   */
  get creationSource(): NodeCreationSource {
    return this._creationSource;
  }

  /**
   * Get the plugin that owns this node's pattern behavior
   */
  get plugin(): PluginDefinition | null {
    return this._plugin;
  }

  /**
   * Whether this node can revert to text type
   *
   * Issue #667: Uses plugin-owned pattern.canRevert setting.
   *
   * Both 'pattern' and 'inherited' source nodes can revert when their
   * syntax is deleted (e.g., "# Hello" → "#Hello" reverts to text),
   * but ONLY if the plugin's pattern.canRevert is true.
   *
   * Task nodes have canRevert: false because the checkbox syntax is
   * removed from content (cleanContent pattern).
   */
  get canRevert(): boolean {
    // Plugin-owned pattern behavior determines reversion capability
    if (this._plugin?.pattern) {
      const source = this._creationSource;
      return (source === 'pattern' || source === 'inherited') && this._plugin.pattern.canRevert === true;
    }

    // No plugin set - cannot revert
    return false;
  }

  /**
   * Whether pattern detection should run on this node
   *
   * Only 'user' source nodes should detect patterns.
   * 'pattern' source nodes watch for reversion instead.
   * 'inherited' source nodes are type-locked (but may revert if plugin allows).
   */
  get shouldDetectPatterns(): boolean {
    return this._creationSource === 'user';
  }

  /**
   * Whether this node should watch for pattern deletion (to revert)
   *
   * Both 'pattern' and 'inherited' source nodes watch for reversion,
   * but only if the plugin's canRevert is true.
   */
  get shouldWatchForReversion(): boolean {
    // Only watch for reversion if the node can actually revert
    return this.canRevert;
  }

  // ============================================================================
  // State Mutation Methods
  // ============================================================================

  /**
   * Record that a plugin pattern was detected and matched (Issue #667)
   *
   * Called when pattern detection finds a match and converts the node type.
   * Changes source to 'pattern' and stores the plugin for reversion.
   *
   * @param plugin - The plugin whose pattern matched
   */
  recordPluginPatternMatch(plugin: PluginDefinition): void {
    this._creationSource = 'pattern';
    this._plugin = plugin;
  }

  /**
   * Attempt to revert node type to text
   *
   * Issue #667: Uses plugin-owned pattern.revert to check if content should revert.
   *
   * Called when content no longer matches the detected pattern.
   * Only succeeds if the plugin has canRevert: true and the revert pattern matches.
   *
   * @param currentContent - The current content (for validation)
   * @returns true if reversion should happen, false otherwise
   */
  attemptRevert(currentContent: string): boolean {
    if (!this.canRevert) return false;

    // Use plugin-owned revert pattern (e.g., "# " → "#" triggers reversion)
    if (this._plugin?.pattern?.revert) {
      if (this._plugin.pattern.revert.test(currentContent)) {
        this.resetToUser();
        return true;
      }
    }

    return false;
  }

  /**
   * Reset state to 'user' source
   *
   * Called after successful reversion or when starting fresh.
   * Clears plugin info and enables pattern detection.
   */
  resetToUser(): void {
    this._creationSource = 'user';
    this._plugin = null;
  }

  /**
   * Mark the node type as existing via plugin pattern (Issue #667)
   *
   * Called when initializing a controller for a node whose content
   * matches its pattern (e.g., loading a header node with "# " content).
   * This enables reversion if the pattern is later deleted.
   *
   * @param plugin - The plugin whose pattern matches the current content
   */
  setPluginPatternExists(plugin: PluginDefinition): void {
    // For 'user' source, upgrade to 'pattern' for reversion capability
    if (this._creationSource === 'user') {
      this._creationSource = 'pattern';
      this._plugin = plugin;
    }
    // For 'inherited' source, just store the plugin for reversion detection
    else if (this._creationSource === 'inherited') {
      this._plugin = plugin;
    }
  }

  /**
   * Update the creation source
   *
   * Used for explicit source changes (e.g., inherited nodes).
   *
   * @param source - The new creation source
   */
  setCreationSource(source: NodeCreationSource): void {
    this._creationSource = source;
  }

  // ============================================================================
  // Debug & Inspection
  // ============================================================================

  /**
   * Get current state for debugging
   */
  getDebugState(): {
    creationSource: NodeCreationSource;
    hasPlugin: boolean;
    pluginId: string | null;
    canRevert: boolean;
    shouldDetectPatterns: boolean;
  } {
    return {
      creationSource: this._creationSource,
      hasPlugin: this._plugin !== null,
      pluginId: this._plugin?.id ?? null,
      canRevert: this.canRevert,
      shouldDetectPatterns: this.shouldDetectPatterns
    };
  }
}

/**
 * Factory function to create a PatternState based on node creation context
 *
 * @param isTypeConversion - Whether this is a node type conversion (from focus manager)
 * @param isInherited - Whether the node inherits its type from parent (Enter key)
 * @param currentNodeType - The current node type
 * @param plugin - Optional plugin for the node type (provides pattern behavior)
 * @returns Configured PatternState instance
 */
export function createPatternState(
  isTypeConversion: boolean,
  isInherited: boolean,
  currentNodeType: string,
  plugin?: PluginDefinition
): PatternState {
  // Inherited nodes (Enter on typed node) - reversion controlled by plugin
  if (isInherited && currentNodeType !== 'text') {
    return new PatternState('inherited', plugin);
  }

  // Type conversion means pattern was detected
  if (isTypeConversion && currentNodeType !== 'text') {
    return new PatternState('pattern', plugin);
  }

  // Default: user-created node, patterns can be detected
  return new PatternState('user', plugin);
}
