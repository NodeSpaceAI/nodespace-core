/**
 * FormatTextCommand - Handles text formatting shortcuts
 *
 * Extracted from ContentEditableController.handleKeyDown (formatting cases)
 * Implements markdown formatting shortcuts with intelligent toggling.
 *
 * Behavior:
 * - Cmd+B / Ctrl+B: Toggle bold formatting (**)
 * - Cmd+I / Ctrl+I: Toggle italic formatting (*)
 * - Cmd+U / Ctrl+U: Toggle underline formatting (__)
 * - Uses controller's toggleFormatting method for advanced nested formatting support
 * - Prevents default browser formatting behavior
 *
 * The toggleFormatting method handles:
 * - Cross-marker toggle (__ and ** both toggle bold)
 * - Nested formatting (mixed marker scenarios)
 * - Sequential application (rich formatting combinations)
 * - Smart context detection
 */

import type { KeyboardCommand, KeyboardContext } from '$lib/services/keyboard-command-registry';

type FormatType = 'bold' | 'italic' | 'underline';

export class FormatTextCommand implements KeyboardCommand {
  id: string;
  description: string;
  private formatType: FormatType;
  private marker: string;

  constructor(formatType: FormatType) {
    this.formatType = formatType;
    this.id = `format-text-${formatType}`;
    this.description = `Toggle ${formatType} formatting`;

    // Map format types to their markdown markers
    switch (formatType) {
      case 'bold':
        this.marker = '**';
        break;
      case 'italic':
        this.marker = '*';
        break;
      case 'underline':
        this.marker = '__';
        break;
    }
  }

  canExecute(context: KeyboardContext): boolean {
    // Check if the correct modifier key is pressed
    const hasModifier = context.event.metaKey || context.event.ctrlKey;
    if (!hasModifier) {
      return false;
    }

    // Check if this is the correct key for this format type
    const key = context.event.key.toLowerCase();

    switch (this.formatType) {
      case 'bold':
        return key === 'b';
      case 'italic':
        return key === 'i';
      case 'underline':
        return key === 'u';
      default:
        return false;
    }
  }

  async execute(context: KeyboardContext): Promise<boolean> {
    // Prevent default browser formatting behavior
    context.event.preventDefault();

    // Access the controller's toggleFormatting method
    const controller = context.controller;
    if (controller.toggleFormatting && controller.isEditing) {
      controller.toggleFormatting(this.marker);
      return true;
    }

    return false;
  }
}
