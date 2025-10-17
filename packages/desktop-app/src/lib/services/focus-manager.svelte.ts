/**
 * FocusManagerService - Single Source of Truth for Editor Focus
 *
 * ARCHITECTURAL IMPROVEMENT (Issue #274):
 * Replaces the three-way conflict between autoFocus, focusedNodeId, and pendingCursorPositions
 * with a single reactive state that works naturally with Svelte 5.
 *
 * Problem Solved:
 * - Multiple conflicting state sources causing race conditions
 * - autoFocus checks blocking new node focus
 * - Manual _updateTrigger++ calls fighting reactivity
 *
 * Solution:
 * - Module-level reactive state (works naturally with Svelte 5 runes)
 * - Optional cursor position tracking
 * - Components derive isEditing from this single source
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
 * // Set editing state with optional cursor position
 * focusManager.setEditingNode(nodeId, cursorPosition);
 *
 * // Derive editing state in components
 * const isEditing = $derived(node.id === focusManager.editingNodeId);
 * ```
 */

export interface ArrowNavigationContext {
  direction: 'up' | 'down';
  pixelOffset: number;
}

export interface FocusState {
  nodeId: string | null;
  cursorPosition: number | null;
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

// Optional cursor position for precise positioning after node type changes
let _pendingCursorPosition = $state<number | null>(null);

// Optional arrow navigation context for pixel-accurate horizontal positioning
// Store as separate primitive values for better Svelte reactivity tracking
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
   * Public reactive getter for pending cursor position
   */
  get pendingCursorPosition(): number | null {
    return _pendingCursorPosition;
  },

  /**
   * Public reactive getter for arrow navigation context
   */
  get arrowNavigationContext(): ArrowNavigationContext | null {
    if (_arrowNavDirection !== null && _arrowNavPixelOffset !== null) {
      return { direction: _arrowNavDirection, pixelOffset: _arrowNavPixelOffset };
    }
    return null;
  },

  /**
   * Public reactive getter for arrow navigation direction (primitive for reactivity)
   */
  get arrowNavDirection(): 'up' | 'down' | null {
    return _arrowNavDirection;
  },

  /**
   * Public reactive getter for arrow navigation pixel offset (primitive for reactivity)
   */
  get arrowNavPixelOffset(): number | null {
    return _arrowNavPixelOffset;
  },

  /**
   * Public reactive getter for node type conversion cursor position
   */
  get nodeTypeConversionCursorPosition(): number | null {
    return _nodeTypeConversionCursorPosition;
  },

  /**
   * Set which node is being edited
   * @param nodeId - The node to edit, or null to clear editing state
   * @param cursorPosition - Optional cursor position for precise positioning
   */
  setEditingNode(nodeId: string | null, cursorPosition?: number): void {
    _editingNodeId = nodeId;
    _pendingCursorPosition = cursorPosition ?? null;
    _arrowNavDirection = null; // Clear arrow navigation when setting via cursor position
    _arrowNavPixelOffset = null;
  },

  /**
   * Set which node is being edited via arrow navigation
   * @param nodeId - The node to edit
   * @param direction - Navigation direction ('up' or 'down')
   * @param pixelOffset - Horizontal pixel offset to maintain
   */
  setEditingNodeFromArrowNavigation(
    nodeId: string,
    direction: 'up' | 'down',
    pixelOffset: number
  ): void {
    _editingNodeId = nodeId;
    _pendingCursorPosition = null; // Clear cursor position when using arrow navigation
    _arrowNavDirection = direction;
    _arrowNavPixelOffset = pixelOffset;
    _nodeTypeConversionCursorPosition = null; // Clear node type conversion position
  },

  /**
   * Set which node is being edited during node type conversion with cursor preservation
   * @param nodeId - The node to edit
   * @param cursorPosition - Cursor position to restore after conversion
   */
  setEditingNodeFromTypeConversion(nodeId: string, cursorPosition: number): void {
    _editingNodeId = nodeId;
    _pendingCursorPosition = null; // Clear pending position
    _arrowNavDirection = null; // Clear arrow navigation
    _arrowNavPixelOffset = null;
    _nodeTypeConversionCursorPosition = cursorPosition; // Set conversion-specific position
  },

  /**
   * Clear editing state (no node is being edited)
   */
  clearEditing(): void {
    _editingNodeId = null;
    _pendingCursorPosition = null;
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;
    _nodeTypeConversionCursorPosition = null;
  },

  /**
   * Clear cursor position after it's been consumed
   */
  clearCursorPosition(): void {
    _pendingCursorPosition = null;
  },

  /**
   * Clear node type conversion cursor position after it's been consumed
   */
  clearNodeTypeConversionCursorPosition(): void {
    _nodeTypeConversionCursorPosition = null;
  },

  /**
   * Check if a specific node is being edited
   */
  isNodeEditing(nodeId: string): boolean {
    return _editingNodeId === nodeId;
  },

  /**
   * Clear arrow navigation context after it's been consumed
   */
  clearArrowNavigationContext(): void {
    _arrowNavDirection = null;
    _arrowNavPixelOffset = null;
  },

  /**
   * Get current focus state (for debugging/logging)
   */
  getCurrentState(): FocusState {
    return {
      nodeId: _editingNodeId,
      cursorPosition: _pendingCursorPosition,
      arrowNavigationContext: this.arrowNavigationContext
    };
  }
};

// Legacy export for backwards compatibility
export function getFocusManager() {
  return focusManager;
}
