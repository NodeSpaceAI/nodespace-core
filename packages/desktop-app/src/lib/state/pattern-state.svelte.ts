/**
 * PatternState - Centralized Pattern Lifecycle Management
 *
 * Issue #664: Replaces the scattered `nodeTypeSetViaPattern` flag with an explicit
 * state machine that owns all pattern-related lifecycle.
 *
 * The key insight is that pattern detection behavior depends on HOW a node was created:
 * - 'user': Created by user (Enter key on blank node) - pattern detection enabled
 * - 'pattern': Created by pattern detection (# → header) - reversion enabled
 * - 'inherited': Created by inheriting parent's type (Enter on header → header) - also can revert
 *
 * IMPORTANT: All non-text nodes can revert to text when their syntax is deleted.
 * For example, "# Hello" → delete space → "#Hello" becomes text.
 * This applies to both pattern-detected AND inherited nodes.
 *
 * Architecture:
 * - Single source of truth for pattern lifecycle state
 * - Explicit creation source (no inference from focus manager)
 * - Svelte 5 runes for reactivity
 * - Clean API for checking capabilities (canRevert, shouldDetectPatterns)
 */

import type { PatternTemplate } from '$lib/patterns/types';
import type { PluginDefinition } from '$lib/plugins/types';

/**
 * How a node was created, which determines pattern detection behavior
 */
export type NodeCreationSource =
  | 'user' // User created (blank node, Enter key) - patterns can be detected
  | 'pattern' // Created via pattern detection (# → header) - can revert to text
  | 'inherited'; // Inherited from parent (Enter on header → header) - can also revert

/**
 * Information about a detected pattern match (LEGACY - for backward compatibility)
 * @deprecated Use PluginDefinition directly - will be removed when legacy pattern system is phased out
 */
export interface PatternMatch {
  /** The pattern template that matched */
  pattern: PatternTemplate;
  /** The regex match result */
  match: RegExpMatchArray;
  /** The node type resulting from the pattern */
  nodeType: string;
}

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
 *    - If source is 'pattern': Watch for pattern deletion, revert on mismatch
 *    - If source is 'inherited': Also watch for pattern deletion, can revert
 *
 * 3. On pattern match:
 *    - Record pattern info
 *    - Change source to 'pattern'
 *    - Enable reversion capability
 *
 * 4. On pattern deletion (source is 'pattern' or 'inherited'):
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
   * The pattern that was detected (if any) - LEGACY
   * Only set when source is 'pattern'
   * @deprecated Use _plugin instead for new plugin-owned patterns
   */
  private _detectedPattern = $state<PatternMatch | null>(null);

  /**
   * The plugin that owns this node's pattern behavior (Issue #667)
   * Replaces scattered pattern logic with plugin-owned definitions
   */
  private _plugin = $state<PluginDefinition | null>(null);

  /**
   * Create a new PatternState with explicit creation source
   *
   * @param source - How the node was created
   * @param initialPattern - If source is 'pattern', the pattern that triggered creation (LEGACY)
   * @param plugin - The plugin that owns the pattern behavior (NEW - Issue #667)
   */
  constructor(source: NodeCreationSource, initialPattern?: PatternMatch, plugin?: PluginDefinition) {
    this._creationSource = source;
    if (source === 'pattern' && initialPattern) {
      this._detectedPattern = initialPattern;
    }
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
   * Get the detected pattern (if any)
   */
  get detectedPattern(): PatternMatch | null {
    return this._detectedPattern;
  }

  /**
   * Whether this node can revert to text type
   *
   * Issue #667: Now uses plugin-owned pattern.canRevert instead of hardcoded logic.
   *
   * Both 'pattern' and 'inherited' source nodes can revert when their
   * syntax is deleted (e.g., "# Hello" → "#Hello" reverts to text).
   * Only 'user' source nodes without a detected pattern cannot revert.
   */
  get canRevert(): boolean {
    // NEW (Issue #667): Use plugin-owned pattern behavior
    // Both 'pattern' and 'inherited' source nodes respect the plugin's canRevert setting
    if (this._plugin?.pattern) {
      const source = this._creationSource;
      return (source === 'pattern' || source === 'inherited') && this._plugin.pattern.canRevert === true;
    }

    // LEGACY: Fall back to old PatternTemplate behavior for backward compatibility
    if (this._creationSource === 'pattern' && this._detectedPattern !== null) {
      // Patterns with cleanContent: true cannot revert - syntax was intentionally removed
      if (this._detectedPattern.pattern.cleanContent === true) {
        return false;
      }
      return true;
    }

    // Inherited source (legacy path) - can also revert (syntax deletion triggers type change)
    // The pattern detection in detectNodeTypeConversion handles this
    if (this._creationSource === 'inherited') {
      return true;
    }

    return false;
  }

  /**
   * Whether pattern detection should run on this node
   *
   * Only 'user' source nodes should detect patterns.
   * 'pattern' source nodes watch for reversion instead.
   * 'inherited' source nodes are type-locked.
   */
  get shouldDetectPatterns(): boolean {
    return this._creationSource === 'user';
  }

  /**
   * Whether this node should watch for pattern deletion (to revert)
   *
   * Both 'pattern' and 'inherited' source nodes watch for reversion.
   *
   * EXCEPTION: Patterns with cleanContent: true don't watch for reversion
   * because the pattern syntax was intentionally removed from content.
   */
  get shouldWatchForReversion(): boolean {
    if (this._creationSource === 'pattern' && this._detectedPattern !== null) {
      // Patterns with cleanContent: true don't watch - syntax was intentionally removed
      if (this._detectedPattern.pattern.cleanContent === true) {
        return false;
      }
      return true;
    }
    // Inherited nodes also watch - handled by detectNodeTypeConversion
    if (this._creationSource === 'inherited') {
      return true;
    }
    return false;
  }

  // ============================================================================
  // State Mutation Methods
  // ============================================================================

  /**
   * Record that a pattern was detected and matched (LEGACY)
   *
   * Called when pattern detection finds a match and converts the node type.
   * Changes source to 'pattern' and stores the pattern info for reversion.
   *
   * @param pattern - The pattern that matched
   * @param _content - The content at the time of match (unused, kept for API compatibility)
   * @deprecated Use recordPluginPatternMatch instead
   */
  recordPatternMatch(pattern: PatternMatch, _content: string): void {
    this._creationSource = 'pattern';
    this._detectedPattern = pattern;
  }

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
   * Check if content still matches the detected pattern
   *
   * Used to determine if the pattern has been deleted and node should revert.
   *
   * @param content - Current content to check
   * @returns true if pattern still matches, false if it was deleted
   */
  patternStillMatches(content: string): boolean {
    if (!this._detectedPattern) return false;

    const pattern = this._detectedPattern.pattern;
    // Reset lastIndex for global patterns
    if (pattern.regex.global) {
      pattern.regex.lastIndex = 0;
    }

    return pattern.regex.test(content);
  }

  /**
   * Attempt to revert node type to text
   *
   * Issue #667: Now uses plugin-owned pattern.revert instead of checking if detect pattern still matches.
   *
   * Called when content no longer matches the detected pattern.
   * Only succeeds if source is 'pattern' (has reversion capability).
   *
   * @param currentContent - The current content (for validation)
   * @returns true if reversion should happen, false otherwise
   */
  attemptRevert(currentContent: string): boolean {
    if (!this.canRevert) return false;

    // NEW (Issue #667): Use plugin-owned revert pattern
    if (this._plugin?.pattern?.revert) {
      // Check if content matches the revert pattern (e.g., "# " → "#" triggers reversion)
      if (this._plugin.pattern.revert.test(currentContent)) {
        this.resetToUser();
        return true;
      }
      return false;
    }

    // LEGACY: Fall back to checking if detect pattern still matches
    // Check if pattern was deleted (content no longer matches)
    if (this.patternStillMatches(currentContent)) {
      return false; // Pattern still present, no reversion
    }

    // Pattern was deleted - reset to user state
    this.resetToUser();
    return true;
  }

  /**
   * Reset state to 'user' source
   *
   * Called after successful reversion or when starting fresh.
   * Clears pattern info and enables pattern detection.
   */
  resetToUser(): void {
    this._creationSource = 'user';
    this._detectedPattern = null;
  }

  /**
   * Mark the node type as existing via pattern (LEGACY)
   *
   * Called when initializing a controller for a node whose content
   * matches its pattern (e.g., loading a header node with "# " content).
   * This enables reversion if the pattern is later deleted.
   *
   * @param pattern - The pattern that matches the current content
   * @deprecated Use setPluginPatternExists instead
   */
  setPatternExists(pattern: PatternMatch): void {
    // For 'user' source, upgrade to 'pattern' for reversion capability
    if (this._creationSource === 'user') {
      this._creationSource = 'pattern';
      this._detectedPattern = pattern;
    }
    // For 'inherited' source, just store the pattern for reversion detection
    // (inherited nodes can now also revert)
    else if (this._creationSource === 'inherited') {
      this._detectedPattern = pattern;
    }
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
    if (source !== 'pattern') {
      this._detectedPattern = null;
    }
  }

  // ============================================================================
  // Debug & Inspection
  // ============================================================================

  /**
   * Get current state for debugging
   */
  getDebugState(): {
    creationSource: NodeCreationSource;
    hasPattern: boolean;
    patternNodeType: string | null;
    canRevert: boolean;
    shouldDetectPatterns: boolean;
  } {
    return {
      creationSource: this._creationSource,
      hasPattern: this._detectedPattern !== null,
      patternNodeType: this._detectedPattern?.nodeType ?? null,
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
 * @returns Configured PatternState instance
 */
export function createPatternState(
  isTypeConversion: boolean,
  isInherited: boolean,
  currentNodeType: string
): PatternState {
  // Inherited nodes (Enter on typed node) cannot revert
  if (isInherited && currentNodeType !== 'text') {
    return new PatternState('inherited');
  }

  // Type conversion means pattern was detected
  if (isTypeConversion && currentNodeType !== 'text') {
    return new PatternState('pattern');
  }

  // Default: user-created node, patterns can be detected
  return new PatternState('user');
}
