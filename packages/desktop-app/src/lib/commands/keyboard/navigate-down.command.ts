/**
 * NavigateDownCommand - Handles ArrowDown key functionality
 *
 * Extracted from ContentEditableController.handleKeyDown (ArrowDown case)
 * Implements smart navigation between nodes only at boundaries.
 *
 * Behavior:
 * - For single-line nodes: Always navigate on ArrowDown
 * - For multiline nodes with actual multiple lines: Navigate only when at last line
 * - For multiline-capable nodes with single line: Always navigate
 * - Emits navigateArrow event with direction 'down' and pixel offset for cursor positioning
 *
 * This command does NOT handle:
 * - Line-by-line navigation within multiline nodes (browser default behavior)
 * - Modal dropdown interactions (checked before command execution)
 */

import type { KeyboardCommand, KeyboardContext } from '$lib/services/keyboard-command-registry';

export class NavigateDownCommand implements KeyboardCommand {
  id = 'navigate-down';
  description = 'Navigate to next node on ArrowDown at last line';

  canExecute(context: KeyboardContext): boolean {
    // Only execute for ArrowDown key
    if (context.event.key !== 'ArrowDown') {
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
        // For nodes with actual multiple lines, navigate only when on last line
        return this.isAtLastLine(context);
      } else {
        // Node supports multiline but currently has only single line
        // Allow navigation from anywhere (like single-line nodes)
        return true;
      }
    } else {
      // For single-line nodes, always navigate on arrow down
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
      direction: 'down',
      pixelOffset
    });

    return true;
  }

  /**
   * Check if cursor is at the last line of a multiline node
   * For single-line nodes, always returns true
   */
  private isAtLastLine(context: KeyboardContext): boolean {
    if (!context.allowMultiline) return true;

    const controller = context.controller;
    return controller.isAtLastLine ? controller.isAtLastLine() : true;
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
