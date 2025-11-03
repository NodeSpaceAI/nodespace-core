/**
 * FocusManagerService - Single Source of Truth for Editor Focus
 *
 * ARCHITECTURAL IMPROVEMENT (Issue #274):
 * Replaces the three-way conflict between autoFocus, focusedNodeId, and pendingCursorPositions
 * with a single reactive state that works naturally with Svelte 5.
 *
 * ARCHITECTURAL IMPROVEMENT (Issue #281):
 * Consolidates cursor positioning into unified CursorPosition type.
 * Replaces separate state properties with reactive action-based architecture.
 *
 * Problem Solved:
 * - Multiple conflicting state sources causing race conditions
 * - autoFocus checks blocking new node focus
 * - Manual _updateTrigger++ calls fighting reactivity
 * - Imperative $effect blocks for cursor positioning
 *
 * Solution:
 * - Module-level reactive state (works naturally with Svelte 5 runes)
 * - Unified cursor position tracking (single source of truth)
 * - Declarative action-based cursor positioning (no $effects)
 * - Components derive isEditing and cursorData from this single source
 *
 * FUTURE: Multi-Viewer Support (Multi-Tab/Pane)
 * When implementing multi-viewer support, extend this to per-viewer focus tracking:
 * - Track focus state per viewer: Map<viewerId, FocusState>
 * - Track active viewer for global focus determination
 * - Migration path: Add viewerId context, default to 'default' for backwards compat
 *
 * Usage:
 * ```typescript
 * import { focusManager } from '$lib/services/focus-manager.svelte';
 *
 * // Set editing state with cursor positioning
 * focusManager.focusNode(nodeId); // Default positioning
 * focusManager.focusNodeAtPosition(nodeId, position); // Absolute position
 * focusManager.focusNodeAtLine(nodeId, line); // Line-column positioning
 * focusManager.focusNodeFromArrowNav(nodeId, direction, pixelOffset); // Arrow navigation
 *
 * // Derive editing state in components
 * const isEditing = $derived(node.id === focusManager.editingNodeId);
 * const cursorData = $derived(
 *   isEditing && focusManager.editingNodeId === node.id
 *     ? focusManager.cursorPosition
 *     : null
 * );
 * ```
 */

import type { CursorPosition } from '$lib/actions/position-cursor';

export interface ArrowNavigationContext {
  direction: 'up' | 'down';
  pixelOffset: number;
}

export interface FocusState {
  nodeId: string | null;
  cursorPosition: CursorPosition | null;
  // Legacy compatibility - will be removed after full migration
  arrowNavigationContext: ArrowNavigationContext | null;
}

/**
 * Module-level reactive state for focus management
 * Using functional pattern (not class) to work naturally with Svelte 5 runes
 */

// ARCHITECTURE NOTE: These are currently global (single viewer)
// For multi-viewer: Convert to Map<viewerId, FocusState> and add activeViewerId tracking

// Single source of truth for which node is being edited
let _editingNodeId = $state<string | null>(null);

// Track which pane the editing node belongs to (for split-pane support)
// When the same node is displayed in multiple panes, only one pane can edit at a time
let _editingPaneId = $state<string>('default');

// Unified cursor position state (replaces multiple separate state variables)
let _cursorPosition = $state<CursorPosition | null>(null);

// LEGACY: Separate state for backwards compatibility during migration
// These will be removed once all call sites are updated
let _pendingCursorPosition = $state<number | null>(null);
let _arrowNavDirection = $state<'up' | 'down' | null>(null);
let _arrowNavPixelOffset = $state<number | null>(null);

// Optional node type conversion context for cursor preservation during type changes
// Separate from pendingCursorPosition to avoid conflicts with autoFocus
let _nodeTypeConversionCursorPosition = $state<number | null>(null);

export const focusManager = {
  /**
   * Public reactive getter for editing node ID
   */
  get editingNodeId(): string | null {
    return _editingNodeId;
  },

  /**
   * Public reactive getter for editing pane ID
   */
  get editingPaneId(): string {
    return _editingPaneId;
  },

  /**
   * Public reactive getter for unified cursor position
   */
  get cursorPosition(): CursorPosition | null {
    return _cursorPosition;
  },

  // ============================================================================
  // NEW API - Unified Cursor Positioning
  // ============================================================================

  /**
   * Focus a node with default cursor positioning (beginning of first line, skip syntax)
   */
  focusNode(nodeId: string, paneId: string): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'default', skipSyntax: true };
    // Clear legacy state
    _pendingCursorPosition = null;
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;
  },

  /**
   * Focus a node at a specific absolute cursor position
   */
  focusNodeAtPosition(nodeId: string, position: number, paneId: string): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'absolute', position };
    // Keep legacy state in sync during migration
    _pendingCursorPosition = position;
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;
  },

  /**
   * Focus a node at a specific line (beginning, optionally skip syntax)
   */
  focusNodeAtLine(nodeId: string, paneId: string, line?: number, skipSyntax?: boolean): void {
    const finalLine = line ?? 0;
    const finalSkipSyntax = skipSyntax ?? true;
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'line-column', line: finalLine, skipSyntax: finalSkipSyntax };
    // Clear legacy state
    _pendingCursorPosition = null;
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;
  },

  /**
   * Focus a node from arrow navigation with pixel-accurate horizontal alignment
   */
  focusNodeFromArrowNav(
    nodeId: string,
    direction: 'up' | 'down',
    pixelOffset: number,
    paneId: string
  ): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'arrow-navigation', direction, pixelOffset };
    // Keep legacy state in sync during migration
    _pendingCursorPosition = null;
    _arrowNavDirection = direction;
    _arrowNavPixelOffset = pixelOffset;
  },

  /**
   * Clear cursor position after it's been consumed by the action
   */
  clearCursorPosition(): void {
    _cursorPosition = null;
    _pendingCursorPosition = null;
  },

  // ============================================================================
  // LEGACY API - Backwards Compatibility (will be removed after migration)
  // ============================================================================

  /**
   * Public reactive getter for pending cursor position (LEGACY)
   * @deprecated Use cursorPosition instead
   */
  get pendingCursorPosition(): number | null {
    return _pendingCursorPosition;
  },

  /**
   * Public reactive getter for arrow navigation context (LEGACY)
   * @deprecated Use cursorPosition instead
   */
  get arrowNavigationContext(): ArrowNavigationContext | null {
    if (_arrowNavDirection !== null && _arrowNavPixelOffset !== null) {
      return { direction: _arrowNavDirection, pixelOffset: _arrowNavPixelOffset };
    }
    return null;
  },

  /**
   * Public reactive getter for arrow navigation direction (LEGACY)
   * @deprecated Use cursorPosition instead
   */
  get arrowNavDirection(): 'up' | 'down' | null {
    return _arrowNavDirection;
  },

  /**
   * Public reactive getter for arrow navigation pixel offset (LEGACY)
   * @deprecated Use cursorPosition instead
   */
  get arrowNavPixelOffset(): number | null {
    return _arrowNavPixelOffset;
  },

  /**
   * Public reactive getter for node type conversion cursor position (LEGACY)
   * @deprecated Use cursorPosition with type 'node-type-conversion' instead
   */
  get nodeTypeConversionCursorPosition(): number | null {
    return _nodeTypeConversionCursorPosition;
  },

  /**
   * Focus a node from node type conversion with cursor preservation
   * Integrates into unified cursor positioning system
   */
  focusNodeFromTypeConversion(nodeId: string, position: number, paneId: string): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'node-type-conversion', position };
    // Keep legacy state in sync during migration
    _pendingCursorPosition = null;
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;
    _nodeTypeConversionCursorPosition = position;
  },

  /**
   * Set which node is being edited (LEGACY)
   * @deprecated Use focusNode, focusNodeAtPosition, or focusNodeAtLine instead
   * @param nodeId - The node to edit, or null to clear editing state
   * @param paneId - The pane ID where the node is being edited (for split-pane support)
   * @param cursorPosition - Optional cursor position for precise positioning
   */
  setEditingNode(nodeId: string | null, paneId: string, cursorPosition?: number): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _pendingCursorPosition = cursorPosition ?? null;
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;

    // Update new unified state
    if (nodeId === null) {
      _cursorPosition = null;
    } else if (cursorPosition !== undefined) {
      _cursorPosition = { type: 'absolute', position: cursorPosition };
    } else {
      _cursorPosition = { type: 'default', skipSyntax: true };
    }
  },

  /**
   * Set which node is being edited via arrow navigation (LEGACY)
   * @deprecated Use focusNodeFromArrowNav instead
   * @param nodeId - The node to edit
   * @param direction - Navigation direction ('up' or 'down')
   * @param pixelOffset - Horizontal pixel offset to maintain
   */
  setEditingNodeFromArrowNavigation(
    nodeId: string,
    direction: 'up' | 'down',
    pixelOffset: number,
    paneId: string
  ): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _pendingCursorPosition = null;
    _arrowNavDirection = direction;
    _arrowNavPixelOffset = pixelOffset;
    _nodeTypeConversionCursorPosition = null; // Clear node type conversion position

    // Update new unified state
    _cursorPosition = { type: 'arrow-navigation', direction, pixelOffset };
  },

  /**
   * Set which node is being edited during node type conversion with cursor preservation (LEGACY)
   * @deprecated Use focusNodeFromTypeConversion instead
   * @param nodeId - The node to edit
   * @param cursorPosition - Cursor position to restore after conversion
   */
  setEditingNodeFromTypeConversion(nodeId: string, cursorPosition: number, paneId: string): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _pendingCursorPosition = null; // Clear pending position
    _arrowNavDirection = null; // Clear arrow navigation
    _arrowNavPixelOffset = null;
    _nodeTypeConversionCursorPosition = cursorPosition; // Set conversion-specific position

    // Update new unified state
    _cursorPosition = { type: 'node-type-conversion', position: cursorPosition };
  },

  /**
   * Clear editing state (no node is being edited)
   */
  clearEditing(): void {
    _editingNodeId = null;
    _cursorPosition = null;
    _pendingCursorPosition = null;
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;
    _nodeTypeConversionCursorPosition = null;
  },

  /**
   * Clear node type conversion cursor position after it's been consumed (LEGACY)
   * @deprecated No longer needed with action-based architecture
   */
  clearNodeTypeConversionCursorPosition(): void {
    _nodeTypeConversionCursorPosition = null;
    // Note: Don't clear _cursorPosition here - the action handles consumption
  },

  /**
   * Check if a specific node is being edited
   */
  isNodeEditing(nodeId: string): boolean {
    return _editingNodeId === nodeId;
  },

  /**
   * Clear arrow navigation context after it's been consumed (LEGACY)
   * @deprecated No longer needed with action-based architecture
   */
  clearArrowNavigationContext(): void {
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;
    // Note: Don't clear _cursorPosition here - the action handles consumption
  },

  /**
   * Get current focus state (for debugging/logging)
   */
  getCurrentState(): FocusState {
    return {
      nodeId: _editingNodeId,
      cursorPosition: _cursorPosition,
      arrowNavigationContext: this.arrowNavigationContext
    };
  }
};

// Legacy export for backwards compatibility
export function getFocusManager() {
  return focusManager;
}
