/**
 * TextareaController (Svelte 5 Reactive Version)
 *
 * Converted from class to reactive factory function using Svelte 5 runes.
 * Props are passed as getter functions for automatic reactivity.
 *
 * Key improvements:
 * - Content sync: Automatic via reactive prop getters (eliminates 2 $effect blocks)
 * - Dropdown state: Reactive properties (eliminates 2 $effect blocks)
 * - Single source of truth: Textarea value is still canonical
 * - Pure TypeScript class logic preserved: All business logic unchanged
 *
 * Migration Pattern:
 * - Element passed via callback for fine-grained reactivity
 * - Props passed as getters for auto-tracking
 * - Internal state uses $state for Svelte 5 reactivity
 * - Effects only for true side-effects (event listeners, cleanup)
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
  insertNodeReference(nodeId: string): void;
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
    private config: TextareaControllerConfig;
    public events: TextareaControllerEvents;

    private isInitialized: boolean = false;
    private nodeTypeSetViaPattern: boolean = false;
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
      config: TextareaControllerConfig = {}
    ) {
      this.element = element;
      this.nodeId = nodeId;
      this.nodeType = nodeType;
      this.paneId = paneId;
      this.events = events;
      this.config = { allowMultiline: false, ...config };

      // CRITICAL: Check if this controller is being created due to a type conversion
      // If so, set the flag immediately BEFORE event listeners are attached
      // This ensures input events processed before initialize() will see the correct flag
      const isTypeConversion = focusManager.cursorPosition?.type === 'node-type-conversion';
      if (isTypeConversion && this.nodeType !== 'text') {
        this.nodeTypeSetViaPattern = true;
      }

      (this.element as unknown as { _textareaController: TextareaController })._textareaController =
        this;

      this.registerKeyboardCommands();
      this.setupEventListeners();
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

    public updateConfig(config: Partial<TextareaControllerConfig>): void {
      this.config = { ...this.config, ...config };
    }

    public initialize(content: string, autoFocus: boolean = false): void {
      if (this.isInitialized) return;

      this.element.value = content;

      // Check if this is a node type conversion in progress
      // Node type conversions are signaled via focusManager.cursorPosition.type
      const isTypeConversion = focusManager.cursorPosition?.type === 'node-type-conversion';

      // Set nodeTypeSetViaPattern flag for non-text types
      // This enables reversion to text type when the pattern is deleted
      if (this.nodeType !== 'text') {
        // If this component is being created due to a type conversion (via pattern detection),
        // always set the flag - the node was created via pattern, so deleting the pattern
        // should revert it back to text
        if (isTypeConversion) {
          this.nodeTypeSetViaPattern = true;
        } else {
          // For non-conversion cases (e.g., page load), check if content matches pattern
          const detection = pluginRegistry.detectPatternInContent(content);
          if (detection && detection.config.targetNodeType === this.nodeType) {
            this.nodeTypeSetViaPattern = true;
          }
        }
      }

      if (autoFocus) {
        this.justCreated = true;
        setTimeout(() => {
          this.justCreated = false;
        }, 50);

        // If this is a type conversion, skip cursor positioning - let positionCursor action handle it
        // This prevents the cursor from jumping to the beginning during type conversions
        if (!isTypeConversion) {
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

      this.element.value = content;
      this.adjustHeight();
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
      const textBefore = this.element.value.substring(0, position);
      const lastNewline = textBefore.lastIndexOf('\n');
      const currentLineStart = lastNewline === -1 ? 0 : lastNewline + 1;
      const textBeforeCursor = this.element.value.substring(currentLineStart, position);

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

      this.measurementElement.textContent = textBeforeCursor;
      const textWidth = this.measurementElement.getBoundingClientRect().width;

      this.lastKnownPixelOffset = rect.left + textWidth + window.scrollX;
      return this.lastKnownPixelOffset;
    }

    public enterFromArrowNavigation(direction: 'up' | 'down', pixelOffset: number): void {
      const content = this.element.value;
      const lines = content.split('\n');

      const lineIndex = direction === 'up' ? lines.length - 1 : 0;
      const targetLine = lines[lineIndex];

      const rect = this.element.getBoundingClientRect();
      const textareaLeftEdge = rect.left + window.scrollX;
      const relativePixelOffset = pixelOffset - textareaLeftEdge;

      const approximateColumn = Math.max(0, Math.round(relativePixelOffset / 8));

      const column = Math.min(approximateColumn, targetLine.length);

      let position = 0;
      for (let i = 0; i < lineIndex; i++) {
        position += lines[i].length + 1;
      }
      position += column;

      this.setCursorPosition(position);
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
      const detection = pluginRegistry.detectPatternInContent(content);

      if (detection) {
        const { config, match } = detection;

        if (this.nodeType === config.targetNodeType) {
          // Node type already matches - just ensure the flag is set
          // This enables reversion when the pattern is later deleted
          this.nodeTypeSetViaPattern = true;
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
          this.nodeTypeSetViaPattern = true;
        });
      } else if (this.nodeType !== 'text' && this.nodeTypeSetViaPattern) {
        const cursorPosition = this.getCursorPosition();

        untrack(() => {
          this.events.nodeTypeConversionDetected({
            nodeId: this.nodeId,
            newNodeType: 'text',
            cleanedContent: content,
            cursorPosition: cursorPosition
          });

          this.nodeType = 'text';
          this.nodeTypeSetViaPattern = false;
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

    public insertNodeReference(nodeId: string): void {
      if (!this.mentionSession) return;

      const content = this.element.value;
      const before = content.substring(0, this.mentionSession.startPosition);
      const after = content.substring(this.getCursorPosition());

      const reference = `[](nodespace://${nodeId})`;
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
  events: TextareaControllerEvents
): TextareaControllerState {
  // Internal mutable state (using $state for Svelte 5 reactivity)
  let controller: TextareaController | null = null;

  // Watch for element changes and manage controller lifecycle
  $effect(() => {
    const element = getElement();
    const nodeId = getNodeId();
    const nodeType = getNodeType();
    const paneId = getPaneId();
    const editableConfig = getEditableConfig();

    untrack(() => {
      if (element && !controller) {
        controller = new TextareaController(
          element,
          nodeId,
          nodeType,
          paneId,
          events,
          editableConfig
        );
      } else if (!element && controller) {
        controller.destroy();
        controller = null;
      }
    });
  });

  // Reactive content sync - replaces manual $effect in base-node.svelte
  $effect(() => {
    const content = getContent();
    if (controller && content !== undefined) {
      controller.updateContent(content);
    }
  });

  // Reactive config sync - replaces manual $effect in base-node.svelte
  $effect(() => {
    const config = getEditableConfig();
    if (controller && config) {
      controller.updateConfig(config);
    }
  });

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

    insertNodeReference(nodeId: string): void {
      controller?.insertNodeReference(nodeId);
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
