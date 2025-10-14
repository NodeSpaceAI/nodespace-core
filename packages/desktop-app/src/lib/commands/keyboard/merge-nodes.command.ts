/**
 * MergeNodesCommand - Handles Backspace/Delete functionality
 *
 * Extracted from ContentEditableController.handleKeyDown (Backspace case)
 * Unified command for merging nodes with directional parameter.
 *
 * Behavior:
 * - Backspace at start of node (direction='up'): Merge with previous node ✅ IMPLEMENTED
 * - Delete at end of node (direction='down'): Merge with next node ⏳ DEFERRED (see TODO below)
 * - Empty node: Deletes the node entirely
 * - Non-empty node: Combines content with adjacent node
 * - Multiline nodes: Only merges when at start of first line
 *
 * Usage:
 * - `new MergeNodesCommand('up')` for Backspace (merge with above)
 * - `new MergeNodesCommand('down')` for Delete (merge with below) - architecture ready, implementation deferred
 *
 * TODO: Implement Delete key functionality (direction='down')
 * Requires:
 * - Add `combineWithNext` event to ContentEditableController.events
 * - Implement isAtEnd() and isAtEndOfLastLine() helper methods
 * - Add node service support for merging with next sibling
 * - Add comprehensive tests for Delete key behavior
 * - Update documentation when complete
 */

import type { KeyboardCommand, KeyboardContext } from '$lib/services/keyboard-command-registry';

export type MergeDirection = 'up' | 'down';

export class MergeNodesCommand implements KeyboardCommand {
  id: string;
  description: string;
  private direction: MergeDirection;

  constructor(direction: MergeDirection = 'up') {
    this.direction = direction;
    this.id = direction === 'up' ? 'merge-nodes-up' : 'merge-nodes-down';
    this.description =
      direction === 'up'
        ? 'Merge with previous node on Backspace'
        : 'Merge with next node on Delete';
  }

  canExecute(context: KeyboardContext): boolean {
    if (this.direction === 'up') {
      // Backspace: only execute at start of node
      if (context.event.key !== 'Backspace') {
        return false;
      }

      // Check if we're at the start
      if (!this.isAtStart(context)) {
        return false;
      }

      // For multiline nodes, ensure we're at start of first line
      if (context.allowMultiline) {
        return this.isAtStartOfFirstLine(context);
      }

      return true;
    } else {
      // Delete: only execute at end of node (future implementation)
      // For now, not implemented
      return false;
    }
  }

  async execute(context: KeyboardContext): Promise<boolean> {
    // Prevent default browser behavior
    context.event.preventDefault();

    // Get current content
    const currentContent = this.getCurrentContent(context);

    if (this.direction === 'up') {
      // Backspace: merge with previous node or delete if empty
      if (currentContent.trim() === '') {
        // Empty node: delete it entirely
        context.controller.events.deleteNode({
          nodeId: context.nodeId
        });
      } else {
        // Non-empty node: combine with previous
        context.controller.events.combineWithPrevious({
          nodeId: context.nodeId,
          currentContent
        });
      }
    } else {
      // Delete: merge with next node (deferred implementation - see TODO in file header)
      // When implemented, would emit combineWithNext event (not yet available)
      // Architecture is ready, but requires event system and node service changes
      return false;
    }

    return true;
  }

  /**
   * Check if cursor is at the start of the node
   * Uses the same logic as ContentEditableController.isAtStart()
   */
  private isAtStart(context: KeyboardContext): boolean {
    // Check selection if available
    const selection = context.selection || window.getSelection();
    let range: Range | null = null;
    if (selection && selection.rangeCount > 0) {
      range = selection.getRangeAt(0);
      if (!range.collapsed) {
        return false; // Not at start if there's a selection
      }
    }

    // Use cursor position from context
    return context.cursorPosition === 0;
  }

  /**
   * Check if cursor is at the start of the first line (for multiline nodes)
   * Uses the same logic as ContentEditableController.isAtStartOfFirstLine()
   */
  private isAtStartOfFirstLine(context: KeyboardContext): boolean {
    if (!context.allowMultiline) {
      return true;
    }

    // For multiline nodes, we need to check both:
    // 1. Are we at the first line?
    // 2. Are we at position 0?

    // First check if we're on the first line
    if (!this.isAtFirstLine(context)) {
      return false;
    }

    // Then check if we're at position 0
    return context.cursorPosition === 0;
  }

  /**
   * Check if cursor is at the first line (for multiline nodes)
   */
  private isAtFirstLine(context: KeyboardContext): boolean {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return true;
    }

    const range = selection.getRangeAt(0);
    const controller = context.controller;
    const element = controller?.element;

    if (!element) {
      return true;
    }

    // Check if we're in a DIV structure (multiline with DIVs)
    let currentNode: Node | null = range.startContainer;

    // Walk up to find the containing DIV
    while (currentNode && currentNode !== element) {
      if (
        currentNode.nodeType === Node.ELEMENT_NODE &&
        (currentNode as Element).tagName === 'DIV' &&
        currentNode.parentNode === element
      ) {
        // Found the containing DIV - check if it's the first one
        return currentNode === element.firstElementChild;
      }
      currentNode = currentNode.parentNode;
    }

    // If no div structure found, assume single line (first line)
    return true;
  }

  /**
   * Get current content from context
   */
  private getCurrentContent(context: KeyboardContext): string {
    const controller = context.controller;
    const element = controller?.element;

    if (element) {
      return element.textContent || '';
    }

    return context.content;
  }

  /**
   * Get cursor position from context
   * Uses controller's getCurrentColumn method for accuracy
   */
  private getCursorPosition(context: KeyboardContext): number {
    const controller = context.controller;
    const getCurrentColumn = controller?.getCurrentColumn;

    if (getCurrentColumn) {
      return getCurrentColumn.call(controller);
    }

    return context.cursorPosition;
  }
}
