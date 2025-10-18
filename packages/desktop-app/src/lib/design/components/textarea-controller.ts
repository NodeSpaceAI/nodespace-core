/**
 * TextareaController
 *
 * Pure TypeScript controller for managing textarea-based markdown editor
 * Replaces ContentEditableController with simpler single-source-of-truth architecture
 *
 * Key improvements over ContentEditableController:
 * - Single state source (textarea.value) instead of dual (DOM + markdown)
 * - Native cursor APIs (selectionStart/End) instead of DOM Range manipulation
 * - No HTML/markdown synchronization bugs
 * - Simpler testing (string assertions instead of DOM queries)
 * - ~950 lines (self-contained) vs ~4250 lines (ContentEditableController + dependencies)
 */

import ContentProcessor from '$lib/services/content-processor';
import type { TriggerContext } from '$lib/services/node-reference-service';
import type { SlashCommandContext } from '$lib/services/slash-command-service';
import { KeyboardCommandRegistry } from '$lib/services/keyboard-command-registry';
import { CreateNodeCommand } from '$lib/commands/keyboard/create-node.command';
import { IndentNodeCommand } from '$lib/commands/keyboard/indent-node.command';
import { OutdentNodeCommand } from '$lib/commands/keyboard/outdent-node.command';
import { MergeNodesCommand } from '$lib/commands/keyboard/merge-nodes.command';
import { NavigateUpCommand } from '$lib/commands/keyboard/navigate-up.command';
import { NavigateDownCommand } from '$lib/commands/keyboard/navigate-down.command';
import { FormatTextCommand } from '$lib/commands/keyboard/format-text.command';
import { CursorPositioningService } from '$lib/services/cursor-positioning-service';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { untrack } from 'svelte';

// Module-level command singletons - created once and reused
const KEYBOARD_COMMANDS = {
  createNode: new CreateNodeCommand(),
  indent: new IndentNodeCommand(),
  outdent: new OutdentNodeCommand(),
  mergeUp: new MergeNodesCommand('up'),
  navigateUp: new NavigateUpCommand(),
  navigateDown: new NavigateDownCommand(),
  formatBold: new FormatTextCommand('bold'),
  formatItalic: new FormatTextCommand('italic')
};

export interface TextareaControllerEvents {
  contentChanged: (content: string) => void;
  headerLevelChanged: (level: number) => void;
  focus: () => void;
  blur: () => void;
  createNewNode: (data: {
    afterNodeId: string;
    nodeType: string;
    currentContent?: string;
    newContent?: string;
    originalContent?: string;
    cursorAtBeginning?: boolean;
    insertAtBeginning?: boolean;
    focusOriginalNode?: boolean;
    newNodeCursorPosition?: number;
  }) => void;
  indentNode: (data: { nodeId: string }) => void;
  outdentNode: (data: { nodeId: string }) => void;
  navigateArrow: (data: { nodeId: string; direction: 'up' | 'down'; pixelOffset: number }) => void;
  combineWithPrevious: (data: { nodeId: string; currentContent: string }) => void;
  deleteNode: (data: { nodeId: string }) => void;
  directSlashCommand: (data: {
    command: string;
    nodeType: string;
    cursorPosition?: number;
  }) => void;
  // @ Trigger System Events
  triggerDetected: (data: {
    triggerContext: TriggerContext;
    cursorPosition: { x: number; y: number };
  }) => void;
  triggerHidden: () => void;
  nodeReferenceSelected: (data: { nodeId: string; nodeTitle: string }) => void;
  // / Slash Command System Events
  slashCommandDetected: (data: {
    commandContext: SlashCommandContext;
    cursorPosition: { x: number; y: number };
  }) => void;
  slashCommandHidden: () => void;
  slashCommandSelected: (data: {
    command: {
      content: string;
      nodeType: string;
      headerLevel?: number;
    };
  }) => void;
  // Node Type Conversion Events
  nodeTypeConversionDetected: (data: {
    nodeId: string;
    newNodeType: string;
    cleanedContent: string;
    cursorPosition: number; // Cursor position at time of conversion
  }) => void;
}

export interface TextareaControllerConfig {
  allowMultiline?: boolean;
}

export class TextareaController {
  // Cursor positioning service
  private cursorService = CursorPositioningService.getInstance();

  // Maximum autocomplete query length (reasonable UX limit, prevents performance issues)
  private static readonly MAX_QUERY_LENGTH = 100;

  private static keyboardCommandsRegistered = false;

  public element: HTMLTextAreaElement;
  private nodeId: string;
  private nodeType: string;
  private config: TextareaControllerConfig;
  public events: TextareaControllerEvents;

  // Single source of truth - no dual representation!
  private isInitialized: boolean = false;
  private currentHeaderLevel: number = 0;

  // Cursor state for arrow navigation
  private lastKnownPixelOffset: number = 0;

  // Singleton measurement element for pixel offset calculations (created lazily)
  private measurementElement: HTMLSpanElement | null = null;

  // Dropdown state
  public slashCommandDropdownActive: boolean = false;
  public autocompleteDropdownActive: boolean = false;

  // Slash command session
  private slashCommandSession: {
    startPosition: number;
    active: boolean;
  } | null = null;

  // @mention session
  private mentionSession: {
    startPosition: number;
    active: boolean;
  } | null = null;

  // Flags
  private skipPatternDetection: boolean = false;
  public recentEnter: boolean = false;
  public justCreated: boolean = false;
  private focusedViaArrowNavigation: boolean = false;

  // Pending cursor position for click-to-edit
  private pendingCursorPosition: number | null = null;

  // Bound event handlers
  private boundHandleFocus = this.handleFocus.bind(this);
  private boundHandleBlur = this.handleBlur.bind(this);
  private boundHandleInput = this.handleInput.bind(this);
  private boundHandleKeyDown = this.handleKeyDown.bind(this);

  constructor(
    element: HTMLTextAreaElement,
    nodeId: string,
    nodeType: string,
    events: TextareaControllerEvents,
    config: TextareaControllerConfig = {}
  ) {
    this.element = element;
    this.nodeId = nodeId;
    this.nodeType = nodeType;
    this.events = events;
    this.config = { allowMultiline: false, ...config }; // Default to single-line

    // Mark DOM element as having a controller attached
    (this.element as unknown as { _textareaController: TextareaController })._textareaController =
      this;

    this.registerKeyboardCommands();
    this.setupEventListeners();
  }

  /**
   * Register keyboard commands with the global registry
   */
  private registerKeyboardCommands(): void {
    if (TextareaController.keyboardCommandsRegistered) {
      return;
    }

    const registry = KeyboardCommandRegistry.getInstance();

    // Core commands
    registry.register({ key: 'Enter' }, KEYBOARD_COMMANDS.createNode);
    registry.register({ key: 'Tab' }, KEYBOARD_COMMANDS.indent);
    registry.register({ key: 'Tab', shift: true }, KEYBOARD_COMMANDS.outdent);
    registry.register({ key: 'Backspace' }, KEYBOARD_COMMANDS.mergeUp);

    // Navigation commands
    registry.register({ key: 'ArrowUp' }, KEYBOARD_COMMANDS.navigateUp);
    registry.register({ key: 'ArrowDown' }, KEYBOARD_COMMANDS.navigateDown);

    // Text formatting commands (cross-platform)
    registry.register({ key: 'b', meta: true }, KEYBOARD_COMMANDS.formatBold);
    registry.register({ key: 'b', ctrl: true }, KEYBOARD_COMMANDS.formatBold);
    registry.register({ key: 'i', meta: true }, KEYBOARD_COMMANDS.formatItalic);
    registry.register({ key: 'i', ctrl: true }, KEYBOARD_COMMANDS.formatItalic);

    TextareaController.keyboardCommandsRegistered = true;
  }

  /**
   * Update controller configuration
   */
  public updateConfig(config: Partial<TextareaControllerConfig>): void {
    this.config = { ...this.config, ...config };
  }

  /**
   * Initialize content
   */
  public initialize(content: string, autoFocus: boolean = false): void {
    if (this.isInitialized) return;

    // Set textarea value - single source of truth!
    this.element.value = content;

    // Initialize header level tracking
    this.currentHeaderLevel = ContentProcessor.getInstance().parseHeaderLevel(content);

    if (autoFocus) {
      this.justCreated = true;
      setTimeout(() => {
        this.justCreated = false;
      }, 50);

      // Use cursor positioning service for consistent behavior
      // Focuses and positions cursor at beginning of first line, skipping syntax
      this.cursorService.setCursorAtBeginningOfLine(this.element, 0, {
        focus: true,
        delay: 0,
        skipSyntax: true
      });
    }

    this.isInitialized = true;
    this.adjustHeight();
  }

  /**
   * Update content from external source (e.g., parent component)
   */
  public updateContent(content: string): void {
    // Skip if content hasn't changed
    if (this.element.value === content) {
      return;
    }

    // Skip during Enter to prevent cursor jumping
    if (this.recentEnter) {
      return;
    }

    this.element.value = content;
    this.adjustHeight();
  }

  /**
   * Force update content (for pattern conversions)
   */
  public forceUpdateContent(content: string): void {
    requestAnimationFrame(() => {
      this.element.value = content;
      this.adjustHeight();
      this.setCursorPosition(content.length); // Position at end
    });
  }

  /**
   * Update the node type (called after successful node type conversion)
   */
  public updateNodeType(newNodeType: string): void {
    this.nodeType = newNodeType;
  }

  /**
   * Mark that this element is about to be focused via arrow navigation
   */
  public prepareForArrowNavigation(): void {
    this.focusedViaArrowNavigation = true;
  }

  /**
   * Focus the textarea with optional cursor positioning
   */
  public focus(): void {
    this.element.focus();

    // Handle pending cursor position from click-to-edit
    if (this.pendingCursorPosition !== null) {
      this.setCursorPosition(this.pendingCursorPosition);
      this.pendingCursorPosition = null;
    }
  }

  /**
   * Position cursor at the beginning of a line using cursor positioning service
   * Public API for external consumers (e.g., base-node.svelte autoFocus handling)
   *
   * @param lineNumber 0-based line number (defaults to 0)
   * @param skipSyntax Whether to skip syntax markers (defaults to true)
   */
  public positionCursorAtLineBeginning(lineNumber: number = 0, skipSyntax: boolean = true): void {
    this.cursorService.setCursorAtBeginningOfLine(this.element, lineNumber, {
      focus: false, // Caller should handle focus separately
      delay: 0,
      skipSyntax
    });
  }

  /**
   * Get current content (markdown)
   */
  public getMarkdownContent(): string {
    return this.element.value;
  }

  /**
   * Destroy controller and cleanup
   */
  public destroy(): void {
    this.removeEventListeners();
    // Clean up the controller reference from DOM element
    delete (this.element as unknown as { _textareaController?: TextareaController })
      ._textareaController;
    // Clean up singleton measurement element
    if (this.measurementElement && this.measurementElement.parentNode) {
      this.measurementElement.parentNode.removeChild(this.measurementElement);
      this.measurementElement = null;
    }
  }

  /**
   * Setup event listeners
   */
  private setupEventListeners(): void {
    this.element.addEventListener('focus', this.boundHandleFocus);
    this.element.addEventListener('blur', this.boundHandleBlur);
    this.element.addEventListener('input', this.boundHandleInput);
    this.element.addEventListener('keydown', this.boundHandleKeyDown);
  }

  /**
   * Remove event listeners
   */
  private removeEventListeners(): void {
    this.element.removeEventListener('focus', this.boundHandleFocus);
    this.element.removeEventListener('blur', this.boundHandleBlur);
    this.element.removeEventListener('input', this.boundHandleInput);
    this.element.removeEventListener('keydown', this.boundHandleKeyDown);
  }

  /**
   * Auto-resize textarea to fit content
   */
  public adjustHeight(): void {
    // Reset height to auto to get correct scrollHeight
    this.element.style.height = 'auto';
    this.element.style.height = `${this.element.scrollHeight}px`;
  }

  /**
   * Set cursor position using native textarea API
   * Made public to allow external cursor positioning (e.g., for newly created nodes)
   */
  public setCursorPosition(position: number): void {
    this.element.selectionStart = position;
    this.element.selectionEnd = position;
  }

  /**
   * Get cursor position using native textarea API
   */
  private getCursorPosition(): number {
    return this.element.selectionStart;
  }

  /**
   * Check if cursor is at start
   */
  private isAtStart(): boolean {
    return this.element.selectionStart === 0;
  }

  /**
   * Check if cursor is at end
   */
  private isAtEnd(): boolean {
    return this.element.selectionStart === this.element.value.length;
  }

  /**
   * Check if cursor is at first line
   */
  public isAtFirstLine(): boolean {
    const position = this.element.selectionStart;
    const textBefore = this.element.value.substring(0, position);
    return !textBefore.includes('\n');
  }

  /**
   * Check if cursor is at last line
   * Returns true if cursor is on the last line (including empty line after trailing newline)
   */
  public isAtLastLine(): boolean {
    const position = this.element.selectionStart;
    const content = this.element.value;

    // Count newlines before cursor
    const textBefore = content.substring(0, position);
    const newlinesBeforeCursor = (textBefore.match(/\n/g) || []).length;

    // Count total newlines
    const totalNewlines = (content.match(/\n/g) || []).length;

    // If no newlines at all, we're on the only line
    if (totalNewlines === 0) {
      return true;
    }

    // Check if there's a trailing newline
    const hasTrailingNewline = content.endsWith('\n');

    if (hasTrailingNewline) {
      // If content ends with newline, there's an empty line at the end
      // We're on last line only if we've passed ALL newlines (on the empty line after trailing \n)
      return newlinesBeforeCursor >= totalNewlines;
    } else {
      // No trailing newline - we're on last line if we've passed all newlines
      return newlinesBeforeCursor >= totalNewlines;
    }
  }

  /**
   * Get current column offset for arrow navigation
   */
  public getCurrentColumn(): number {
    const position = this.element.selectionStart;
    const textBefore = this.element.value.substring(0, position);
    const lastNewline = textBefore.lastIndexOf('\n');
    return lastNewline === -1 ? position : position - lastNewline - 1;
  }

  /**
   * Get current pixel offset for arrow navigation (pixel-accurate measurement)
   * Uses singleton hidden span element to measure actual rendered text width
   */
  public getCurrentPixelOffset(): number {
    const position = this.element.selectionStart;
    const textBefore = this.element.value.substring(0, position);
    const lastNewline = textBefore.lastIndexOf('\n');
    const currentLineStart = lastNewline === -1 ? 0 : lastNewline + 1;
    const textBeforeCursor = this.element.value.substring(currentLineStart, position);

    // Get textarea bounding rect for absolute positioning
    const rect = this.element.getBoundingClientRect();

    // Lazy initialization: create singleton measurement element on first use
    if (!this.measurementElement) {
      this.measurementElement = document.createElement('span');
      const computedStyle = window.getComputedStyle(this.element);
      this.measurementElement.style.cssText = `
        position: absolute;
        visibility: hidden;
        white-space: pre;
        font-family: ${computedStyle.fontFamily};
        font-size: ${computedStyle.fontSize};
        font-weight: ${computedStyle.fontWeight};
        letter-spacing: ${computedStyle.letterSpacing};
        word-spacing: ${computedStyle.wordSpacing};
        line-height: ${computedStyle.lineHeight};
      `;
      document.body.appendChild(this.measurementElement);
    }

    // Reuse measurement element: update text content and measure
    this.measurementElement.textContent = textBeforeCursor;
    const textWidth = this.measurementElement.getBoundingClientRect().width;

    // Return absolute pixel position including node indentation
    this.lastKnownPixelOffset = rect.left + textWidth + window.scrollX;
    return this.lastKnownPixelOffset;
  }

  /**
   * Position cursor when entering from arrow navigation
   * @param direction - 'up' (entering from bottom) or 'down' (entering from top)
   * @param pixelOffset - Absolute horizontal pixel offset to maintain (viewport-relative)
   */
  public enterFromArrowNavigation(direction: 'up' | 'down', pixelOffset: number): void {
    const content = this.element.value;
    const lines = content.split('\n');

    // Choose target line based on direction
    const lineIndex = direction === 'up' ? lines.length - 1 : 0;
    const targetLine = lines[lineIndex];

    // Convert absolute pixel offset to relative offset within textarea
    const rect = this.element.getBoundingClientRect();
    const textareaLeftEdge = rect.left + window.scrollX;
    const relativePixelOffset = pixelOffset - textareaLeftEdge;

    // Convert relative pixel offset to column (approximate)
    // Fallback: ~8px per character for monospace estimation when measurement unavailable
    const approximateColumn = Math.max(0, Math.round(relativePixelOffset / 8));

    // Clamp column to line length
    const column = Math.min(approximateColumn, targetLine.length);

    // Calculate absolute position in textarea
    let position = 0;
    for (let i = 0; i < lineIndex; i++) {
      position += lines[i].length + 1; // +1 for newline character
    }
    position += column;

    // Set cursor position
    this.setCursorPosition(position);
  }

  // ============================================================================
  // Event Handlers
  // ============================================================================

  private handleFocus(): void {
    this.events.focus();
  }

  private handleBlur(): void {
    // Hide autocomplete and slash commands on blur
    if (this.mentionSession?.active) {
      this.events.triggerHidden();
      this.mentionSession = null;
    }

    if (this.slashCommandSession?.active) {
      this.events.slashCommandHidden();
      this.slashCommandSession = null;
    }

    this.events.blur();
  }

  private handleInput(): void {
    const content = this.element.value;

    // Emit content changed event
    this.events.contentChanged(content);

    // Auto-resize
    this.adjustHeight();

    // Detect patterns if not skipped
    if (!this.skipPatternDetection) {
      this.detectPatterns();
    }

    this.skipPatternDetection = false;
  }

  private async handleKeyDown(event: KeyboardEvent): Promise<void> {
    const registry = KeyboardCommandRegistry.getInstance();

    // Handle slash commands first
    if (this.slashCommandDropdownActive) {
      // Let dropdown handle Enter/Escape/Arrow keys
      if (['Enter', 'Escape', 'ArrowUp', 'ArrowDown'].includes(event.key)) {
        return;
      }
    }

    // Handle autocomplete
    if (this.autocompleteDropdownActive) {
      // Let dropdown handle Enter/Escape/Arrow keys
      if (['Enter', 'Escape', 'ArrowUp', 'ArrowDown'].includes(event.key)) {
        return;
      }
    }

    // Try to execute keyboard command
    const context = this.buildKeyboardContext(event);
    const handled = await registry.execute(event, this, context);

    if (handled) {
      return; // Command handled the event
    }

    // Detect Shift+Enter for multiline (only if multiline is enabled)
    if (event.key === 'Enter' && event.shiftKey) {
      if (this.config.allowMultiline) {
        // Allow default newline insertion
        return;
      } else {
        // Prevent Shift+Enter in single-line mode
        event.preventDefault();
        return;
      }
    }
  }

  /**
   * Build keyboard context for command execution
   */
  private buildKeyboardContext(event: KeyboardEvent): Record<string, unknown> {
    return {
      event,
      controller: this,
      nodeId: this.nodeId,
      nodeType: this.nodeType,
      content: this.element.value,
      cursorPosition: this.getCursorPosition(),
      selection: window.getSelection(),
      allowMultiline: this.config.allowMultiline,
      metadata: {}
    };
  }

  /**
   * Detect patterns for @mentions, slash commands, and node type conversions
   */
  private detectPatterns(): void {
    const content = this.element.value;
    const position = this.getCursorPosition();

    // Detect @mention trigger
    this.detectMentionTrigger(content, position);

    // Detect slash command trigger
    this.detectSlashCommandTrigger(content, position);

    // Detect node type conversion (e.g., typing `- [ ]` converts to task node)
    this.detectNodeTypeConversion(content);
  }

  /**
   * Detect @mention trigger
   */
  private detectMentionTrigger(content: string, position: number): void {
    const textBefore = content.substring(0, position);
    const atIndex = textBefore.lastIndexOf('@');

    if (atIndex === -1) {
      if (this.mentionSession?.active) {
        this.events.triggerHidden();
        this.mentionSession = null;
      }
      return;
    }

    const query = textBefore.substring(atIndex + 1);

    // Check if @ is at word boundary
    const charBefore = atIndex > 0 ? textBefore[atIndex - 1] : ' ';
    const isAtWordBoundary = /\s/.test(charBefore) || atIndex === 0;

    if (!isAtWordBoundary || query.length > TextareaController.MAX_QUERY_LENGTH) {
      if (this.mentionSession?.active) {
        this.events.triggerHidden();
        this.mentionSession = null;
      }
      return;
    }

    // Active @mention session
    if (!this.mentionSession || !this.mentionSession.active) {
      this.mentionSession = { startPosition: atIndex, active: true };
    }

    // Get cursor position for popup
    const cursorCoords = this.getCursorCoordinates();

    this.events.triggerDetected({
      triggerContext: {
        trigger: '@',
        query: query,
        startPosition: atIndex,
        endPosition: position,
        element: this.element,
        isValid: true,
        metadata: {}
      },
      cursorPosition: cursorCoords
    });
  }

  /**
   * Detect slash command trigger
   */
  private detectSlashCommandTrigger(content: string, position: number): void {
    const textBefore = content.substring(0, position);
    const slashIndex = textBefore.lastIndexOf('/');

    if (slashIndex === -1) {
      if (this.slashCommandSession?.active) {
        this.events.slashCommandHidden();
        this.slashCommandSession = null;
      }
      return;
    }

    const query = textBefore.substring(slashIndex + 1);

    // Check if / is at start of line or after whitespace
    const charBefore = slashIndex > 0 ? textBefore[slashIndex - 1] : '\n';
    const isAtLineStart = charBefore === '\n' || slashIndex === 0;
    const isAfterWhitespace = /\s/.test(charBefore);

    if ((!isAtLineStart && !isAfterWhitespace) || query.includes(' ') || query.length > 50) {
      if (this.slashCommandSession?.active) {
        this.events.slashCommandHidden();
        this.slashCommandSession = null;
      }
      return;
    }

    // Active slash command session
    if (!this.slashCommandSession || !this.slashCommandSession.active) {
      this.slashCommandSession = { startPosition: slashIndex, active: true };
    }

    // Get cursor position for popup
    const cursorCoords = this.getCursorCoordinates();

    this.events.slashCommandDetected({
      commandContext: {
        trigger: '/',
        query: query,
        startPosition: slashIndex,
        endPosition: position,
        element: this.element,
        isValid: true,
        metadata: {}
      },
      cursorPosition: cursorCoords
    });
  }

  /**
   * Detect node type conversion patterns using plugin registry
   * Replaces hardcoded pattern detection with extensible plugin-based system
   *
   * Handles bidirectional conversion:
   * - Pattern match → Convert to specialized node type (e.g., text → header)
   * - Pattern removed → Convert back to text node (e.g., header → text)
   */
  private detectNodeTypeConversion(content: string): void {
    // Use plugin registry to detect patterns
    const detection = pluginRegistry.detectPatternInContent(content);

    if (detection) {
      // Pattern detected - convert to specialized node type
      const { config, match, metadata } = detection;

      // CRITICAL: Capture cursor position BEFORE event emission
      // Use desiredCursorPosition from config if specified, otherwise preserve current position
      const cursorPosition =
        config.desiredCursorPosition !== undefined
          ? config.desiredCursorPosition
          : this.getCursorPosition();

      // Calculate cleaned content based on plugin config
      const cleanedContent = config.cleanContent
        ? content.replace(match[0], '') // Remove pattern from content
        : content; // Keep pattern in content (e.g., headers keep "# ")

      // Use untrack to prevent Svelte 5 reactive tracking during event emission
      // This prevents state_unsafe_mutation errors when parent components update state
      // in response to these events during their own reactive updates
      untrack(() => {
        // Emit header level changed event if metadata contains headerLevel
        if (metadata.headerLevel !== undefined) {
          this.events.headerLevelChanged(metadata.headerLevel as number);
        }

        // Emit node type conversion event with cursor position
        this.events.nodeTypeConversionDetected({
          nodeId: this.nodeId,
          newNodeType: config.targetNodeType,
          cleanedContent: cleanedContent,
          cursorPosition: cursorPosition
        });

        // CRITICAL: Update internal nodeType so reverse conversion can detect it
        this.nodeType = config.targetNodeType;
      });
    } else if (this.nodeType !== 'text') {
      // No pattern detected AND current node is NOT text
      // This means user removed the pattern (e.g., backspaced "## " to "##")
      // CRITICAL: Capture cursor position BEFORE event emission
      const cursorPosition = this.getCursorPosition();

      // Convert back to plain text node
      untrack(() => {
        this.events.nodeTypeConversionDetected({
          nodeId: this.nodeId,
          newNodeType: 'text',
          cleanedContent: content,
          cursorPosition: cursorPosition
        });

        // CRITICAL: Update internal nodeType so future conversions work correctly
        this.nodeType = 'text';
      });
    }
  }

  /**
   * Get cursor coordinates for popup positioning
   */
  private getCursorCoordinates(): { x: number; y: number } {
    // Get textarea bounding rect
    const rect = this.element.getBoundingClientRect();

    // Approximate cursor position based on line number
    const position = this.getCursorPosition();
    const textBefore = this.element.value.substring(0, position);
    const lines = textBefore.split('\n');
    const lineNumber = lines.length - 1;
    // Default line height in pixels (matches CSS --line-height: 1.5 * 16px base font)
    const lineHeight = 24;

    return {
      x: rect.left,
      y: rect.top + lineNumber * lineHeight + lineHeight
    };
  }

  // ============================================================================
  // Dropdown State Management
  // ============================================================================

  public setSlashCommandDropdownActive(active: boolean): void {
    this.slashCommandDropdownActive = active;
  }

  public setAutocompleteDropdownActive(active: boolean): void {
    this.autocompleteDropdownActive = active;
  }

  // ============================================================================
  // Content Manipulation Methods (for slash commands, @mentions, formatting)
  // ============================================================================

  /**
   * Insert node reference at current @mention position
   */
  public insertNodeReference(nodeId: string, nodeTitle: string): void {
    if (!this.mentionSession) return;

    const content = this.element.value;
    const before = content.substring(0, this.mentionSession.startPosition);
    const after = content.substring(this.getCursorPosition());

    // Insert node reference in markdown format
    const reference = `[@${nodeTitle}](node://${nodeId})`;
    const newContent = before + reference + after;

    this.element.value = newContent;
    this.setCursorPosition(before.length + reference.length);

    this.mentionSession = null;
    this.events.contentChanged(newContent);
    this.adjustHeight();
  }

  /**
   * Insert slash command content
   */
  public insertSlashCommand(
    content: string,
    skipDetection: boolean,
    _targetNodeType?: string
  ): number {
    if (!this.slashCommandSession) return 0;

    this.skipPatternDetection = skipDetection;

    const currentContent = this.element.value;
    const before = currentContent.substring(0, this.slashCommandSession.startPosition);
    const after = currentContent.substring(this.getCursorPosition());

    const newContent = before + content + after;
    const cursorPosition = before.length + content.length;

    this.element.value = newContent;
    this.setCursorPosition(cursorPosition);

    this.slashCommandSession = null;
    this.events.contentChanged(newContent);
    this.adjustHeight();

    return cursorPosition;
  }

  /**
   * Toggle markdown formatting (bold, italic, underline)
   */
  public toggleFormatting(marker: string): void {
    const start = this.element.selectionStart;
    const end = this.element.selectionEnd;
    const content = this.element.value;

    // If no selection, do nothing
    if (start === end) {
      return;
    }

    const selectedText = content.substring(start, end);
    const before = content.substring(0, start);
    const after = content.substring(end);

    // Check if already formatted
    const markerLength = marker.length;
    const isFormatted = before.endsWith(marker) && after.startsWith(marker);

    let newContent: string;
    let newCursorStart: number;
    let newCursorEnd: number;

    if (isFormatted) {
      // Remove formatting
      newContent =
        before.substring(0, before.length - markerLength) +
        selectedText +
        after.substring(markerLength);
      newCursorStart = start - markerLength;
      newCursorEnd = end - markerLength;
    } else {
      // Add formatting
      newContent = before + marker + selectedText + marker + after;
      newCursorStart = start + markerLength;
      newCursorEnd = end + markerLength;
    }

    this.element.value = newContent;
    this.element.selectionStart = newCursorStart;
    this.element.selectionEnd = newCursorEnd;

    this.events.contentChanged(newContent);
    this.adjustHeight();
  }

  /**
   * Check if currently processing input
   */
  public isProcessingInput(): boolean {
    return this.slashCommandDropdownActive || this.autocompleteDropdownActive;
  }

  // ============================================================================
  // Keyboard Command Compatibility Methods
  // ============================================================================

  /**
   * Get cursor position in markdown (for keyboard commands)
   * In textarea, this is just selectionStart - no HTML/markdown conversion needed!
   */
  public getCursorPositionInMarkdown(): number {
    return this.element.selectionStart;
  }

  /**
   * Convert HTML to text with newlines (for keyboard commands)
   * In textarea, content is already plain text - just return it!
   */
  public convertHtmlToTextWithNewlines(_html: string): string {
    // In textarea mode, we don't have HTML - content is already plain text
    // This method is only called by keyboard commands for multiline support
    return this.element.value;
  }

  /**
   * Set live formatted content (for keyboard commands)
   * In textarea, we just update the value directly
   */
  public setLiveFormattedContent(content: string): void {
    this.element.value = content;
    this.adjustHeight();
  }
}
