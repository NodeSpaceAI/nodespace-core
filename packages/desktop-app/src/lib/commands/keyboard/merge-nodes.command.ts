/**
 * MergeNodesCommand - Handles Backspace at the start of a node
 *
 * Extracted from ContentEditableController.handleKeyDown (Backspace case)
 *
 * Behavior:
 * - Backspace at start of node: Merge with previous node
 * - Empty node: Deletes the node entirely
 * - Non-empty node: Combines content with the previous node
 * - Multiline nodes: Only merges when at start of first line
 */

import type { KeyboardCommand, KeyboardContext } from '$lib/services/keyboard-command-registry';

export class MergeNodesCommand implements KeyboardCommand {
  id: string;
  description: string;

  constructor() {
    this.id = 'merge-nodes-up';
    this.description = 'Merge with previous node on Backspace';
  }

  canExecute(context: KeyboardContext): boolean {
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
  }

  async execute(context: KeyboardContext): Promise<boolean> {
    // Prevent default browser behavior
    context.event.preventDefault();

    // Get current content
    const currentContent = this.getCurrentContent(context);

    // Merge with previous node, or delete if empty
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

    return true;
  }

  /**
   * Check if cursor is at the start of the node
   * IMPORTANT: Only returns true when cursor is at start with NO SELECTION
   * If there's a selection, this returns false so browser can handle text deletion naturally
   */
  private isAtStart(context: KeyboardContext): boolean {
    // For textarea-based controllers, use native selection API
    const controller = context.controller;
    const element = controller?.element;

    if (element && 'selectionStart' in element) {
      // Textarea: check if cursor at start AND no selection
      const textarea = element as HTMLTextAreaElement;
      const hasSelection = textarea.selectionStart !== textarea.selectionEnd;

      if (hasSelection) {
        return false; // Let browser delete the selection, don't merge
      }

      return textarea.selectionStart === 0;
    }

    // Fallback: use DOM selection for contenteditable
    const selection = context.selection || window.getSelection();
    if (selection && selection.rangeCount > 0) {
      const range = selection.getRangeAt(0);

      if (!range.collapsed) {
        return false; // Has selection - let browser delete it, don't merge
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
