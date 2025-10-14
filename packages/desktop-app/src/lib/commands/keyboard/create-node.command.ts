/**
 * CreateNodeCommand - Handles Enter key functionality
 *
 * Extracted from ContentEditableController.handleKeyDown (Enter case)
 * Implements smart text splitting with markdown-aware content handling.
 *
 * Behavior:
 * - Regular Enter: Creates new node with smart text splitting
 * - Shift+Enter (multiline mode): Inserts line break (handled by controller)
 * - At beginning/syntax area: Creates empty node above, preserves original
 * - Middle/end: Splits content intelligently, preserving markdown formatting
 *
 * This command does NOT handle:
 * - Shift+Enter (multiline insertion) - controller handles this
 * - Slash command dropdown interactions - checked before command execution
 */

import type { KeyboardCommand, KeyboardContext } from '$lib/services/keyboard-command-registry';
import { splitMarkdownContent } from '$lib/utils/markdown-splitter';

// Constants for timing coordination
const ENTER_FLAG_RESET_DELAY_MS = 100; // Timeout for recentEnter flag to prevent cursor restoration race conditions

// DOM selectors for dropdown detection
const SLASH_COMMAND_DROPDOWN_SELECTOR = '[role="listbox"][aria-label="Slash command palette"]';
const AUTOCOMPLETE_DROPDOWN_SELECTOR = '[role="listbox"][aria-label="Node reference autocomplete"]';

export class CreateNodeCommand implements KeyboardCommand {
  id = 'create-node';
  description = 'Create new node on Enter key';

  canExecute(context: KeyboardContext): boolean {
    // Only execute for regular Enter (not Shift+Enter)
    if (context.event.key !== 'Enter' || context.event.shiftKey) {
      return false;
    }

    // Don't execute if slash command dropdown is active
    // Check both state and actual DOM presence
    const slashDropdownExists = document.querySelector(SLASH_COMMAND_DROPDOWN_SELECTOR);
    const autocompleteDropdownExists = document.querySelector(AUTOCOMPLETE_DROPDOWN_SELECTOR);

    // Let dropdowns handle Enter if they're active
    if (slashDropdownExists || autocompleteDropdownExists) {
      return false;
    }

    return true;
  }

  async execute(context: KeyboardContext): Promise<boolean> {
    // Prevent default browser behavior
    context.event.preventDefault();

    // Get current content
    // For multiline nodes, preserve line breaks
    const currentContent = this.getCurrentContent(context);
    const cursorPosition = this.getCursorPosition(context);

    // Set flag to prevent cursor restoration during node creation
    // This is a bit of a hack but necessary for timing coordination
    this.setRecentEnterFlag(context);

    // Check if we should create new node above (at beginning/syntax area)
    const shouldCreateAbove = this.shouldCreateNodeAbove(currentContent, cursorPosition);

    if (shouldCreateAbove) {
      // Create new empty node above, preserve original node unchanged
      context.controller.events.createNewNode({
        afterNodeId: context.nodeId,
        nodeType: context.nodeType,
        currentContent: currentContent, // Original node keeps its content unchanged
        newContent: '', // New node above starts empty
        originalContent: currentContent,
        cursorAtBeginning: true, // Focus at beginning of bottom node (original node)
        insertAtBeginning: true, // Insert BEFORE the current node
        focusOriginalNode: true // Focus the original node (bottom) instead of new node (top)
      });
    } else {
      // Normal splitting behavior for middle/end positions
      const splitResult = splitMarkdownContent(currentContent, cursorPosition);

      // Update current element immediately to show completed syntax
      this.updateElementContent(context, splitResult.beforeContent);

      context.controller.events.createNewNode({
        afterNodeId: context.nodeId,
        nodeType: context.nodeType, // Preserve original node's type
        currentContent: splitResult.beforeContent,
        newContent: splitResult.afterContent,
        originalContent: currentContent, // Pass original content before split for inheritance
        cursorAtBeginning: false,
        insertAtBeginning: false // Normal splitting creates nodes after, not above
      });
    }

    return true;
  }

  /**
   * Get current content from context
   * For multiline nodes, preserves line breaks
   */
  private getCurrentContent(context: KeyboardContext): string {
    if (context.allowMultiline) {
      // Access the controller's convertHtmlToTextWithNewlines method
      // We need to access the element's innerHTML
      const element = context.controller.element;
      if (element) {
        return context.controller.convertHtmlToTextWithNewlines(element.innerHTML);
      }
    }
    return context.content;
  }

  /**
   * Get cursor position from context
   * CRITICAL: Must use getCursorPositionInMarkdown for markdown-aware splitting
   *
   * getCurrentColumn() returns position in plain text (e.g., "bold" in visual)
   * getCursorPositionInMarkdown() returns position in raw markdown (e.g., "**bold**")
   * splitMarkdownContent() requires raw markdown positions to work correctly
   */
  private getCursorPosition(context: KeyboardContext): number {
    // Use markdown-aware cursor position for proper splitting
    const getCursorInMarkdown = context.controller.getCursorPositionInMarkdown;
    if (getCursorInMarkdown) {
      return getCursorInMarkdown.call(context.controller);
    }
    return context.cursorPosition;
  }

  /**
   * Set recentEnter flag to prevent cursor restoration
   * This is necessary for timing coordination during node creation
   */
  private setRecentEnterFlag(context: KeyboardContext): void {
    const controller = context.controller;
    if (controller) {
      controller.recentEnter = true;
      setTimeout(() => {
        controller.recentEnter = false;
      }, ENTER_FLAG_RESET_DELAY_MS);
    }
  }

  /**
   * Update element content immediately
   * Reflects the split in the DOM before event processing
   *
   * CRITICAL: Must call setLiveFormattedContent to preserve inline markdown formatting
   * Without it, textContent strips formatting and displays raw markdown (e.g., **bo** as plain text)
   */
  private updateElementContent(context: KeyboardContext, newContent: string): void {
    const controller = context.controller;
    if (controller && controller.element) {
      controller.originalContent = newContent;
      controller.element.textContent = newContent; // Clear existing HTML
      controller.setLiveFormattedContent(newContent); // Re-apply markdown formatting
    }
  }

  /**
   * Determine if we should create a new node above instead of splitting
   * This preserves the original node's identity and relationships
   *
   * Returns true when cursor is:
   * - At the very beginning (position 0)
   * - Within header syntax (e.g., |#, #|, # |)
   * - Within inline format opening syntax at beginning (e.g., |**, *|*)
   */
  private shouldCreateNodeAbove(content: string, position: number): boolean {
    // Always create above when cursor is at the very beginning
    if (position <= 0) {
      return true;
    }

    // For headers, create above when cursor is within or at the end of the syntax area
    const headerMatch = content.match(/^(#{1,6}\s+)/);
    if (headerMatch) {
      const headerPrefixLength = headerMatch[1].length;
      // Create above if cursor is within or right after header syntax
      if (position <= headerPrefixLength) {
        return true;
      }
    }

    // For inline formatting, create above when cursor is within opening syntax at the beginning
    const inlineFormats = [
      { pattern: /^\*\*/, length: 2 }, // Bold **
      { pattern: /^__/, length: 2 }, // Bold __
      { pattern: /^\*(?!\*)/, length: 1 }, // Italic * (not part of **)
      { pattern: /^_(?!_)/, length: 1 }, // Italic _ (not part of __)
      { pattern: /^~~/, length: 2 }, // Strikethrough ~~
      { pattern: /^`/, length: 1 } // Code `
    ];

    for (const format of inlineFormats) {
      if (format.pattern.test(content)) {
        // Create above if cursor is within the opening syntax
        if (position <= format.length) {
          return true;
        }
      }
    }

    // For all other cases, use normal splitting
    return false;
  }
}
