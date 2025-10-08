/**
 * IndentNodeCommand - Handles Tab key functionality
 *
 * Extracted from ContentEditableController.handleKeyDown (Tab case)
 * Indents the current node one level deeper in the hierarchy.
 *
 * Behavior:
 * - Tab key (without Shift): Indent the current node
 * - Validates indentation rules before emitting event
 * - Event emission delegates to node service for hierarchy validation
 *
 * Indentation Rules (enforced by node service):
 * - Cannot indent if no previous sibling exists
 * - Cannot indent beyond one level deeper than parent
 * - Maintains tree integrity
 */

import type { KeyboardCommand, KeyboardContext } from '$lib/services/keyboard-command-registry';

export class IndentNodeCommand implements KeyboardCommand {
  id = 'indent-node';
  description = 'Indent node on Tab key';

  canExecute(context: KeyboardContext): boolean {
    // Only execute for Tab without Shift modifier
    return context.event.key === 'Tab' && !context.event.shiftKey;
  }

  async execute(context: KeyboardContext): Promise<boolean> {
    // Prevent default browser tab navigation
    context.event.preventDefault();

    // Emit indentNode event
    // Node service will validate indentation rules and update hierarchy
    context.controller.events.indentNode({
      nodeId: context.nodeId
    });

    return true;
  }
}
