/**
 * FocusManagerService - Single Source of Truth for Editor Focus
 *
 * ARCHITECTURAL IMPROVEMENT:
 * Replaces the three-way conflict between autoFocus, focusedNodeId, and pendingCursorPositions
 * with a single reactive state that works naturally with Svelte 5.
 *
 * ARCHITECTURAL IMPROVEMENT:
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
 * focusManager.focusNode(nodeId, paneId); // Default positioning
 * focusManager.focusNodeAtPosition(nodeId, position, paneId); // Absolute position
 * focusManager.focusNodeAtLine(nodeId, paneId, line); // Line-column positioning
 * focusManager.focusNodeFromArrowNav(nodeId, direction, pixelOffset, paneId); // Arrow navigation
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

export interface FocusState {
  nodeId: string | null;
  cursorPosition: CursorPosition | null;
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

  /**
   * Focus a node with default cursor positioning (beginning of first line, skip syntax)
   */
  focusNode(nodeId: string, paneId: string): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'default', skipSyntax: true };
  },

  /**
   * Focus a node at a specific absolute cursor position
   */
  focusNodeAtPosition(nodeId: string, position: number, paneId: string): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'absolute', position };
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
  },

  /**
   * Focus a node from node type conversion with cursor preservation
   */
  focusNodeFromTypeConversion(nodeId: string, position: number, paneId: string): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'node-type-conversion', position };
  },

  /**
   * Focus an inherited-type node with cursor preservation.
   *
   * Called when Enter key creates a new node that inherits its type from the parent.
   * Unlike focusNodeFromTypeConversion, this sets cursor type to 'inherited-type'
   * which signals TextareaController to use 'inherited' creation source.
   */
  focusNodeFromInheritedType(nodeId: string, cursorPosition: number, paneId: string): void {
    _editingNodeId = nodeId;
    _editingPaneId = paneId;
    _cursorPosition = { type: 'inherited-type', position: cursorPosition };
  },

  /**
   * Clear cursor position after it's been consumed by the action
   */
  clearCursorPosition(): void {
    _cursorPosition = null;
  },

  /**
   * Clear editing state (no node is being edited)
   */
  clearEditing(): void {
    _editingNodeId = null;
    _cursorPosition = null;
  },

  /**
   * Check if a specific node is being edited
   */
  isNodeEditing(nodeId: string): boolean {
    return _editingNodeId === nodeId;
  },

  /**
   * Get current focus state (for debugging/logging)
   */
  getCurrentState(): FocusState {
    return {
      nodeId: _editingNodeId,
      cursorPosition: _cursorPosition
    };
  }
};
