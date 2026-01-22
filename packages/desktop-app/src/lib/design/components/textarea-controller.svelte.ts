/**
 * TextareaController (Svelte 5 Reactive Version)
 *
 * Architecture: Hybrid imperative/reactive design
 * - TextareaController CLASS: Pure TypeScript, works in tests without Svelte context
 * - createTextareaController FACTORY: Wraps class with reactive effects for components
 *
 * Key Design Decisions (Issue #695):
 * - Dual usage: Class can be instantiated directly in tests, or via factory in components
 * - Content sync effect: KEPT - necessary for external content changes (SSE, undo/redo)
 * - Config sync effect: ELIMINATED - controller stores getter, reads on-demand
 * - Early-returns in updateContent() prevent unnecessary DOM operations
 *
 * Effect Elimination Strategy:
 * - Config: Controller stores getConfig getter, reads via this.config getter
 *   Tests pass simple functions: () => ({ allowMultiline: true })
 * - Content: Effect kept because external changes need to sync to textarea DOM
 *   The early-return (if textarea.value === content) makes it efficient
 *
 * Migration Pattern (from original class-based controller):
 * - Element passed via callback for fine-grained reactivity
 * - Config passed as getter (no effect needed)
 * - Content synced via effect (external changes)
 * - Internal state uses $state for Svelte 5 reactivity
 */

import type { TriggerContext } from '$lib/services/content-processor';
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
import { focusManager } from '$lib/services/focus-manager.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { tabState } from '$lib/stores/navigation';
import { get } from 'svelte/store';
import { untrack } from 'svelte';
import {
  PatternState,
  type NodeCreationSource
} from '$lib/state/pattern-state.svelte';
import {
  mapViewPositionToEditPosition,
  stripAllMarkdown
} from '$lib/utils/view-edit-mapper';

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
  contentChanged: (content: string, cursorPosition: number) => void;
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
  /**
   * Whether other nodes can merge into this node via Backspace
   * Set to false for structured nodes (code-block, quote-block) that can't accept arbitrary merges
   * @default true
   * @example
   * // Code block with merge prevention
   * const editableConfig = { allowMultiline: true, allowMergeInto: false };
   */
  allowMergeInto?: boolean;
}

/**
 * TextareaController state interface
 */
export interface TextareaControllerState {
  // Dropdown state
  slashCommandDropdownActive: boolean;
  autocompleteDropdownActive: boolean;
  // Status flags
  recentEnter: boolean;
  justCreated: boolean;
  // Expose as getters for external access
  getMarkdownContent(): string;
  getCursorPosition(): number;
  isAtFirstLine(): boolean;
  isAtLastLine(): boolean;
  getCurrentColumn(): number;
  getCurrentPixelOffset(): number;
  // Methods
  initialize(content: string, autoFocus?: boolean): void;
  focus(): void;
  setCursorPosition(position: number): void;
  updateContent(content: string): void;
  forceUpdateContent(content: string): void;
  adjustHeight(): void;
  positionCursorAtLineBeginning(lineNumber?: number, skipSyntax?: boolean): void;
  enterFromArrowNavigation(direction: 'up' | 'down', pixelOffset: number): void;
  setSlashCommandDropdownActive(active: boolean): void;
  setAutocompleteDropdownActive(active: boolean): void;
  insertNodeReference(nodeId: string, title?: string): void;
  insertSlashCommand(content: string, skipDetection: boolean, targetNodeType?: string): number;
  toggleFormatting(marker: string): void;
  isProcessingInput(): boolean;
  getCursorPositionInMarkdown(): number;
  convertHtmlToTextWithNewlines(html: string): string;
  setLiveFormattedContent(content: string): void;
  prepareForArrowNavigation(): void;
  updateNodeType(newNodeType: string): void;
  destroy(): void;
}

/**
 * Create a reactive textarea controller
 * Props are passed as getter functions for fine-grained reactivity
 */

// Keep track of command registration state
let keyboardCommandsRegistered = false;

// Module-level services and state
const cursorService = CursorPositioningService.getInstance();
const MAX_QUERY_LENGTH = 100;

// TextareaController - Core implementation class
// Can be used directly in tests with 'new TextareaController(...)'
// Or used reactively via createTextareaController() factory in components
export class TextareaController {
    public element: HTMLTextAreaElement;
    private nodeId: string;
    private nodeType: string;
    private paneId: string;
    /**
     * Config getter - reads current config on-demand (Issue #695)
     * Eliminates the need for config sync $effect by reading from getter
     * Tests can pass simple functions: () => ({ allowMultiline: true })
     */
    private getConfig: () => TextareaControllerConfig;
    public events: TextareaControllerEvents;

    private isInitialized: boolean = false;
    /**
     * Pattern state machine - manages pattern detection lifecycle
     * Replaces the old nodeTypeSetViaPattern boolean flag (Issue #664)
     */
    private patternState: PatternState;
    private lastKnownPixelOffset: number = 0;
    private measurementElement: HTMLSpanElement | null = null;

    public slashCommandDropdownActive: boolean = false;
    public autocompleteDropdownActive: boolean = false;

    private slashCommandSession: { startPosition: number; active: boolean } | null = null;
    private mentionSession: { startPosition: number; active: boolean } | null = null;

    private skipPatternDetection: boolean = false;
    public recentEnter: boolean = false;
    public justCreated: boolean = false;
    private focusedViaArrowNavigation: boolean = false;
    private pendingCursorPosition: number | null = null;

    private boundHandleFocus = this.handleFocus.bind(this);
    private boundHandleBlur = this.handleBlur.bind(this);
    private boundHandleInput = this.handleInput.bind(this);
    private boundHandleKeyDown = this.handleKeyDown.bind(this);

    constructor(
      element: HTMLTextAreaElement,
      nodeId: string,
      nodeType: string,
      paneId: string,
      events: TextareaControllerEvents,
      /**
       * Config getter - reads current config on-demand (Issue #695)
       * Pass a function that returns the config object
       * Factory passes reactive getter, tests pass simple functions
       */
      getConfig: () => TextareaControllerConfig = () => ({}),
      /**
       * Creation source for pattern state (Issue #664)
       * - 'user': User created node (patterns can be detected)
       * - 'pattern': Node created via pattern detection (can revert to text)
       * - 'inherited': Node inherits type from parent (cannot revert)
       * If not provided, inferred from focusManager for backward compatibility
       */
      creationSource?: NodeCreationSource
    ) {
      this.element = element;
      this.nodeId = nodeId;
      this.nodeType = nodeType;
      this.paneId = paneId;
      this.events = events;
      // Store config getter - reads on-demand instead of via $effect (Issue #695)
      this.getConfig = getConfig;

      // Initialize pattern state (Issue #664)
      // If creationSource not provided, infer from focusManager for backward compatibility
      const cursorType = focusManager.cursorPosition?.type;
      const isTypeConversion = cursorType === 'node-type-conversion';
      const isInheritedType = cursorType === 'inherited-type';

      // Determine creation source:
      // - 'inherited': Enter key on typed node (cannot revert)
      // - 'pattern': Pattern detection conversion (can revert)
      // - 'user': Default (patterns can be detected)
      let effectiveSource: NodeCreationSource;
      if (creationSource) {
        effectiveSource = creationSource;
      } else if (isInheritedType && nodeType !== 'text') {
        effectiveSource = 'inherited';
      } else if (isTypeConversion && nodeType !== 'text') {
        effectiveSource = 'pattern';
      } else {
        effectiveSource = 'user';
      }

      // For inherited nodes, lookup the plugin to respect its canRevert setting
      // This fixes the bug where inherited task nodes would revert to text on first keystroke
      // because the legacy canRevert fallback returned true unconditionally
      const plugin =
        effectiveSource === 'inherited' ? pluginRegistry.getPlugin(nodeType) ?? undefined : undefined;

      this.patternState = new PatternState(effectiveSource, plugin);

      (this.element as unknown as { _textareaController: TextareaController })._textareaController =
        this;

      this.registerKeyboardCommands();
      this.setupEventListeners();
    }

    /**
     * Config getter - reads from getConfig() on-demand (Issue #695)
     * This eliminates the config sync $effect by reading reactively
     */
    private get config(): TextareaControllerConfig {
      return { allowMultiline: false, ...this.getConfig() };
    }

    private registerKeyboardCommands(): void {
      if (keyboardCommandsRegistered) {
        return;
      }

      const registry = KeyboardCommandRegistry.getInstance();

      registry.register({ key: 'Enter' }, KEYBOARD_COMMANDS.createNode);
      registry.register({ key: 'Tab' }, KEYBOARD_COMMANDS.indent);
      registry.register({ key: 'Tab', shift: true }, KEYBOARD_COMMANDS.outdent);
      registry.register({ key: 'Backspace' }, KEYBOARD_COMMANDS.mergeUp);

      registry.register({ key: 'ArrowUp' }, KEYBOARD_COMMANDS.navigateUp);
      registry.register({ key: 'ArrowDown' }, KEYBOARD_COMMANDS.navigateDown);

      registry.register({ key: 'b', meta: true }, KEYBOARD_COMMANDS.formatBold);
      registry.register({ key: 'b', ctrl: true }, KEYBOARD_COMMANDS.formatBold);
      registry.register({ key: 'i', meta: true }, KEYBOARD_COMMANDS.formatItalic);
      registry.register({ key: 'i', ctrl: true }, KEYBOARD_COMMANDS.formatItalic);

      keyboardCommandsRegistered = true;
    }

    // NOTE: updateConfig() REMOVED (Issue #695)
    // Config is now read on-demand via the config getter, which calls getConfig()
    // The $effect that previously synced config changes is eliminated
    // Config updates now automatically reflect when getConfig() returns new values

    public initialize(content: string, autoFocus: boolean = false): void {
      if (this.isInitialized) {
        return;
      }

      this.element.value = content;

      // Check if FocusManager has a pending cursor position
      // Issue #669: ANY cursor position type should skip the default setCursorAtBeginningOfLine
      // because the positionCursor action handles all cursor positioning cases.
      // Previously only checked for 'node-type-conversion', but 'absolute', 'default', etc.
      // also need to be handled by the action, not overridden here.
      const hasPendingCursorPosition = focusManager.cursorPosition !== null;
      const isTypeConversion = focusManager.cursorPosition?.type === 'node-type-conversion';

      // Clear cursor position AFTER checking type
      // The positionCursor action will handle cursor positioning
      // This must happen here (not in the action) to avoid a race condition where
      // RAF runs before initialize() and clears the position before we can check it
      // Issue #669: Clear for ALL position types, not just node-type-conversion
      if (hasPendingCursorPosition) {
        // Use RAF to ensure positionCursor action has a chance to read and process the position
        requestAnimationFrame(() => {
          focusManager.clearCursorPosition();
        });
      }

      // Initialize pattern state for non-text types (Issue #664)
      // This enables reversion to text type when the pattern is deleted
      if (this.nodeType !== 'text') {
        // If this component is being created due to a type conversion (via pattern detection),
        // the patternState was already set to 'pattern' in constructor
        if (!isTypeConversion) {
          // For non-conversion cases (e.g., page load), check if content matches pattern
          const detection = pluginRegistry.detectPatternInContent(content);
          if (detection && detection.config.targetNodeType === this.nodeType) {
            // Content matches pattern - enable reversion capability (Issue #667)
            this.patternState.setPluginPatternExists(detection.plugin);
          }
        }
      }

      if (autoFocus) {
        this.justCreated = true;
        setTimeout(() => {
          this.justCreated = false;
        }, 50);

        // Issue #669: Skip cursor positioning here if FocusManager has a pending position
        // The positionCursor action will handle ALL cursor positioning types
        // This prevents the cursor from being positioned at 0 before the action can apply
        // the correct position (e.g., 'absolute' position from Enter key node creation)
        if (!hasPendingCursorPosition) {
          cursorService.setCursorAtBeginningOfLine(this.element, 0, {
            focus: true,
            delay: 0,
            skipSyntax: true
          });
        }
      }

      this.isInitialized = true;
      this.adjustHeight();
    }

    public updateContent(content: string): void {
      if (this.element.value === content) {
        return;
      }

      if (this.recentEnter) {
        return;
      }

      // Preserve cursor position when updating content from external source
      // Setting element.value resets cursor to position 0
      const cursorPosition = this.element.selectionStart;
      const cursorEnd = this.element.selectionEnd;

      this.element.value = content;
      this.adjustHeight();

      // Restore cursor position, clamping to new content length
      const newPosition = Math.min(cursorPosition, content.length);
      const newEnd = Math.min(cursorEnd, content.length);
      this.element.setSelectionRange(newPosition, newEnd);
    }

    public forceUpdateContent(content: string): void {
      requestAnimationFrame(() => {
        this.element.value = content;
        this.adjustHeight();
        this.setCursorPosition(content.length);
      });
    }

    public updateNodeType(newNodeType: string): void {
      this.nodeType = newNodeType;
    }

    public prepareForArrowNavigation(): void {
      this.focusedViaArrowNavigation = true;
    }

    public focus(): void {
      this.element.focus();

      if (this.pendingCursorPosition !== null) {
        this.setCursorPosition(this.pendingCursorPosition);
        this.pendingCursorPosition = null;
      }
    }

    public positionCursorAtLineBeginning(lineNumber: number = 0, skipSyntax: boolean = true): void {
      cursorService.setCursorAtBeginningOfLine(this.element, lineNumber, {
        focus: false,
        delay: 0,
        skipSyntax
      });
    }

    public getMarkdownContent(): string {
      return this.element.value;
    }

    public destroy(): void {
      this.removeEventListeners();
      delete (this.element as unknown as { _textareaController?: TextareaController })
        ._textareaController;
      if (this.measurementElement && this.measurementElement.parentNode) {
        this.measurementElement.parentNode.removeChild(this.measurementElement);
        this.measurementElement = null;
      }
    }

    private setupEventListeners(): void {
      this.element.addEventListener('focus', this.boundHandleFocus);
      this.element.addEventListener('blur', this.boundHandleBlur);
      this.element.addEventListener('input', this.boundHandleInput);
      this.element.addEventListener('keydown', this.boundHandleKeyDown);
    }

    private removeEventListeners(): void {
      this.element.removeEventListener('focus', this.boundHandleFocus);
      this.element.removeEventListener('blur', this.boundHandleBlur);
      this.element.removeEventListener('input', this.boundHandleInput);
      this.element.removeEventListener('keydown', this.boundHandleKeyDown);
    }

    public adjustHeight(): void {
      this.element.style.height = 'auto';
      this.element.style.height = `${this.element.scrollHeight}px`;
    }

    public setCursorPosition(position: number): void {
      this.element.selectionStart = position;
      this.element.selectionEnd = position;
    }

    public getCursorPosition(): number {
      return this.element.selectionStart;
    }

    private isAtStart(): boolean {
      return this.element.selectionStart === 0;
    }

    private isAtEnd(): boolean {
      return this.element.selectionStart === this.element.value.length;
    }

    public isAtFirstLine(): boolean {
      const position = this.element.selectionStart;
      const textBefore = this.element.value.substring(0, position);
      return !textBefore.includes('\n');
    }

    public isAtLastLine(): boolean {
      const position = this.element.selectionStart;
      const content = this.element.value;

      const textBefore = content.substring(0, position);
      const newlinesBeforeCursor = (textBefore.match(/\n/g) || []).length;

      const totalNewlines = (content.match(/\n/g) || []).length;

      if (totalNewlines === 0) {
        return true;
      }

      const hasTrailingNewline = content.endsWith('\n');

      if (hasTrailingNewline) {
        return newlinesBeforeCursor >= totalNewlines;
      } else {
        return newlinesBeforeCursor >= totalNewlines;
      }
    }

    public getCurrentColumn(): number {
      const position = this.element.selectionStart;
      const textBefore = this.element.value.substring(0, position);
      const lastNewline = textBefore.lastIndexOf('\n');
      return lastNewline === -1 ? position : position - lastNewline - 1;
    }

    public getCurrentPixelOffset(): number {
      const position = this.element.selectionStart;
      const content = this.element.value;
      const textBefore = content.substring(0, position);
      const lastNewline = textBefore.lastIndexOf('\n');
      const currentLineStart = lastNewline === -1 ? 0 : lastNewline + 1;
      const textBeforeCursorOnLine = content.substring(currentLineStart, position);

      const rect = this.element.getBoundingClientRect();

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

      // Measure the ACTUAL text before cursor (including markdown syntax)
      // This gives us the true visual position of the cursor on screen
      this.measurementElement.textContent = textBeforeCursorOnLine;
      const textWidth = this.measurementElement.getBoundingClientRect().width;

      this.lastKnownPixelOffset = rect.left + textWidth + window.scrollX;
      return this.lastKnownPixelOffset;
    }

    public enterFromArrowNavigation(direction: 'up' | 'down', pixelOffset: number): void {
      const content = this.element.value;
      const lines = content.split('\n');

      const lineIndex = direction === 'up' ? lines.length - 1 : 0;
      const targetLine = lines[lineIndex];

      // Get the textarea's left edge position
      const rect = this.element.getBoundingClientRect();
      const textareaLeftEdge = rect.left + window.scrollX;
      const relativePixelOffset = pixelOffset - textareaLeftEdge;

      // The pixelOffset represents the VISUAL cursor position from source's EDIT mode
      // But the target node displays in VIEW mode first, then switches to EDIT mode
      // So we need to:
      // 1. Find the column in VIEW text that matches the pixel offset
      // 2. Map that view column to an edit column (accounting for syntax that will appear)

      // Get the view text (what user sees in display mode - no syntax)
      const viewLine = stripAllMarkdown(targetLine);

      // Find column in view text that matches the pixel offset
      const viewColumn = this.findColumnForPixelOffset(viewLine, relativePixelOffset);

      // Map view column to edit column (accounting for markdown syntax)
      const editColumn = mapViewPositionToEditPosition(viewColumn, viewLine, targetLine);

      // Calculate absolute position
      let position = 0;
      for (let i = 0; i < lineIndex; i++) {
        position += lines[i].length + 1;
      }
      position += editColumn;

      this.focus();
      this.setCursorPosition(position);
    }

    /**
     * Find the column position in a line that best matches a target pixel offset
     * Uses binary search with proper text measurement for accurate positioning
     */
    private findColumnForPixelOffset(line: string, targetPixelOffset: number): number {
      if (line.length === 0 || targetPixelOffset <= 0) {
        return 0;
      }

      // Create or reuse measurement element with textarea's font styles
      // In test environments (Happy-DOM), getComputedStyle may not be available
      // In that case, fall back to approximate character width
      if (!this.measurementElement) {
        this.measurementElement = document.createElement('span');

        // Check if getComputedStyle is available (browser environment)
        if (typeof window !== 'undefined' && typeof window.getComputedStyle === 'function') {
          try {
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
          } catch {
            // getComputedStyle failed, use fallback
            this.measurementElement = null;
          }
        } else {
          // Test environment - getComputedStyle not available
          this.measurementElement = null;
        }
      }

      // If measurement element is not available, use approximate calculation
      // This provides reasonable behavior in test environments
      if (!this.measurementElement) {
        const approximateCharWidth = 8; // Average monospace character width
        const approximateColumn = Math.max(0, Math.round(targetPixelOffset / approximateCharWidth));
        return Math.min(approximateColumn, line.length);
      }

      // Binary search for the best column position
      let low = 0;
      let high = line.length;
      let bestColumn = 0;
      let bestDiff = Infinity;

      while (low <= high) {
        const mid = Math.floor((low + high) / 2);
        this.measurementElement.textContent = line.substring(0, mid);
        const width = this.measurementElement.getBoundingClientRect().width;
        const diff = Math.abs(width - targetPixelOffset);

        if (diff < bestDiff) {
          bestDiff = diff;
          bestColumn = mid;
        }

        if (width < targetPixelOffset) {
          low = mid + 1;
        } else if (width > targetPixelOffset) {
          high = mid - 1;
        } else {
          // Exact match
          return mid;
        }
      }

      return bestColumn;
    }

    private handleFocus(): void {
      this.events.focus();
    }

    private handleBlur(): void {
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
      const cursorPosition = this.element.selectionStart ?? content.length;

      this.events.contentChanged(content, cursorPosition);

      this.adjustHeight();

      if (!this.skipPatternDetection) {
        this.detectPatterns();
      }

      this.skipPatternDetection = false;
    }

    private async handleKeyDown(event: KeyboardEvent): Promise<void> {
      const registry = KeyboardCommandRegistry.getInstance();

      if (this.slashCommandDropdownActive) {
        if (['Enter', 'Escape', 'ArrowUp', 'ArrowDown'].includes(event.key)) {
          return;
        }
      }

      if (this.autocompleteDropdownActive) {
        if (['Enter', 'Escape', 'ArrowUp', 'ArrowDown'].includes(event.key)) {
          return;
        }
      }

      const context = this.buildKeyboardContext(event);
      const handled = await registry.execute(event, this, context);

      if (handled) {
        return;
      }

      if (event.key === 'Enter' && event.shiftKey) {
        if (this.config.allowMultiline) {
          return;
        } else {
          event.preventDefault();
          return;
        }
      }
    }

    private buildKeyboardContext(event: KeyboardEvent): Record<string, unknown> {
      const currentTabState = get(tabState);
      const activePaneId = currentTabState.activePaneId;

      return {
        event,
        controller: this,
        nodeId: this.nodeId,
        nodeType: this.nodeType,
        paneId: this.paneId,
        content: this.element.value,
        cursorPosition: this.getCursorPosition(),
        selection: window.getSelection(),
        allowMultiline: this.config.allowMultiline,
        metadata: {
          activePaneId
        }
      };
    }

    private detectPatterns(): void {
      const content = this.element.value;
      const position = this.getCursorPosition();

      this.detectMentionTrigger(content, position);
      this.detectSlashCommandTrigger(content, position);
      this.detectNodeTypeConversion(content);
    }

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

      const charBefore = atIndex > 0 ? textBefore[atIndex - 1] : ' ';
      const isAtWordBoundary = /\s/.test(charBefore) || atIndex === 0;

      if (!isAtWordBoundary || query.length > MAX_QUERY_LENGTH) {
        if (this.mentionSession?.active) {
          this.events.triggerHidden();
          this.mentionSession = null;
        }
        return;
      }

      if (!this.mentionSession || !this.mentionSession.active) {
        this.mentionSession = { startPosition: atIndex, active: true };
      }

      const cursorCoords = this.getCursorCoordinates();

      this.events.triggerDetected({
        triggerContext: {
          trigger: '@',
          query: query,
          startPosition: atIndex,
          endPosition: position,
          element: this.element,
          isValid: true
        },
        cursorPosition: cursorCoords
      });
    }

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

      if (!this.slashCommandSession || !this.slashCommandSession.active) {
        this.slashCommandSession = { startPosition: slashIndex, active: true };
      }

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

    private detectNodeTypeConversion(content: string): void {
      // All nodes should detect pattern changes - inherited nodes behave the same
      // as pattern-detected nodes for reversion (user clarification)
      const detection = pluginRegistry.detectPatternInContent(content);

      if (detection) {
        const { plugin, config, match } = detection;

        if (this.nodeType === config.targetNodeType) {
          // Node type already matches - record pattern for reversion capability (Issue #667)
          this.patternState.setPluginPatternExists(plugin);
          return;
        }

        const cursorPosition =
          config.desiredCursorPosition !== undefined
            ? config.desiredCursorPosition
            : this.getCursorPosition();

        let cleanedContent: string;
        if (config.contentTemplate) {
          cleanedContent = config.contentTemplate;
        } else if (config.cleanContent) {
          cleanedContent = content.replace(match[0], '');
        } else {
          cleanedContent = content;
        }

        untrack(() => {
          this.events.nodeTypeConversionDetected({
            nodeId: this.nodeId,
            newNodeType: config.targetNodeType,
            cleanedContent: cleanedContent,
            cursorPosition: cursorPosition
          });

          this.nodeType = config.targetNodeType;
          // Record pattern match for reversion capability (Issue #667)
          this.patternState.recordPluginPatternMatch(plugin);
        });
      } else if (this.nodeType !== 'text' && this.patternState.canRevert) {
        // No pattern detected and node is not text - revert to text
        // This handles both pattern-detected and inherited nodes when syntax is deleted
        // e.g., "# Hello" -> "#Hello" (space deleted, no longer matches header pattern)
        //
        // IMPORTANT: Only revert if canRevert is true. Patterns with cleanContent: true
        // (like tasks) intentionally remove their syntax, so they should NOT revert
        // just because the pattern no longer matches.
        const cursorPosition = this.getCursorPosition();

        untrack(() => {
          this.events.nodeTypeConversionDetected({
            nodeId: this.nodeId,
            newNodeType: 'text',
            cleanedContent: content,
            cursorPosition: cursorPosition
          });

          this.nodeType = 'text';
          // Reset pattern state to user mode (enables future pattern detection)
          this.patternState.resetToUser();
        });
      }
    }

    private getCursorCoordinates(): { x: number; y: number } {
      const rect = this.element.getBoundingClientRect();

      const position = this.getCursorPosition();
      const textBefore = this.element.value.substring(0, position);
      const lines = textBefore.split('\n');
      const lineNumber = lines.length - 1;
      const lineHeight = 24;

      return {
        x: rect.left,
        y: rect.top + lineNumber * lineHeight + lineHeight
      };
    }

    public setSlashCommandDropdownActive(active: boolean): void {
      this.slashCommandDropdownActive = active;
    }

    public setAutocompleteDropdownActive(active: boolean): void {
      this.autocompleteDropdownActive = active;
    }

    public insertNodeReference(nodeId: string, title?: string): void {
      if (!this.mentionSession) return;

      const content = this.element.value;
      const before = content.substring(0, this.mentionSession.startPosition);
      const after = content.substring(this.getCursorPosition());

      // Use title as display text, falling back to empty (will show nodeId in view mode)
      const displayText = title || '';
      const reference = `[${displayText}](nodespace://${nodeId})`;
      const newContent = before + reference + after;

      this.element.value = newContent;
      const newCursorPosition = before.length + reference.length;
      this.setCursorPosition(newCursorPosition);

      this.mentionSession = null;
      this.events.contentChanged(newContent, newCursorPosition);
      this.adjustHeight();
    }

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
      this.events.contentChanged(newContent, cursorPosition);
      this.adjustHeight();

      return cursorPosition;
    }

    public toggleFormatting(marker: string): void {
      const start = this.element.selectionStart;
      const end = this.element.selectionEnd;
      const content = this.element.value;

      if (start === end) {
        return;
      }

      const selectedText = content.substring(start, end);
      const before = content.substring(0, start);
      const after = content.substring(end);

      const markerLength = marker.length;
      const isFormatted = before.endsWith(marker) && after.startsWith(marker);

      let newContent: string;
      let newCursorStart: number;
      let newCursorEnd: number;

      if (isFormatted) {
        newContent =
          before.substring(0, before.length - markerLength) +
          selectedText +
          after.substring(markerLength);
        newCursorStart = start - markerLength;
        newCursorEnd = end - markerLength;
      } else {
        newContent = before + marker + selectedText + marker + after;
        newCursorStart = start + markerLength;
        newCursorEnd = end + markerLength;
      }

      this.element.value = newContent;
      this.element.selectionStart = newCursorStart;
      this.element.selectionEnd = newCursorEnd;

      this.events.contentChanged(newContent, newCursorEnd);
      this.adjustHeight();
    }

    public isProcessingInput(): boolean {
      return this.slashCommandDropdownActive || this.autocompleteDropdownActive;
    }

    public getCursorPositionInMarkdown(): number {
      return this.element.selectionStart;
    }

    public convertHtmlToTextWithNewlines(_html: string): string {
      return this.element.value;
    }

    public setLiveFormattedContent(content: string): void {
      this.element.value = content;
      this.adjustHeight();
    }
  }

// Re-export NodeCreationSource for use by components
export type { NodeCreationSource } from '$lib/state/pattern-state.svelte';

// Factory function for reactive controller with Svelte 5 runes
export function createTextareaController(
  // Element getter for fine-grained reactivity
  getElement: () => HTMLTextAreaElement | undefined,
  // Props passed as getters
  getNodeId: () => string,
  getNodeType: () => string,
  getPaneId: () => string,
  getContent: () => string,
  getEditableConfig: () => TextareaControllerConfig,
  // Events and config
  events: TextareaControllerEvents,
  /**
   * Pattern state creation source (Issue #664)
   * Controls how pattern detection behaves for this node:
   * - 'user': User-created node, patterns can be detected and can revert
   * - 'pattern': Created via pattern detection, can revert to text
   * - 'inherited': Inherited type from parent (Enter key), cannot revert
   * If not provided, inferred from focusManager for backward compatibility
   */
  creationSource?: NodeCreationSource
): TextareaControllerState {
  // Internal mutable state (using $state for Svelte 5 reactivity)
  let controller: TextareaController | null = null;

  // Watch for element changes and manage controller lifecycle
  $effect(() => {
    const element = getElement();
    const nodeId = getNodeId();
    const nodeType = getNodeType();
    const paneId = getPaneId();

    untrack(() => {
      if (element && !controller) {
        // Pass getEditableConfig directly - controller stores getter and reads on-demand
        // This eliminates the need for config sync $effect (Issue #695)
        controller = new TextareaController(
          element,
          nodeId,
          nodeType,
          paneId,
          events,
          getEditableConfig,
          creationSource
        );
      } else if (!element && controller) {
        controller.destroy();
        controller = null;
      }
    });
  });

  // ============================================================================
  // Content Sync Effect - JUSTIFIED AS NECESSARY (Issue #695)
  // ============================================================================
  //
  // WHY THIS EFFECT IS REQUIRED:
  // The TextareaController is a vanilla TypeScript class that doesn't inherently
  // react to prop changes. This effect bridges Svelte's reactive system with
  // the imperative controller by syncing external content changes.
  //
  // WHEN THIS RUNS:
  // - External content changes (database sync via SSE, undo/redo operations)
  // - Parent component programmatic updates
  // - Initial prop hydration
  //
  // PERFORMANCE CONSIDERATIONS:
  // - Early-return in updateContent() when textarea value matches prop prevents
  //   unnecessary DOM operations during user typing
  // - The overhead is minimal: just a function call and string comparison
  //
  // ALTERNATIVE CONSIDERED (Issue #695):
  // Making the controller read directly from getters instead of caching values
  // would eliminate this effect BUT would:
  // 1. Break test isolation (controller would require reactive context)
  // 2. Make the controller tightly coupled to Svelte's reactivity
  // 3. Prevent direct instantiation in tests with 'new TextareaController(...)'
  //
  // CONCLUSION: Keep effect - necessary for external sync, minimal overhead
  // ============================================================================
  $effect(() => {
    const content = getContent();
    if (controller && content !== undefined) {
      controller.updateContent(content);
    }
  });

  // ============================================================================
  // Config Sync Effect - ELIMINATED (Issue #695)
  // ============================================================================
  //
  // PREVIOUS APPROACH: $effect synced config changes via updateConfig()
  //
  // NEW APPROACH: Controller stores getConfig getter and reads on-demand
  // via the private `config` getter property. No effect needed.
  //
  // BENEFITS:
  // - One fewer $effect in the codebase (reduces effect-related bugs)
  // - Config is read when needed, not pushed on every change
  // - Tests can pass simple functions: () => ({ allowMultiline: true })
  // ============================================================================

  // Cleanup on destroy
  $effect.pre(() => {
    return () => {
      if (controller) {
        controller.destroy();
        controller = null;
      }
    };
  });

  // Return reactive state interface that delegates to controller
  // This allows base-node.svelte to work with the controller reactively
  // while the factory's internal effects handle prop syncing
  return {
    get slashCommandDropdownActive(): boolean {
      return controller?.slashCommandDropdownActive ?? false;
    },
    set slashCommandDropdownActive(value: boolean) {
      if (controller) {
        controller.slashCommandDropdownActive = value;
      }
    },

    get autocompleteDropdownActive(): boolean {
      return controller?.autocompleteDropdownActive ?? false;
    },
    set autocompleteDropdownActive(value: boolean) {
      if (controller) {
        controller.autocompleteDropdownActive = value;
      }
    },

    get recentEnter(): boolean {
      return controller?.recentEnter ?? false;
    },
    set recentEnter(value: boolean) {
      if (controller) {
        controller.recentEnter = value;
      }
    },

    get justCreated(): boolean {
      return controller?.justCreated ?? false;
    },
    set justCreated(value: boolean) {
      if (controller) {
        controller.justCreated = value;
      }
    },

    getMarkdownContent(): string {
      return controller?.getMarkdownContent() ?? '';
    },

    getCursorPosition(): number {
      return controller?.getCursorPosition() ?? 0;
    },

    isAtFirstLine(): boolean {
      return controller?.isAtFirstLine() ?? false;
    },

    isAtLastLine(): boolean {
      return controller?.isAtLastLine() ?? false;
    },

    getCurrentColumn(): number {
      return controller?.getCurrentColumn() ?? 0;
    },

    getCurrentPixelOffset(): number {
      return controller?.getCurrentPixelOffset() ?? 0;
    },

    focus(): void {
      controller?.focus();
    },

    setCursorPosition(position: number): void {
      controller?.setCursorPosition(position);
    },

    updateContent(content: string): void {
      controller?.updateContent(content);
    },

    forceUpdateContent(content: string): void {
      controller?.forceUpdateContent(content);
    },

    initialize(content: string, autoFocus?: boolean): void {
      controller?.initialize(content, autoFocus);
    },

    adjustHeight(): void {
      controller?.adjustHeight();
    },

    positionCursorAtLineBeginning(lineNumber?: number, skipSyntax?: boolean): void {
      controller?.positionCursorAtLineBeginning(lineNumber, skipSyntax);
    },

    enterFromArrowNavigation(direction: 'up' | 'down', pixelOffset: number): void {
      controller?.enterFromArrowNavigation(direction, pixelOffset);
    },

    setSlashCommandDropdownActive(active: boolean): void {
      if (controller) {
        controller.setSlashCommandDropdownActive(active);
      }
    },

    setAutocompleteDropdownActive(active: boolean): void {
      if (controller) {
        controller.setAutocompleteDropdownActive(active);
      }
    },

    insertNodeReference(nodeId: string, title?: string): void {
      controller?.insertNodeReference(nodeId, title);
    },

    insertSlashCommand(content: string, skipDetection: boolean, targetNodeType?: string): number {
      return controller?.insertSlashCommand(content, skipDetection, targetNodeType) ?? 0;
    },

    toggleFormatting(marker: string): void {
      controller?.toggleFormatting(marker);
    },

    isProcessingInput(): boolean {
      return controller?.isProcessingInput() ?? false;
    },

    getCursorPositionInMarkdown(): number {
      return controller?.getCursorPositionInMarkdown() ?? 0;
    },

    convertHtmlToTextWithNewlines(html: string): string {
      return controller?.convertHtmlToTextWithNewlines(html) ?? '';
    },

    setLiveFormattedContent(content: string): void {
      controller?.setLiveFormattedContent(content);
    },

    prepareForArrowNavigation(): void {
      controller?.prepareForArrowNavigation();
    },

    updateNodeType(newNodeType: string): void {
      controller?.updateNodeType(newNodeType);
    },

    destroy(): void {
      controller?.destroy();
      controller = null;
    }
  };
}
