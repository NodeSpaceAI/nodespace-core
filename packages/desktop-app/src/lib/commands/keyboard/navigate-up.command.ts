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
      // Check if the node actually has multiple lines (DIVs exist)
      const element = controller.element;
      const lineElements = Array.from(element.children).filter(
        (child: Element) => child.tagName === 'DIV'
      );
      const hasMultipleLines = lineElements.length > 0;

      if (hasMultipleLines) {
        // For nodes with actual multiple lines, navigate only when on first line
        return this.isAtFirstLine(context);
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
   * Get current cursor position as pixel offset from viewport
   * This allows proper horizontal positioning across nodes with different font sizes
   */
  private getCurrentPixelOffset(context: KeyboardContext): number {
    const controller = context.controller;
    return controller.getCurrentPixelOffset ? controller.getCurrentPixelOffset() : 0;
  }
}
