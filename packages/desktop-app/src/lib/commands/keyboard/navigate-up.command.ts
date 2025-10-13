/**
 * NavigateUpCommand - Handles ArrowUp key functionality
 *
 * Extracted from ContentEditableController.handleKeyDown (ArrowUp case)
 * Implements smart navigation between nodes only at boundaries.
 *
 * Behavior:
 * - For single-line nodes: Always navigate on ArrowUp
 * - For multiline nodes with actual multiple lines: Navigate only when at first line
 * - For multiline-capable nodes with single line: Always navigate
 * - Emits navigateArrow event with direction 'up' and pixel offset for cursor positioning
 *
 * This command does NOT handle:
 * - Line-by-line navigation within multiline nodes (browser default behavior)
 * - Modal dropdown interactions (checked before command execution)
 */

import type { KeyboardCommand, KeyboardContext } from '$lib/services/keyboard-command-registry';

export class NavigateUpCommand implements KeyboardCommand {
  id = 'navigate-up';
  description = 'Navigate to previous node on ArrowUp at first line';

  canExecute(context: KeyboardContext): boolean {
    // Only execute for ArrowUp key
    if (context.event.key !== 'ArrowUp') {
      return false;
    }

    // Don't execute if node was just created (layout not settled)
    const controller = context.controller;
    if (controller.justCreated) {
      return false;
    }

    // Don't execute if any dropdown is active - let dropdowns handle navigation
    if (controller.slashCommandDropdownActive || controller.autocompleteDropdownActive) {
      return false;
    }

    // Determine if we should navigate between nodes
    if (context.allowMultiline) {
      // Check if the node actually has multiple lines (DIVs or BRs exist)
      const element = controller.element;
      const lineElements = Array.from(element.children).filter(
        (child: Element) => child.tagName === 'DIV'
      );
      const hasBrTags = element.innerHTML.includes('<br>');
      const hasMultipleLines = lineElements.length > 0 || hasBrTags;

      if (hasMultipleLines) {
        // For nodes with actual multiple lines, let browser handle all navigation
        // Only intercept when we're at the absolute start (no content above cursor)
        const atAbsoluteStart = this.isAtAbsoluteStart(context);
        return atAbsoluteStart;
      } else {
        // Node supports multiline but currently has only single line
        // Allow navigation from anywhere (like single-line nodes)
        return true;
      }
    } else {
      // For single-line nodes, always navigate on arrow up
      return true;
    }
  }

  async execute(context: KeyboardContext): Promise<boolean> {
    // Prevent default browser behavior
    context.event.preventDefault();

    // Calculate pixel offset for cursor positioning
    const pixelOffset = this.getCurrentPixelOffset(context);

    // Emit navigation event
    context.controller.events.navigateArrow({
      nodeId: context.nodeId,
      direction: 'up',
      pixelOffset
    });

    return true;
  }

  /**
   * Check if cursor is at the first line of a multiline node
   * For single-line nodes, always returns true
   */
  private isAtFirstLine(context: KeyboardContext): boolean {
    if (!context.allowMultiline) return true;

    const controller = context.controller;
    return controller.isAtFirstLine ? controller.isAtFirstLine() : true;
  }

  /**
   * Check if cursor is at the absolute start of content (before any text or blank lines)
   * Only returns true when there's literally nothing above the cursor
   */
  private isAtAbsoluteStart(context: KeyboardContext): boolean {
    const controller = context.controller;
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return true;

    const range = selection.getRangeAt(0);

    // Special case: If cursor is at the start of a DIV that's not the first DIV,
    // let browser handle (it should navigate to previous line)
    let currentDiv: Element | null = null;

    if (range.startContainer.nodeType === Node.TEXT_NODE) {
      currentDiv = range.startContainer.parentElement;
    } else if (range.startContainer.nodeType === Node.ELEMENT_NODE) {
      // Cursor might be directly in the DIV (e.g., at start of <br>)
      currentDiv = range.startContainer as Element;
    }

    if (currentDiv && currentDiv.tagName === 'DIV' && range.startOffset === 0) {
      // Cursor is at start of a DIV
      // Check if this DIV is the first child
      const divIndex = Array.from(controller.element.children).indexOf(currentDiv);
      if (divIndex > 0) {
        // Not the first DIV, let browser navigate to previous line
        return false;
      }
    }

    // Create a range from start of element to current cursor position
    const preCaretRange = range.cloneRange();
    preCaretRange.selectNodeContents(controller.element);
    preCaretRange.setEnd(range.startContainer, range.startOffset);

    // Get all content before cursor (including blank lines as newlines)
    const contentBefore = preCaretRange.toString();

    // If there's ANY content (including whitespace/newlines) before cursor, don't navigate
    return contentBefore.length === 0;
  }

  /**
   * Get current cursor position as pixel offset from viewport
   * This allows proper horizontal positioning across nodes with different font sizes
   */
  private getCurrentPixelOffset(context: KeyboardContext): number {
    const controller = context.controller;
    return controller.getCurrentPixelOffset ? controller.getCurrentPixelOffset() : 0;
  }
}
