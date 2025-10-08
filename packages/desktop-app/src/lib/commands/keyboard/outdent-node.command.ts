/**
 * OutdentNodeCommand - Handles Shift+Tab functionality
 *
 * Extracted from ContentEditableController.handleKeyDown (Shift+Tab case)
 * Outdents (unindents) the current node one level higher in the hierarchy.
 *
 * Behavior:
 * - Shift+Tab: Outdent the current node
 * - Event emission delegates to node service for hierarchy validation
 *
 * Outdentation Rules (enforced by node service):
 * - Cannot outdent root-level nodes
 * - Maintains tree integrity
 * - Updates parent-child relationships
 */

import type { KeyboardCommand, KeyboardContext } from '$lib/services/keyboard-command-registry';

export class OutdentNodeCommand implements KeyboardCommand {
  id = 'outdent-node';
  description = 'Outdent node on Shift+Tab';

  canExecute(context: KeyboardContext): boolean {
    // Only execute for Tab WITH Shift modifier
    return context.event.key === 'Tab' && context.event.shiftKey;
  }

  async execute(context: KeyboardContext): Promise<boolean> {
    // Prevent default browser behavior
    context.event.preventDefault();

    // Emit outdentNode event
    // Node service will validate outdentation rules and update hierarchy
    context.controller.events.outdentNode({
      nodeId: context.nodeId
    });

    return true;
  }
}
