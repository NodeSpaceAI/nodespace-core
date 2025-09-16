/**
 * ContentEditableController
 *
 * Pure TypeScript controller for managing dual-representation text editor
 * Separates DOM manipulation from Svelte reactive logic to eliminate race conditions
 */

import ContentProcessor from '$lib/services/contentProcessor';
import type { TriggerContext } from '$lib/services/nodeReferenceService';
import type { SlashCommandContext } from '$lib/services/slashCommandService';
import { markdownToHtml, htmlToMarkdown } from '$lib/utils/markedConfig';

export interface ContentEditableEvents {
  contentChanged: (content: string) => void;
  headerLevelChanged: (level: number) => void;
  focus: () => void;
  blur: () => void;
  createNewNode: (data: {
    afterNodeId: string;
    nodeType: string;
    currentContent?: string;
    newContent?: string;
    cursorAtBeginning?: boolean;
  }) => void;
  indentNode: (data: { nodeId: string }) => void;
  outdentNode: (data: { nodeId: string }) => void;
  navigateArrow: (data: { nodeId: string; direction: 'up' | 'down'; columnHint: number }) => void;
  combineWithPrevious: (data: { nodeId: string; currentContent: string }) => void;
  deleteNode: (data: { nodeId: string }) => void;
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
}

export interface ContentEditableConfig {
  allowMultiline?: boolean;
}

export class ContentEditableController {
  private element: HTMLDivElement;
  private nodeId: string;
  private config: ContentEditableConfig;
  private isEditing: boolean = false;
  private isInitialized: boolean = false;
  private events: ContentEditableEvents;
  private originalContent: string = ''; // Store original markdown content
  private isUpdatingFromInput: boolean = false; // Flag to prevent reactive loops
  private currentHeaderLevel: number = 0; // Track header level for CSS updates

  // Cursor positioning state for precise click-to-edit
  private pendingClickPosition: { x: number; y: number } | null = null;
  private wasEditing: boolean = false;

  // Track recent Shift+Enter to avoid interfering with newlines
  private recentShiftEnter: boolean = false;

  // Track recent Enter to avoid interfering with node creation
  private recentEnter: boolean = false;

  // Bound event handlers for proper cleanup
  private boundHandleFocus = this.handleFocus.bind(this);
  private boundHandleBlur = this.handleBlur.bind(this);
  private boundHandleInput = this.handleInput.bind(this);
  private boundHandleKeyDown = this.handleKeyDown.bind(this);
  private boundHandleMouseDown = this.handleMouseDown.bind(this);

  constructor(
    element: HTMLDivElement,
    nodeId: string,
    events: ContentEditableEvents,
    config: ContentEditableConfig = {}
  ) {
    this.element = element;
    this.nodeId = nodeId;
    this.config = { allowMultiline: false, ...config };
    this.events = events;
    // Mark DOM element as having a controller attached
    (
      this.element as unknown as { _contentEditableController: ContentEditableController }
    )._contentEditableController = this;
    this.setupEventListeners();
  }

  /**
   * Initialize content with dual-representation support
   */
  public initialize(content: string, autoFocus: boolean = false): void {
    if (this.isInitialized) return;

    // Store original markdown content
    this.originalContent = content;

    // Initialize header level tracking
    this.currentHeaderLevel = ContentProcessor.getInstance().parseHeaderLevel(content);

    // Set initial content based on editing state
    if (autoFocus) {
      this.isEditing = true;
      this.setRawMarkdown(content);
      this.focus();

      // Position cursor after header syntax for new header nodes
      if (this.currentHeaderLevel > 0) {
        const headerPrefix = '#'.repeat(this.currentHeaderLevel) + ' ';
        if (content === headerPrefix) {
          // This is a new header node - position cursor at the end
          setTimeout(() => {
            this.restoreCursorPosition(headerPrefix.length);
          }, 0);
        }
      }
    } else {
      this.isEditing = false;
      this.setFormattedContent(content);
    }

    this.isInitialized = true;
  }

  /**
   * Update content without triggering events (for external updates)
   */
  public updateContent(content: string): void {
    // Skip updates during input events to prevent cursor jumping
    if (this.isUpdatingFromInput) {
      return;
    }

    // Skip updates during Enter key handling to prevent cursor jumping
    if (this.recentEnter) {
      return;
    }

    // Prevent reactive loops - don't update if content hasn't changed
    if (this.originalContent === content) {
      return;
    }

    // Also prevent update if the element already has this content
    if (this.isEditing && this.element.textContent === content) {
      this.originalContent = content; // Update stored content but don't touch DOM
      return;
    }

    // Update stored original content
    this.originalContent = content;

    if (this.isEditing) {
      this.setRawMarkdown(content);
    } else {
      this.setFormattedContent(content);
    }
  }

  /**
   * Focus the element programmatically
   */
  public focus(): void {
    // Skip focus during Enter key handling to prevent cursor jumping
    if (this.recentEnter) {
      return;
    }
    this.element.focus();
  }

  /**
   * Check if controller is currently processing input
   * Used to prevent autocomplete from being hidden during DOM manipulation
   */
  public isProcessingInput(): boolean {
    return this.isUpdatingFromInput;
  }

  /**
   * Get current content in markdown format
   */
  public getMarkdownContent(): string {
    if (this.isEditing) {
      return this.element.textContent || '';
    } else {
      return this.htmlToMarkdown(this.element.innerHTML);
    }
  }

  /**
   * Cleanup controller when component is destroyed
   */
  public destroy(): void {
    this.removeEventListeners();
    // Clean up the controller reference from DOM element
    delete (this.element as unknown as { _contentEditableController?: ContentEditableController })
      ._contentEditableController;
  }

  // ============================================================================
  // Private Methods - DOM Manipulation
  // ============================================================================

  private setupEventListeners(): void {
    this.element.addEventListener('focus', this.boundHandleFocus);
    this.element.addEventListener('blur', this.boundHandleBlur);
    this.element.addEventListener('input', this.boundHandleInput);
    this.element.addEventListener('keydown', this.boundHandleKeyDown);
    this.element.addEventListener('mousedown', this.boundHandleMouseDown);
  }

  private removeEventListeners(): void {
    this.element.removeEventListener('focus', this.boundHandleFocus);
    this.element.removeEventListener('blur', this.boundHandleBlur);
    this.element.removeEventListener('input', this.boundHandleInput);
    this.element.removeEventListener('keydown', this.boundHandleKeyDown);
    this.element.removeEventListener('mousedown', this.boundHandleMouseDown);
  }

  private setRawMarkdown(content: string): void {
    // For editing mode, apply live inline formatting while preserving syntax
    if (this.isEditing) {
      this.setLiveFormattedContent(content);
    } else {
      this.element.textContent = content;
    }
  }

  private setLiveFormattedContent(content: string): void {
    // Apply live formatting while preserving markdown syntax for editing
    // Use sequential parser to handle nested patterns correctly
    const escapedContent = this.escapeHtml(content);
    const html = this.markdownToLiveHtml(escapedContent);
    this.element.innerHTML = html;
  }

  /**
   * Convert markdown to HTML with syntax highlighting for editing mode
   * Uses comprehensive parsing to handle all formatting patterns including mixed syntax
   */
  private markdownToLiveHtml(content: string): string {
    // Find all formatting patterns including mixed syntax
    const patterns = this.findAllFormattingPatterns(content);

    if (patterns.length === 0) {
      return content; // No formatting patterns found
    }

    let result = '';
    let lastIndex = 0;

    // Process each pattern in order
    patterns.forEach((pattern) => {
      const { start, end, openMarker, closeMarker, content: innerContent, type } = pattern;

      // Add any text before this pattern
      result += content.substring(lastIndex, start);

      // Build CSS class based on formatting type
      let cssClass = '';
      if (type === 'bold-italic') {
        cssClass = 'markdown-bold markdown-italic';
      } else if (type === 'bold') {
        cssClass = 'markdown-bold';
      } else if (type === 'italic') {
        cssClass = 'markdown-italic';
      }

      // Create the edit-mode format with visible syntax
      result += `<span class="markdown-syntax">${openMarker}<span class="${cssClass}">${innerContent}</span>${closeMarker}</span>`;

      lastIndex = end;
    });

    // Add any remaining text after the last pattern
    result += content.substring(lastIndex);

    return result;
  }

  /**
   * Find all formatting patterns in markdown text including mixed syntax
   * Returns patterns in order of appearance for proper replacement
   */
  private findAllFormattingPatterns(text: string): Array<{
    start: number;
    end: number;
    openMarker: string;
    closeMarker: string;
    content: string;
    type: 'bold-italic' | 'bold' | 'italic';
  }> {
    const patterns: Array<{
      start: number;
      end: number;
      openMarker: string;
      closeMarker: string;
      content: string;
      type: 'bold-italic' | 'bold' | 'italic';
    }> = [];

    // Define all possible formatting patterns in order of precedence
    // Mixed patterns first, then homogeneous patterns
    const formatRules = [
      // Mixed bold-italic patterns (highest precedence)
      {
        regex: /\*__(.*?)__\*/g,
        type: 'bold-italic' as const,
        openMarker: '*__',
        closeMarker: '__*'
      },
      {
        regex: /__\*(.*?)\*__/g,
        type: 'bold-italic' as const,
        openMarker: '__*',
        closeMarker: '*__'
      },
      {
        regex: /\*\*_(.*?)_\*\*/g,
        type: 'bold-italic' as const,
        openMarker: '**_',
        closeMarker: '_**'
      },
      {
        regex: /_\*\*(.*?)\*\*_/g,
        type: 'bold-italic' as const,
        openMarker: '_**',
        closeMarker: '**_'
      },

      // Homogeneous bold-italic patterns
      {
        regex: /\*\*\*(.*?)\*\*\*/g,
        type: 'bold-italic' as const,
        openMarker: '***',
        closeMarker: '***'
      },
      {
        regex: /___(.*?)___/g,
        type: 'bold-italic' as const,
        openMarker: '___',
        closeMarker: '___'
      },

      // Bold patterns (medium precedence)
      { regex: /\*\*([^*]+?)\*\*/g, type: 'bold' as const, openMarker: '**', closeMarker: '**' },
      { regex: /__([^_]+?)__/g, type: 'bold' as const, openMarker: '__', closeMarker: '__' },

      // Italic patterns (lowest precedence)
      {
        regex: /(?<!\*)\*([^*\n]+?)\*(?!\*)/g,
        type: 'italic' as const,
        openMarker: '*',
        closeMarker: '*'
      },
      {
        regex: /(?<!_)_([^_\n]+?)_(?!_)/g,
        type: 'italic' as const,
        openMarker: '_',
        closeMarker: '_'
      }
    ];

    // Find all matches for each pattern type
    formatRules.forEach((rule) => {
      let match;
      while ((match = rule.regex.exec(text)) !== null) {
        const start = match.index;
        const end = match.index + match[0].length;
        const content = match[1];

        // Check for overlaps with existing patterns (skip if overlapping)
        const hasOverlap = patterns.some(
          (existing) => start < existing.end && end > existing.start
        );

        if (!hasOverlap) {
          patterns.push({
            start,
            end,
            openMarker: rule.openMarker,
            closeMarker: rule.closeMarker,
            content,
            type: rule.type
          });
        }
      }

      // Reset regex lastIndex to ensure we find all matches
      rule.regex.lastIndex = 0;
    });

    // Sort patterns by start position for proper processing order
    return patterns.sort((a, b) => a.start - b.start);
  }

  private escapeHtml(text: string): string {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }

  private setFormattedContent(content: string): void {
    const headerLevel = ContentProcessor.getInstance().parseHeaderLevel(content);

    if (headerLevel > 0) {
      // For headers: strip # symbols but preserve inline formatting
      const cleanText = ContentProcessor.getInstance().stripHeaderSyntax(content);
      const htmlContent = this.markdownToHtml(cleanText);
      this.element.innerHTML = htmlContent;
    } else {
      // For non-headers: show formatted HTML (bold, italic, etc.)
      let htmlContent = this.markdownToHtml(content);

      // For multiline nodes: ensure newlines are preserved as <br> tags
      if (this.config.allowMultiline) {
        // Convert any remaining \n characters to <br> tags for display
        htmlContent = htmlContent.replace(/\n/g, '<br>');
      }

      this.element.innerHTML = htmlContent;
    }
  }

  // ============================================================================
  // Private Methods - Content Conversion
  // ============================================================================

  /**
   * Convert markdown to HTML using marked.js library
   * Replaces the previous regex-based parser that had edge case bugs
   */
  private markdownToHtml(markdownContent: string): string {
    return markdownToHtml(markdownContent);
  }

  /**
   * Convert HTML back to markdown using marked.js utilities
   * Replaces the previous regex-based converter
   */
  private htmlToMarkdown(htmlContent: string): string {
    return htmlToMarkdown(htmlContent);
  }

  /**
   * Convert HTML structure created by Shift+Enter to text with newlines
   * Browser creates <div> elements for newlines in contenteditable
   */
  private convertHtmlToTextWithNewlines(html: string): string {
    // Create a temporary element to parse the HTML
    const tempDiv = document.createElement('div');
    tempDiv.innerHTML = html;

    let result = '';

    // Walk through all child nodes
    for (const node of tempDiv.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) {
        // Text node: add the text content
        result += node.textContent || '';
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        const element = node as Element;
        if (element.tagName === 'DIV') {
          // Div element: represents a new line, add newline + content
          result += '\n' + (element.textContent || '');
        } else {
          // Other elements: just add their text content
          result += element.textContent || '';
        }
      }
    }

    return result;
  }

  // ============================================================================
  // Private Methods - Event Handlers
  // ============================================================================

  private handleFocus(): void {
    // Get formatted content before showing raw markdown for position calculation
    const formattedContent = this.element.innerHTML;
    let calculatedMarkdownPosition: number | null = null;

    // Pre-calculate position BEFORE showing syntax (if not already editing)
    if (!this.wasEditing && this.pendingClickPosition) {
      calculatedMarkdownPosition = this.calculateMarkdownPositionFromClick(
        this.pendingClickPosition,
        formattedContent
      );
    }

    this.isEditing = true;

    // Show raw markdown for editing (this changes the DOM)
    this.setRawMarkdown(this.originalContent);

    // Apply pre-calculated position
    if (calculatedMarkdownPosition !== null) {
      setTimeout(() => {
        this.restoreCursorPosition(calculatedMarkdownPosition);
      }, 0);
    }

    // Clear pending position
    this.pendingClickPosition = null;

    this.events.focus();
  }

  private handleBlur(): void {
    this.isEditing = false;
    this.wasEditing = false;

    // On blur: update original content and show formatted display
    let currentText: string;

    if (this.config.allowMultiline) {
      // For multiline nodes: convert HTML structure (with <div> elements) to text with newlines
      currentText = this.convertHtmlToTextWithNewlines(this.element.innerHTML);
    } else {
      // For single-line nodes: use textContent as before
      currentText = this.element.textContent || '';
    }

    this.originalContent = currentText; // Store the edited content
    this.setFormattedContent(currentText);

    this.events.blur();
  }

  private handleInput(): void {
    // Set flag to prevent reactive updates during input processing
    this.isUpdatingFromInput = true;

    if (this.isEditing) {
      // Store cursor position before content changes
      const selection = window.getSelection();
      let cursorOffset = 0;
      if (selection && selection.rangeCount > 0) {
        const range = selection.getRangeAt(0);
        cursorOffset = this.getTextOffsetFromElement(range.startContainer, range.startOffset);
      }

      // When editing (focused), content is plain text - use it directly
      const textContent = this.element.textContent || '';
      this.originalContent = textContent; // Update stored content immediately

      // Check for @ trigger and / slash command detection
      this.checkForTrigger(textContent, cursorOffset);

      // Check for header level changes
      const newHeaderLevel = ContentProcessor.getInstance().parseHeaderLevel(textContent);
      if (newHeaderLevel !== this.currentHeaderLevel) {
        this.currentHeaderLevel = newHeaderLevel;
        this.events.headerLevelChanged(newHeaderLevel);
      }

      // Apply live formatting while preserving cursor, unless we just had a Shift+Enter or regular Enter
      if (!this.recentShiftEnter && !this.recentEnter) {
        this.setLiveFormattedContent(textContent);
        this.restoreCursorPosition(cursorOffset);
      }

      this.events.contentChanged(textContent);
    } else {
      // When not editing (blurred), convert HTML back to markdown
      const htmlContent = this.element.innerHTML || '';
      const markdownContent = this.htmlToMarkdown(htmlContent);
      this.originalContent = markdownContent; // Update stored content immediately
      this.events.contentChanged(markdownContent);
    }

    // Clear flag after a microtask to allow Svelte to process
    Promise.resolve().then(() => {
      this.isUpdatingFromInput = false;
    });
  }

  private handleKeyDown(event: KeyboardEvent): void {
    // Debug logging for Tab and Backspace keys
    if (event.key === 'Tab') {
      // Tab handling logic will follow
    }

    if (event.key === 'Backspace') {
      // Backspace handling logic will follow
    }

    // Handle formatting shortcuts (Cmd+B, Cmd+I)
    if ((event.metaKey || event.ctrlKey) && this.isEditing) {
      if (event.key === 'b' || event.key === 'B') {
        event.preventDefault();
        this.toggleFormatting('**');
        return;
      }
      if (event.key === 'i' || event.key === 'I') {
        event.preventDefault();
        this.toggleFormatting('*');
        return;
      }
    }

    // Check for immediate header detection when space is typed
    if (event.key === ' ' && this.isEditing) {
      // Get content that will exist after this space is added
      const currentText = this.element.textContent || '';
      const futureText = currentText + ' ';

      // Check if this completes a header pattern (e.g., "#" becomes "# ")
      const newHeaderLevel = ContentProcessor.getInstance().parseHeaderLevel(futureText);
      if (newHeaderLevel !== this.currentHeaderLevel) {
        // Use setTimeout to let the space character be added first
        setTimeout(() => {
          this.currentHeaderLevel = newHeaderLevel;
          this.events.headerLevelChanged(newHeaderLevel);
        }, 0);
      }
    }

    // Enter key handling - distinguish between regular Enter and Shift+Enter
    if (event.key === 'Enter') {
      if (event.shiftKey && this.config.allowMultiline) {
        // Shift+Enter for multiline nodes: allow default browser behavior (insert newline)
        // Don't preventDefault() - let the browser handle newline insertion naturally
        // Set flag to prevent live formatting from interfering with the newline
        this.recentShiftEnter = true;
        setTimeout(() => {
          this.recentShiftEnter = false;
        }, 100); // Clear flag after brief delay
        return;
      }

      if (!event.shiftKey) {
        // Regular Enter: create new node with smart text splitting
        event.preventDefault();

        const currentContent = this.element.textContent || '';
        const cursorPosition = this.getCurrentColumn();
        const cursorAtBeginning = cursorPosition === 0;

        // Set flag to prevent cursor restoration during node creation
        this.recentEnter = true;
        setTimeout(() => {
          this.recentEnter = false;
        }, 100); // Clear flag after brief delay

        if (cursorAtBeginning) {
          // Cursor at beginning - create node above
          this.events.createNewNode({
            afterNodeId: this.nodeId,
            nodeType: 'text',
            currentContent: currentContent,
            newContent: '',
            cursorAtBeginning: true
          });
        } else {
          // Normal split with formatting preservation
          const splitResult = this.smartTextSplit(currentContent, cursorPosition);

          this.events.createNewNode({
            afterNodeId: this.nodeId,
            nodeType: 'text',
            currentContent: splitResult.beforeCursor,
            newContent: splitResult.afterCursor,
            cursorAtBeginning: false
          });
        }
        return;
      }
    }

    // Tab key indents node
    if (event.key === 'Tab' && !event.shiftKey) {
      event.preventDefault();
      this.events.indentNode({ nodeId: this.nodeId });
      return;
    }

    // Shift+Tab outdents node
    if (event.key === 'Tab' && event.shiftKey) {
      event.preventDefault();
      this.events.outdentNode({ nodeId: this.nodeId });
      return;
    }

    // Arrow keys for navigation
    if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
      const direction = event.key === 'ArrowUp' ? 'up' : 'down';
      const columnHint = this.getCurrentColumn();
      this.events.navigateArrow({
        nodeId: this.nodeId,
        direction,
        columnHint
      });
      return;
    }

    // Backspace at start of node
    if (event.key === 'Backspace' && this.isAtStart()) {
      event.preventDefault();
      const currentContent = this.element.textContent || '';

      if (currentContent.trim() === '') {
        this.events.deleteNode({ nodeId: this.nodeId });
      } else {
        this.events.combineWithPrevious({
          nodeId: this.nodeId,
          currentContent
        });
      }
      return;
    }
  }

  // ============================================================================
  // Private Methods - Cursor Utilities
  // ============================================================================

  private isAtStart(): boolean {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return false;
    }

    const range = selection.getRangeAt(0);
    return range.startOffset === 0 && range.collapsed;
  }

  private getCurrentColumn(): number {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return 0;

    const range = selection.getRangeAt(0);
    const preCaretRange = range.cloneRange();
    preCaretRange.selectNodeContents(this.element);
    preCaretRange.setEnd(range.startContainer, range.startOffset);

    return preCaretRange.toString().length;
  }

  private getTextOffsetFromElement(node: Node, offset: number): number {
    // Calculate text offset within the contenteditable element
    const walker = document.createTreeWalker(this.element, NodeFilter.SHOW_TEXT, null);

    let textOffset = 0;
    let currentNode;

    while ((currentNode = walker.nextNode())) {
      if (currentNode === node) {
        return textOffset + offset;
      }
      textOffset += currentNode.textContent?.length || 0;
    }

    return textOffset;
  }

  private restoreCursorPosition(textOffset: number): void {
    const selection = window.getSelection();
    if (!selection) return;

    const walker = document.createTreeWalker(this.element, NodeFilter.SHOW_TEXT, null);

    let currentOffset = 0;
    let currentNode;

    while ((currentNode = walker.nextNode())) {
      const nodeLength = currentNode.textContent?.length || 0;

      if (currentOffset + nodeLength >= textOffset) {
        const range = document.createRange();
        const offsetInNode = textOffset - currentOffset;
        range.setStart(currentNode, Math.min(offsetInNode, nodeLength));
        range.setEnd(currentNode, Math.min(offsetInNode, nodeLength));

        selection.removeAllRanges();
        selection.addRange(range);
        return;
      }

      currentOffset += nodeLength;
    }

    // Fallback: place cursor at end
    const range = document.createRange();
    range.selectNodeContents(this.element);
    range.collapse(false);
    selection.removeAllRanges();
    selection.addRange(range);
  }

  private setSelection(startOffset: number, endOffset: number): void {
    const selection = window.getSelection();
    if (!selection) return;

    const walker = document.createTreeWalker(this.element, NodeFilter.SHOW_TEXT, null);

    let currentOffset = 0;
    let startNode: Node | null = null;
    let startNodeOffset = 0;
    let endNode: Node | null = null;
    let endNodeOffset = 0;
    let currentNode;

    while ((currentNode = walker.nextNode())) {
      const nodeLength = currentNode.textContent?.length || 0;

      // Find start position
      if (!startNode && currentOffset + nodeLength >= startOffset) {
        startNode = currentNode;
        startNodeOffset = startOffset - currentOffset;
      }

      // Find end position
      if (!endNode && currentOffset + nodeLength >= endOffset) {
        endNode = currentNode;
        endNodeOffset = endOffset - currentOffset;
        break;
      }

      currentOffset += nodeLength;
    }

    if (startNode && endNode) {
      const range = document.createRange();
      range.setStart(startNode, Math.min(startNodeOffset, startNode.textContent?.length || 0));
      range.setEnd(endNode, Math.min(endNodeOffset, endNode.textContent?.length || 0));

      selection.removeAllRanges();
      selection.addRange(range);
    }
  }

  /**
   * Toggle markdown formatting for selected text or at cursor position
   *
   * ADVANCED NESTED FORMATTING SOLUTION
   * ===================================
   *
   * This implementation solves complex nested markdown formatting scenarios that standard
   * markdown editors struggle with. Key capabilities:
   *
   * 1. **Cross-Marker Toggle**: Cmd+B toggles both ** and __ (bold formatting)
   *    - `__bold__` + Cmd+B → `bold` (removes __ markers)
   *    - `**bold**` + Cmd+B → `bold` (removes ** markers)
   *
   * 2. **Nested Formatting Support**: Handles mixed marker scenarios
   *    - `*__bold__*` + select "bold" + Cmd+B → `*bold*` (removes inner __)
   *    - `**_italic_**` + select "italic" + Cmd+I → `**italic**` (removes inner _)
   *
   * 3. **Sequential Application**: Enables rich formatting combinations
   *    - `**text**` + Cmd+I → `***text***` (adds italic outside bold)
   *    - `***text***` + Cmd+I → `**text**` (removes italic component)
   *
   * 4. **Smart Context Detection**: Analyzes surrounding text to determine action
   *    - Uses `shouldRemoveInnerFormatting()` for nested pattern detection
   *    - Uses `isTextAlreadyFormatted()` for cross-marker compatibility
   *
   * ALGORITHM DESIGN:
   * - Inspired by analysis of Logseq's formatting approach (examined at /Users/malibio/Zed Projects/logseq)
   * - Uses marked.js library for markdown parsing consistency
   * - Implements context-aware selection analysis instead of simple regex matching
   * - Handles double-click selection behavior (includes underscores in word boundaries)
   *
   * Supports: Bold (**,__), Italic (*,_), Sequential nesting, Mixed scenarios
   */
  private toggleFormatting(marker: string): void {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return;

    const range = selection.getRangeAt(0);
    const selectedText = range.toString();
    const textContent = this.element.textContent || '';

    if (selectedText) {
      // Get the actual selection positions using DOM position calculation
      const selectionStartOffset = this.getTextOffsetFromElement(
        range.startContainer,
        range.startOffset
      );
      const selectionEndOffset = this.getTextOffsetFromElement(range.endContainer, range.endOffset);

      // FIXED LOGIC: Check for existing formatting with strict marker type detection
      const formattingState = this.getFormattingState(
        textContent,
        selectionStartOffset,
        selectionEndOffset,
        marker
      );

      let newContent: string = textContent;
      let newSelectionStart: number = selectionStartOffset;
      let newSelectionEnd: number = selectionEndOffset;

      if (formattingState.hasFormatting && formattingState.actualMarker === marker) {
        // TOGGLE OFF: Remove exact marker formatting only

        const beforeFormat = textContent.substring(0, formattingState.formatStart!);
        const insideFormat = textContent.substring(
          formattingState.formatStart! + formattingState.actualMarker.length,
          formattingState.formatEnd!
        );
        const afterFormat = textContent.substring(
          formattingState.formatEnd! + formattingState.actualMarker.length
        );

        newContent = beforeFormat + insideFormat + afterFormat;
        // Adjust selection position to account for removed opening marker
        const markerLength = formattingState.actualMarker.length;
        newSelectionStart = selectionStartOffset - markerLength;
        newSelectionEnd = selectionEndOffset - markerLength;
      } else if (
        this.shouldRemoveInnerFormatting(
          selectedText,
          marker,
          textContent,
          selectionStartOffset,
          selectionEndOffset
        )
      ) {
        // TOGGLE OFF: Remove inner formatting markers from nested scenarios
        // e.g., *__bold__* + Cmd+B should remove __ and leave *bold*
        // e.g., **_italic_** + Cmd+I should remove _ and leave **italic**

        const cleanedText = this.removeInnerFormatting(selectedText, marker);
        const beforeSelection = textContent.substring(0, selectionStartOffset);
        const afterSelection = textContent.substring(selectionEndOffset);

        newContent = beforeSelection + cleanedText + afterSelection;
        newSelectionStart = selectionStartOffset;
        newSelectionEnd = selectionStartOffset + cleanedText.length;
      } else if (formattingState.hasFormatting && formattingState.actualMarker?.startsWith('***')) {
        // TOGGLE OFF: Handle triple asterisk scenarios

        if (formattingState.actualMarker === '***-italic') {
          // Remove italic component from ***text***, leaving **text**
          const beforeFormat = textContent.substring(0, formattingState.formatStart!);
          const insideFormat = textContent.substring(
            formattingState.formatStart! + 3, // Skip ***
            formattingState.formatEnd!
          );
          const afterFormat = textContent.substring(formattingState.formatEnd! + 3); // Skip ***

          newContent = beforeFormat + '**' + insideFormat + '**' + afterFormat;
          newSelectionStart = selectionStartOffset - 1; // One less asterisk
          newSelectionEnd = selectionEndOffset - 1;
        } else if (formattingState.actualMarker === '***-bold') {
          // Remove bold component from ***text***, leaving *text*
          const beforeFormat = textContent.substring(0, formattingState.formatStart!);
          const insideFormat = textContent.substring(
            formattingState.formatStart! + 3, // Skip ***
            formattingState.formatEnd!
          );
          const afterFormat = textContent.substring(formattingState.formatEnd! + 3); // Skip ***

          newContent = beforeFormat + '*' + insideFormat + '*' + afterFormat;
          newSelectionStart = selectionStartOffset - 2; // Two less asterisks
          newSelectionEnd = selectionEndOffset - 2;
        }
      } else {
        // Check if the selected text itself contains the exact formatting markers
        const isFormattedSelection = this.isTextAlreadyFormatted(selectedText, marker);

        if (isFormattedSelection) {
          // TOGGLE OFF: Remove exact formatting from the selected text itself

          const unformattedText = this.removeFormattingFromText(selectedText, marker);
          const beforeSelection = textContent.substring(0, selectionStartOffset);
          const afterSelection = textContent.substring(selectionEndOffset);

          newContent = beforeSelection + unformattedText + afterSelection;
          newSelectionStart = selectionStartOffset;
          newSelectionEnd = selectionStartOffset + unformattedText.length;
        } else {
          // NEST ADD: Add formatting markers around selection (including mixed scenarios)

          // For nesting scenarios like **text** + Cmd+I, we want to create ***text***
          // DO NOT clean conflicting markers - preserve them for nesting
          const beforeSelection = textContent.substring(0, selectionStartOffset);
          const afterSelection = textContent.substring(selectionEndOffset);

          newContent = beforeSelection + marker + selectedText + marker + afterSelection;
          newSelectionStart = selectionStartOffset + marker.length;
          newSelectionEnd = selectionStartOffset + marker.length + selectedText.length;
        }
      }

      // Update content and maintain selection
      this.originalContent = newContent;
      this.setLiveFormattedContent(newContent);
      this.events.contentChanged(newContent);

      // Restore selection on the text content (not the markers)
      setTimeout(() => {
        this.setSelection(newSelectionStart, newSelectionEnd);
      }, 0);
    } else {
      // No selection - check if cursor is inside existing formatting
      const cursorPos = this.getTextOffsetFromElement(range.startContainer, range.startOffset);
      const surroundingFormat = this.findSurroundingFormatting(textContent, cursorPos, marker);

      let newContent: string;
      let newCursorPos: number;

      if (surroundingFormat) {
        // Remove surrounding formatting markers
        const beforeFormat = textContent.substring(0, surroundingFormat.startPos);
        const insideFormat = textContent.substring(
          surroundingFormat.startPos + marker.length,
          surroundingFormat.endPos
        );
        const afterFormat = textContent.substring(surroundingFormat.endPos + marker.length);

        newContent = beforeFormat + insideFormat + afterFormat;
        newCursorPos = cursorPos - marker.length;
      } else {
        // Insert formatting markers at cursor
        const beforeCursor = textContent.substring(0, cursorPos);
        const afterCursor = textContent.substring(cursorPos);

        newContent = beforeCursor + marker + marker + afterCursor;
        newCursorPos = cursorPos + marker.length;
      }

      // Update content and restore cursor
      this.originalContent = newContent;
      this.setLiveFormattedContent(newContent);
      this.events.contentChanged(newContent);
      this.restoreCursorPosition(newCursorPos);
    }
  }

  private findSurroundingFormatting(
    text: string,
    cursorPos: number,
    marker: string
  ): { startPos: number; endPos: number } | null {
    // Look backwards for opening marker
    let startPos = -1;
    for (let i = cursorPos - marker.length; i >= 0; i--) {
      if (text.substring(i, i + marker.length) === marker) {
        startPos = i;
        break;
      }
    }

    if (startPos === -1) return null;

    // Look forwards for closing marker
    let endPos = -1;
    for (let i = cursorPos; i <= text.length - marker.length; i++) {
      if (text.substring(i, i + marker.length) === marker) {
        endPos = i;
        break;
      }
    }

    if (endPos === -1) return null;

    return { startPos, endPos };
  }

  /**
   * Smart text splitting that preserves formatting and header syntax
   * Handles:
   * 1. Header syntax inheritance (# , ## , etc.)
   * 2. Inline formatting preservation (**bold**, *italic*, __underline__)
   * 3. Proper cursor positioning in new node
   */
  private smartTextSplit(
    content: string,
    cursorPosition: number
  ): {
    beforeCursor: string;
    afterCursor: string;
  } {
    // Check if we're in a header node
    const headerLevel = ContentProcessor.getInstance().parseHeaderLevel(content);
    let inheritedSyntax = '';

    if (headerLevel > 0) {
      // Add header syntax to new node
      inheritedSyntax = '#'.repeat(headerLevel) + ' ';
    }

    // Check for inline formatting that needs to be preserved
    const formattingResult = this.preserveInlineFormatting(content, cursorPosition);

    return {
      beforeCursor: formattingResult.beforeCursor,
      afterCursor: inheritedSyntax + formattingResult.afterCursor
    };
  }

  /**
   * Preserve inline formatting when splitting text
   * Ensures all opening markers have matching closing markers
   * For ***__text|more__***, produces: "***__text__***" and "***__more__***"
   */
  private preserveInlineFormatting(
    content: string,
    cursorPosition: number
  ): {
    beforeCursor: string;
    afterCursor: string;
  } {
    const beforeCursor = content.substring(0, cursorPosition);
    const afterCursor = content.substring(cursorPosition);

    // Find all unmatched opening markers that need to be closed
    const unmatchedOpening = this.findUnmatchedOpeningMarkers(beforeCursor);

    // Close the first part with all unmatched opening markers (in reverse order for proper nesting)
    const closingForBefore = unmatchedOpening.slice().reverse().join('');

    // Open the second part with all unmatched opening markers (in original order)
    const openingForAfter = unmatchedOpening.join('');

    const processedBefore = beforeCursor + closingForBefore;
    const processedAfter = openingForAfter + afterCursor;

    return {
      beforeCursor: processedBefore,
      afterCursor: processedAfter
    };
  }

  /**
   * Find all unmatched opening markers that need to be closed
   * Scans the entire beforeCursor text to find all formatting markers that don't have matching closing markers
   */
  private findUnmatchedOpeningMarkers(beforeCursor: string): string[] {
    const openMarkers: Array<{ marker: string; position: number }> = [];
    const closeMarkers: Array<{ marker: string; position: number }> = [];

    // Find all markers in order of appearance
    const markers = ['***', '**', '__', '*'];

    for (let i = 0; i < beforeCursor.length; i++) {
      for (const marker of markers) {
        if (beforeCursor.substring(i, i + marker.length) === marker) {
          // Check if this is an opening or closing marker by counting previous occurrences
          const before = beforeCursor.substring(0, i);
          const count = (before.match(new RegExp(this.escapeRegex(marker), 'g')) || []).length;

          if (count % 2 === 0) {
            // Even count = opening marker
            openMarkers.push({ marker, position: i });
          } else {
            // Odd count = closing marker
            closeMarkers.push({ marker, position: i });
          }

          i += marker.length - 1; // Skip ahead
          break;
        }
      }
    }

    // Find unmatched opening markers
    const unmatchedOpening: string[] = [];
    const usedClosing = new Set<number>();

    // Match opening markers with closing markers (in reverse order for proper nesting)
    for (let i = openMarkers.length - 1; i >= 0; i--) {
      const openMarker = openMarkers[i];
      let matched = false;

      // Find the closest unused closing marker after this opening
      for (let j = 0; j < closeMarkers.length; j++) {
        const closeMarker = closeMarkers[j];
        if (
          !usedClosing.has(j) &&
          closeMarker.marker === openMarker.marker &&
          closeMarker.position > openMarker.position
        ) {
          usedClosing.add(j);
          matched = true;
          break;
        }
      }

      if (!matched) {
        unmatchedOpening.unshift(openMarker.marker); // Add to front to maintain order
      }
    }

    return unmatchedOpening;
  }

  /**
   * Detect triple asterisk formatting scenarios for proper toggle behavior
   * ***text*** + Cmd+I should become **text** (remove italic component)
   * ***text*** + Cmd+B should become *text* (remove bold component)
   */
  private detectTripleAsteriskFormatting(
    text: string,
    selectionStart: number,
    selectionEnd: number,
    marker: string
  ): {
    hasFormatting: boolean;
    formatStart?: number;
    formatEnd?: number;
    actualMarker?: string;
  } {
    // CRITICAL FIX: Only apply to actual *** patterns, not nested ** + _ patterns
    // First, check if the selection is actually within a *** pattern

    // Look for *** patterns around the selection
    const beforeSelection = text.substring(0, selectionStart);
    const afterSelection = text.substring(selectionEnd);

    // Find the closest *** before selection
    let tripleStarStart = -1;
    for (let i = beforeSelection.length - 3; i >= 0; i--) {
      if (beforeSelection.substring(i, i + 3) === '***') {
        // Verify this is likely an opening marker (even count of *** before it)
        const beforeTriple = beforeSelection.substring(0, i);
        const tripleCount = (beforeTriple.match(/\*\*\*/g) || []).length;
        if (tripleCount % 2 === 0) {
          tripleStarStart = i;
          break;
        }
      }
    }

    if (tripleStarStart === -1) {
      return { hasFormatting: false };
    }

    // Find the closest *** after selection
    let tripleStarEnd = -1;
    for (let i = 0; i <= afterSelection.length - 3; i++) {
      if (afterSelection.substring(i, i + 3) === '***') {
        tripleStarEnd = selectionEnd + i;
        break;
      }
    }

    if (tripleStarEnd === -1) {
      return { hasFormatting: false };
    }

    // ADDITIONAL FIX: Verify this is actually a ***text*** pattern, not nested **_text_**
    // Check that the content between markers doesn't have other nested patterns that would conflict
    // Content between triple stars (not used in current logic)
    // const contentBetween = text.substring(tripleStarStart + 3, tripleStarEnd);
    const selectedText = text.substring(selectionStart, selectionEnd);

    // If the selected text contains mixed format markers (**_text_**), this is not a triple asterisk scenario
    if (
      selectedText.includes('**') ||
      selectedText.includes('__') ||
      (selectedText.includes('_') && !selectedText.startsWith('_') && !selectedText.endsWith('_'))
    ) {
      return { hasFormatting: false };
    }

    // We have a ***text*** pattern - determine which component to toggle off
    if (marker === '*') {
      // Cmd+I on ***text*** should remove italic, leaving **text**
      return {
        hasFormatting: true,
        formatStart: tripleStarStart,
        formatEnd: tripleStarEnd,
        actualMarker: '***-italic' // Special marker to indicate triple asterisk italic removal
      };
    } else if (marker === '**') {
      // Cmd+B on ***text*** should remove bold, leaving *text*
      return {
        hasFormatting: true,
        formatStart: tripleStarStart,
        formatEnd: tripleStarEnd,
        actualMarker: '***-bold' // Special marker to indicate triple asterisk bold removal
      };
    }

    return { hasFormatting: false };
  }

  /**
   * Escape special regex characters
   */
  private escapeRegex(string: string): string {
    return string.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  /**
   * Get formatting state for a selection, similar to easy-markdown-editor's getState()
   * Returns whether the selection is currently formatted and where the markers are
   * FIXED: Handles nested formatting properly without interfering with normal operations
   */
  private getFormattingState(
    text: string,
    selectionStart: number,
    selectionEnd: number,
    marker: string
  ): {
    hasFormatting: boolean;
    formatStart?: number;
    formatEnd?: number;
    actualMarker?: string;
  } {
    // Handle triple asterisk patterns ONLY when we actually have *** in the text
    // AND the selection is for an exact *** pattern (not nested patterns like **_text_**)
    if (text.includes('***')) {
      const result = this.detectTripleAsteriskFormatting(
        text,
        selectionStart,
        selectionEnd,
        marker
      );
      if (result.hasFormatting) {
        return result;
      }
    }

    // Define equivalent markers based on the requested marker
    let equivalentMarkers: string[];
    if (marker === '**') {
      equivalentMarkers = ['**', '__']; // Only bold equivalents
    } else if (marker === '*') {
      equivalentMarkers = ['*', '_']; // Only italic equivalents, but exclude ** patterns
    } else {
      equivalentMarkers = [marker];
    }

    // Try each equivalent marker type with strict boundary detection
    for (const testMarker of equivalentMarkers) {
      const result = this.findFormattingBoundariesStrict(
        text,
        selectionStart,
        selectionEnd,
        testMarker
      );
      if (result.hasFormatting) {
        return {
          hasFormatting: true,
          formatStart: result.formatStart,
          formatEnd: result.formatEnd,
          actualMarker: testMarker
        };
      }
    }

    // No formatting detected
    return { hasFormatting: false };
  }

  /**
   * Find formatting boundaries with strict marker type checking
   * FIXED: Prevents ** from being detected as * markers in sequential operations
   */
  private findFormattingBoundariesStrict(
    text: string,
    selectionStart: number,
    selectionEnd: number,
    marker: string
  ): {
    hasFormatting: boolean;
    formatStart?: number;
    formatEnd?: number;
  } {
    // For italic (*), we must exclude positions that are part of bold (**)
    // For bold (**), we must check for exact ** patterns

    const allOpeningPositions: number[] = [];
    const allClosingPositions: number[] = [];

    // Find all valid occurrences of the marker with strict type checking
    for (let i = 0; i <= text.length - marker.length; i++) {
      if (this.isValidMarkerAtPosition(text, i, marker)) {
        // Determine if this is likely an opening or closing marker
        // by checking if we're before or after the selection
        if (i < selectionStart) {
          allOpeningPositions.push(i);
        } else if (i >= selectionEnd) {
          allClosingPositions.push(i);
        }
      }
    }

    // Find the closest opening marker before the selection
    let bestOpeningPos = -1;
    for (let i = allOpeningPositions.length - 1; i >= 0; i--) {
      const pos = allOpeningPositions[i];
      // Check if this could be a valid opening (even count of same markers before it)
      const beforePos = text.substring(0, pos);
      const markerCountBefore = this.countValidMarkersInText(beforePos, marker);

      if (markerCountBefore % 2 === 0) {
        bestOpeningPos = pos;
        break;
      }
    }

    if (bestOpeningPos === -1) {
      return { hasFormatting: false };
    }

    // Find the closest closing marker after the selection
    let bestClosingPos = -1;
    for (const pos of allClosingPositions) {
      // Check if this could be a valid closing (odd total count up to this point)
      const beforeAndAtPos = text.substring(0, pos + marker.length);
      const markerCountTotal = this.countValidMarkersInText(beforeAndAtPos, marker);

      if (markerCountTotal % 2 === 0) {
        // After adding this marker, we have an even count = closing
        bestClosingPos = pos;
        break;
      }
    }

    if (bestClosingPos === -1) {
      return { hasFormatting: false };
    }

    // Verify we have a proper pairing
    const insideText = text.substring(bestOpeningPos + marker.length, bestClosingPos);
    const markerCountInside = this.countValidMarkersInText(insideText, marker);

    // Must have even number of markers inside for a valid pair
    if (markerCountInside % 2 === 0) {
      return {
        hasFormatting: true,
        formatStart: bestOpeningPos,
        formatEnd: bestClosingPos
      };
    }

    return { hasFormatting: false };
  }

  /**
   * Check if a marker at a specific position is valid for the given marker type
   * CRITICAL: Prevents ** from being detected as * markers
   */
  private isValidMarkerAtPosition(text: string, position: number, marker: string): boolean {
    // Check if the text at this position matches the marker
    if (text.substring(position, position + marker.length) !== marker) {
      return false;
    }

    // Special handling for single * to avoid confusion with **
    if (marker === '*') {
      // Make sure this * is not part of a ** pattern
      const charBefore = position > 0 ? text[position - 1] : '';
      const charAfter = position + 1 < text.length ? text[position + 1] : '';

      // Invalid if this * is part of **
      if (charBefore === '*' || charAfter === '*') {
        return false;
      }
    }

    // Special handling for single _ to avoid confusion with __
    if (marker === '_') {
      // Make sure this _ is not part of a __ pattern
      const charBefore = position > 0 ? text[position - 1] : '';
      const charAfter = position + 1 < text.length ? text[position + 1] : '';

      // Invalid if this _ is part of __
      if (charBefore === '_' || charAfter === '_') {
        return false;
      }
    }

    return true;
  }

  /**
   * Count valid markers in text with strict type checking
   */
  private countValidMarkersInText(text: string, marker: string): number {
    let count = 0;
    for (let i = 0; i <= text.length - marker.length; i++) {
      if (this.isValidMarkerAtPosition(text, i, marker)) {
        count++;
      }
    }
    return count;
  }

  /**
   * Find formatting boundaries for a specific marker type (DEPRECATED - using strict version)
   */
  private findFormattingBoundaries(
    text: string,
    selectionStart: number,
    selectionEnd: number,
    marker: string
  ): {
    hasFormatting: boolean;
    formatStart?: number;
    formatEnd?: number;
  } {
    // For nested formatting scenarios, we need to find the closest matching pair
    // For example: **_italic_** - if "**" is requested and "_italic_" is selected,
    // we should find the outer ** markers, not inner ones

    const allOpeningPositions: number[] = [];
    const allClosingPositions: number[] = [];

    // Find all occurrences of the marker
    for (let i = 0; i <= text.length - marker.length; i++) {
      if (text.substring(i, i + marker.length) === marker) {
        // Determine if this is likely an opening or closing marker
        // by checking if we're before or after the selection
        if (i < selectionStart) {
          allOpeningPositions.push(i);
        } else if (i >= selectionEnd) {
          allClosingPositions.push(i);
        }
      }
    }

    // Find the closest opening marker before the selection
    let bestOpeningPos = -1;
    for (let i = allOpeningPositions.length - 1; i >= 0; i--) {
      const pos = allOpeningPositions[i];
      // Check if this could be a valid opening (even count of same markers before it)
      const beforePos = text.substring(0, pos);
      const markerCountBefore = (beforePos.match(new RegExp(this.escapeRegex(marker), 'g')) || [])
        .length;

      if (markerCountBefore % 2 === 0) {
        bestOpeningPos = pos;
        break;
      }
    }

    if (bestOpeningPos === -1) {
      return { hasFormatting: false };
    }

    // Find the closest closing marker after the selection
    let bestClosingPos = -1;
    for (const pos of allClosingPositions) {
      // Check if this could be a valid closing (odd total count up to this point)
      const beforeAndAtPos = text.substring(0, pos + marker.length);
      const markerCountTotal = (
        beforeAndAtPos.match(new RegExp(this.escapeRegex(marker), 'g')) || []
      ).length;

      if (markerCountTotal % 2 === 0) {
        // After adding this marker, we have an even count = closing
        bestClosingPos = pos;
        break;
      }
    }

    if (bestClosingPos === -1) {
      return { hasFormatting: false };
    }

    // Verify we have a proper pairing
    const insideText = text.substring(bestOpeningPos + marker.length, bestClosingPos);
    const markerCountInside = (insideText.match(new RegExp(this.escapeRegex(marker), 'g')) || [])
      .length;

    // Must have even number of markers inside for a valid pair
    if (markerCountInside % 2 === 0) {
      return {
        hasFormatting: true,
        formatStart: bestOpeningPos,
        formatEnd: bestClosingPos
      };
    }

    return { hasFormatting: false };
  }

  /**
   * Check if text is already formatted with the target marker OR equivalent markers
   * FIXED: Handle double-click selection that includes formatting markers
   * When user double-clicks "__bold__", the selection includes underscores and should be toggled off
   */
  private isTextAlreadyFormatted(text: string, targetMarker: string): boolean {
    // CRITICAL FIX: When user double-clicks "__bold__" and presses Cmd+B,
    // the selectedText is "__bold__" and targetMarker is "**"
    // We need to detect that this text is already bold-formatted (with __ equivalent)
    // and should be toggled OFF, not nested

    if (targetMarker === '**') {
      // Bold formatting: check for ** OR __ (equivalent markers when selection includes them)
      return (
        (text.startsWith('**') && text.endsWith('**') && text.length > 4) ||
        (text.startsWith('__') && text.endsWith('__') && text.length > 4)
      );
    } else if (targetMarker === '*') {
      // Italic formatting: check for * OR _ (equivalent markers when selection includes them)
      return (
        (text.startsWith('*') &&
          text.endsWith('*') &&
          text.length > 2 &&
          !text.startsWith('**') &&
          !text.endsWith('**')) ||
        (text.startsWith('_') &&
          text.endsWith('_') &&
          text.length > 2 &&
          !text.startsWith('__') &&
          !text.endsWith('__'))
      );
    } else if (targetMarker === '__') {
      // Bold underscore formatting: check for __ OR ** (equivalent markers when selection includes them)
      return (
        (text.startsWith('__') && text.endsWith('__') && text.length > 4) ||
        (text.startsWith('**') && text.endsWith('**') && text.length > 4)
      );
    } else if (targetMarker === '_') {
      // Italic underscore formatting: check for _ OR * (equivalent markers when selection includes them)
      return (
        (text.startsWith('_') &&
          text.endsWith('_') &&
          text.length > 2 &&
          !text.startsWith('__') &&
          !text.endsWith('__')) ||
        (text.startsWith('*') &&
          text.endsWith('*') &&
          text.length > 2 &&
          !text.startsWith('**') &&
          !text.endsWith('**'))
      );
    }

    return false;
  }

  /**
   * Remove formatting markers from text based on target marker type or equivalent markers
   * FIXED: Handle equivalent markers when removing formatting (for double-click scenarios)
   */
  private removeFormattingFromText(text: string, targetMarker: string): string {
    // Remove formatting markers that correspond to the target format type
    // For bold (targetMarker **): remove ** or __
    // For italic (targetMarker *): remove * or _
    // This handles the double-click scenario where "__bold__" should be unformatted by Cmd+B

    if (targetMarker === '**') {
      // Bold: remove ** or __ (equivalent markers)
      if (text.startsWith('**') && text.endsWith('**') && text.length > 4) {
        return text.substring(2, text.length - 2);
      } else if (text.startsWith('__') && text.endsWith('__') && text.length > 4) {
        return text.substring(2, text.length - 2);
      }
    } else if (targetMarker === '*') {
      // Italic: remove * or _ (equivalent markers, but not ** or __)
      if (
        text.startsWith('*') &&
        text.endsWith('*') &&
        text.length > 2 &&
        !text.startsWith('**') &&
        !text.endsWith('**')
      ) {
        return text.substring(1, text.length - 1);
      } else if (
        text.startsWith('_') &&
        text.endsWith('_') &&
        text.length > 2 &&
        !text.startsWith('__') &&
        !text.endsWith('__')
      ) {
        return text.substring(1, text.length - 1);
      }
    } else if (targetMarker === '__') {
      // Bold underscore: remove __ or ** (equivalent markers)
      if (text.startsWith('__') && text.endsWith('__') && text.length > 4) {
        return text.substring(2, text.length - 2);
      } else if (text.startsWith('**') && text.endsWith('**') && text.length > 4) {
        return text.substring(2, text.length - 2);
      }
    } else if (targetMarker === '_') {
      // Italic underscore: remove _ or * (equivalent markers, but not __ or **)
      if (
        text.startsWith('_') &&
        text.endsWith('_') &&
        text.length > 2 &&
        !text.startsWith('__') &&
        !text.endsWith('__')
      ) {
        return text.substring(1, text.length - 1);
      } else if (
        text.startsWith('*') &&
        text.endsWith('*') &&
        text.length > 2 &&
        !text.startsWith('**') &&
        !text.endsWith('**')
      ) {
        return text.substring(1, text.length - 1);
      }
    }

    return text; // Return original if no formatting found
  }

  /**
   * Clean conflicting markers from text when adding new formatting
   * Based on easy-markdown-editor's approach of removing conflicting markers
   */
  private cleanConflictingMarkers(text: string, targetMarker: string): string {
    let cleanedText = text;

    if (targetMarker === '**') {
      // For bold, remove existing bold markers but keep italic
      cleanedText = cleanedText.replace(/\*\*/g, '');
      cleanedText = cleanedText.replace(/__/g, '');
    } else if (targetMarker === '*') {
      // For italic, remove existing italic markers but keep bold
      // Be careful not to remove ** markers, only single *
      cleanedText = cleanedText.replace(/(?<!\*)\*(?!\*)/g, '');
      cleanedText = cleanedText.replace(/(?<!_)_(?!_)/g, '');
    }

    return cleanedText;
  }

  /**
   * Check if we should remove inner formatting from nested patterns
   * e.g., *__bold__* + Cmd+B should remove inner __ markers
   * e.g., **_italic_** + Cmd+I should remove inner _ markers
   */
  private shouldRemoveInnerFormatting(
    selectedText: string,
    marker: string,
    textContent: string,
    selectionStartOffset: number,
    selectionEndOffset: number
  ): boolean {
    // Look for nested formatting patterns around the current selection
    if (marker === '**') {
      // Cmd+B: Check if selection is inside *__text__* pattern (bold inside italic)
      const beforeSelection = textContent.substring(0, selectionStartOffset);
      const afterSelection = textContent.substring(selectionEndOffset);

      // Check different scenarios:
      // 1. Selection includes markers: "*__bold__*" where selectedText = "__bold__"
      // 2. Selection is just text: "*__bold__*" where selectedText = "bold"

      let isInsideStarUnderscore = false;

      if (selectedText.startsWith('__') && selectedText.endsWith('__')) {
        // Case 1: Selection includes the __ markers, look for * around
        // IMPORTANT: Only consider this inner formatting if there are actually * markers around
        isInsideStarUnderscore = beforeSelection.endsWith('*') && afterSelection.startsWith('*');
      } else {
        // Case 2: Selection is just text, look for *__ before and __* after
        isInsideStarUnderscore =
          !!beforeSelection.match(/\*__[^_]*$/) && !!afterSelection.match(/^[^_]*__\*/);
      }

      // Additional validation: only return true if we're actually in a nested scenario
      // Don't interfere with normal __text__ toggle behavior
      if (selectedText.startsWith('__') && selectedText.endsWith('__') && !isInsideStarUnderscore) {
        // This is standalone __bold__, let normal toggle logic handle it
        return false;
      }

      return isInsideStarUnderscore;
    } else if (marker === '*') {
      // Cmd+I: Check if selection is inside **_text_** pattern (italic inside bold)
      const beforeSelection = textContent.substring(0, selectionStartOffset);
      const afterSelection = textContent.substring(selectionEndOffset);

      let isInsideDoubleStarUnderscore = false;

      if (
        selectedText.startsWith('_') &&
        selectedText.endsWith('_') &&
        !selectedText.startsWith('__')
      ) {
        // Case 1: Selection includes the _ markers (double-click behavior), look for ** around
        isInsideDoubleStarUnderscore =
          beforeSelection.endsWith('**') && afterSelection.startsWith('**');
      } else {
        // Case 2: Selection is just text, look for **_ before and _** after
        isInsideDoubleStarUnderscore =
          !!beforeSelection.match(/\*\*_[^_]*$/) && !!afterSelection.match(/^[^_]*_\*\*/);
      }

      // Additional validation: only return true if we're actually in a nested scenario
      // Don't interfere with normal _text_ toggle behavior
      if (
        selectedText.startsWith('_') &&
        selectedText.endsWith('_') &&
        !selectedText.startsWith('__') &&
        !isInsideDoubleStarUnderscore
      ) {
        // This is standalone _italic_, let normal toggle logic handle it
        return false;
      }

      return isInsideDoubleStarUnderscore;
    }

    return false;
  }

  /**
   * Remove inner formatting markers from nested patterns
   * e.g., *__bold__* → *bold* (remove __)
   * e.g., **_italic_** → **italic** (remove _)
   */
  private removeInnerFormatting(text: string, marker: string): string {
    if (marker === '**') {
      // Remove __ markers while preserving outer * or _
      return text.replace(/__/g, '');
    } else if (marker === '*') {
      // Remove single _ markers while preserving outer **
      // Use negative lookbehind/lookahead to avoid removing _ that are part of __
      return text.replace(/(?<!_)_(?!_)/g, '');
    }
    return text;
  }

  // ============================================================================
  // Private Methods - @ Trigger Detection
  // ============================================================================

  /**
   * Check for @ trigger and / slash commands and emit events for autocomplete modals
   */
  private checkForTrigger(content: string, cursorPosition: number): void {
    const triggerContext = this.detectTrigger(content, cursorPosition);

    if (triggerContext && triggerContext.isValid) {
      // Get cursor position in screen coordinates
      const cursorCoords = this.getCursorScreenPosition();
      if (cursorCoords) {
        this.events.triggerDetected({
          triggerContext,
          cursorPosition: cursorCoords
        });
      }
    } else {
      this.events.triggerHidden();
    }

    // Check for / slash command (only if @ is not active)
    if (!triggerContext || !triggerContext.isValid) {
      const slashContext = this.detectSlashCommand(content, cursorPosition);
      if (slashContext && slashContext.isValid) {
        // Get cursor position in screen coordinates
        const cursorCoords = this.getCursorScreenPosition();
        if (cursorCoords) {
          this.events.slashCommandDetected({
            commandContext: slashContext,
            cursorPosition: cursorCoords
          });
        }
      } else {
        this.events.slashCommandHidden();
      }
    } else {
      // Hide slash commands when @ is active
      this.events.slashCommandHidden();
    }
  }

  /**
   * Detect @ trigger in content at cursor position
   */
  private detectTrigger(content: string, cursorPosition: number): TriggerContext | null {
    // Look backwards from cursor to find @ symbol
    const beforeCursor = content.substring(0, cursorPosition);
    const lastAtIndex = beforeCursor.lastIndexOf('@');

    if (lastAtIndex === -1) {
      return null; // No @ symbol found
    }

    // Check if @ is at word boundary or start of content
    const charBeforeAt = lastAtIndex > 0 ? beforeCursor[lastAtIndex - 1] : ' ';
    if (!/\s/.test(charBeforeAt) && lastAtIndex > 0) {
      return null; // @ is not at word boundary
    }

    // Extract query text between @ and cursor
    const queryText = content.substring(lastAtIndex + 1, cursorPosition);

    // Validate query (no spaces, reasonable length)
    if (queryText.includes(' ') || queryText.includes('\n')) {
      return null; // Query contains invalid characters
    }

    if (queryText.length > 50) {
      return null; // Query too long
    }

    return {
      trigger: '@',
      query: queryText,
      startPosition: lastAtIndex,
      endPosition: cursorPosition,
      element: this.element,
      isValid: true,
      metadata: {}
    };
  }

  /**
   * Get cursor position in screen coordinates for modal positioning
   * Uses consistent positioning: -8px from cursor baseline regardless of text formatting
   */
  private getCursorScreenPosition(): { x: number; y: number } | null {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return null;
    }

    const range = selection.getRangeAt(0);

    // Create a temporary element at the cursor to get consistent positioning
    const tempElement = document.createElement('span');
    tempElement.style.position = 'absolute';
    tempElement.style.visibility = 'hidden';
    tempElement.style.height = '1px';
    tempElement.style.width = '1px';

    // Insert the temp element at cursor position
    range.insertNode(tempElement);

    // Get the position of our temp element (this gives consistent baseline positioning)
    const rect = tempElement.getBoundingClientRect();

    // Clean up the temp element
    tempElement.remove();

    // Normalize selection after cleanup
    selection.collapseToEnd();

    // Add small spacing below cursor for optimal readability
    const verticalSpacing = 4; // 4px below cursor baseline

    return {
      x: rect.left,
      y: rect.top + verticalSpacing // Position with small gap below cursor
    };
  }

  /**
   * Insert node reference at current cursor position
   */
  public insertNodeReference(nodeId: string, nodeTitle: string): void {
    // Allow insertion even when not actively editing since this is programmatic
    // Temporarily switch to editing mode for the insertion
    const wasEditing = this.isEditing;
    if (!wasEditing) {
      this.isEditing = true;
      this.setRawMarkdown(this.originalContent || this.element.textContent || '');
    }

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return;
    }

    const currentContent = this.element.textContent || '';
    const cursorPosition = this.getCurrentColumn();

    // Find the @ trigger that initiated this
    const triggerContext = this.detectTrigger(currentContent, cursorPosition);

    if (!triggerContext) {
      return;
    }

    // Create nodespace URI link
    const nodeReference = `[${nodeTitle}](nodespace://${nodeId})`;

    // Replace the @ trigger and query with the reference
    const beforeTrigger = currentContent.substring(0, triggerContext.startPosition);
    const afterCursor = currentContent.substring(triggerContext.endPosition);
    const newContent = beforeTrigger + nodeReference + afterCursor;

    // Update content and position cursor after the inserted reference
    this.originalContent = newContent;
    this.setLiveFormattedContent(newContent);

    // Position cursor after the inserted reference
    const newCursorPosition = triggerContext.startPosition + nodeReference.length;
    this.restoreCursorPosition(newCursorPosition);

    // Emit content change event
    this.events.contentChanged(newContent);
    this.events.nodeReferenceSelected({ nodeId, nodeTitle });

    // Restore original editing state
    if (!wasEditing) {
      this.isEditing = false;
      // Switch back to formatted display mode
      this.setFormattedContent(newContent);
    }
  }

  // ============================================================================
  // Cursor Positioning System - Pre-calculation Approach
  // ============================================================================

  /**
   * Handle mouse down events to capture click coordinates for cursor positioning
   * Only captures single clicks (preserves double-click selection behavior)
   */
  private handleMouseDown(event: MouseEvent): void {
    // Only capture single clicks (preserve double-click selection)
    if (event.detail !== 1) return;

    // Store current editing state
    this.wasEditing = this.isEditing;

    // Capture click coordinates for later use during focus
    this.pendingClickPosition = { x: event.clientX, y: event.clientY };

    // Let event bubble - don't interfere with other functionality
  }

  /**
   * Calculate markdown cursor position from click coordinates using pre-calculation approach
   */
  private calculateMarkdownPositionFromClick(
    clickCoords: { x: number; y: number },
    formattedContent: string
  ): number | null {
    try {
      // Get character position in display content (without syntax)
      const htmlCharacterPosition = this.getCharacterPositionFromCoordinates(
        clickCoords.x,
        clickCoords.y
      );

      if (htmlCharacterPosition === null) {
        return null;
      }

      // Extract plain text from HTML for mapping
      const htmlText = this.extractTextFromHtml(formattedContent);

      // Map the HTML character position to markdown character position
      const markdownPosition = this.mapHtmlPositionToMarkdown(
        htmlCharacterPosition,
        htmlText,
        this.originalContent
      );

      return markdownPosition;
    } catch (error) {
      console.warn('Error calculating markdown position from click:', error);
      return null;
    }
  }

  /**
   * Get character position from click using browser's native APIs
   */
  private getCharacterPositionFromCoordinates(x: number, y: number): number | null {
    try {
      // Try modern caretPositionFromPoint first (better accuracy)
      if (
        (
          document as unknown as {
            caretPositionFromPoint?: (
              x: number,
              y: number
            ) => { offsetNode: Node; offset: number } | null;
          }
        ).caretPositionFromPoint
      ) {
        const caretPosition = (
          document as unknown as {
            caretPositionFromPoint: (
              x: number,
              y: number
            ) => { offsetNode: Node; offset: number } | null;
          }
        ).caretPositionFromPoint(x, y);
        if (caretPosition && caretPosition.offsetNode) {
          return this.getTextOffsetFromElement(caretPosition.offsetNode, caretPosition.offset);
        }
      }

      // Fallback to caretRangeFromPoint (older but widely supported)
      if (
        (document as unknown as { caretRangeFromPoint?: (x: number, y: number) => Range | null })
          .caretRangeFromPoint
      ) {
        const range = (
          document as unknown as { caretRangeFromPoint: (x: number, y: number) => Range | null }
        ).caretRangeFromPoint(x, y);
        if (range && range.startContainer) {
          return this.getTextOffsetFromElement(range.startContainer, range.startOffset);
        }
      }

      return null;
    } catch (error) {
      console.warn('Error getting character position from coordinates:', error);
      return null;
    }
  }

  /**
   * Map HTML display position to equivalent markdown position
   */
  private mapHtmlPositionToMarkdown(
    htmlPosition: number,
    htmlText: string,
    markdownContent: string
  ): number {
    // Simple case: content matches exactly (no markdown syntax)
    if (htmlText === markdownContent) {
      return Math.min(htmlPosition, markdownContent.length);
    }

    // Build character mapping between HTML and markdown
    const mapping = this.buildCharacterMapping(htmlText, markdownContent);

    // Get mapped position (with bounds checking)
    if (htmlPosition < mapping.length && mapping[htmlPosition] !== undefined) {
      return mapping[htmlPosition];
    }

    // Fallback: proportional positioning
    const ratio = htmlPosition / htmlText.length;
    return Math.floor(ratio * markdownContent.length);
  }

  /**
   * Build character-by-character mapping between HTML display and markdown source
   */
  private buildCharacterMapping(htmlText: string, markdownText: string): number[] {
    const mapping: number[] = [];
    let htmlIndex = 0;
    let markdownIndex = 0;

    while (htmlIndex < htmlText.length && markdownIndex < markdownText.length) {
      if (htmlText[htmlIndex] === markdownText[markdownIndex]) {
        // Characters match - direct mapping
        mapping[htmlIndex] = markdownIndex;
        htmlIndex++;
        markdownIndex++;
      } else {
        // Markdown has extra syntax characters - skip them
        markdownIndex++;
      }
    }

    // Handle remaining HTML characters
    while (htmlIndex < htmlText.length) {
      mapping[htmlIndex] = markdownText.length;
      htmlIndex++;
    }

    return mapping;
  }

  /**
   * Extract plain text from HTML content for position mapping
   */
  private extractTextFromHtml(html: string): string {
    // Create a temporary element to extract text content
    const tempDiv = document.createElement('div');
    tempDiv.innerHTML = html;
    return tempDiv.textContent || '';
  }

  /**
   * Set cursor position to a specific character index
   */
  public setCursorPosition(characterIndex: number): void {
    this.restoreCursorPosition(characterIndex);
  }

  /**
   * Detect / slash command in content at cursor position
   */
  private detectSlashCommand(content: string, cursorPosition: number): SlashCommandContext | null {
    // Look backwards from cursor to find / symbol
    const beforeCursor = content.substring(0, cursorPosition);
    const lastSlashIndex = beforeCursor.lastIndexOf('/');

    if (lastSlashIndex === -1) {
      return null; // No / symbol found
    }

    // Check if / is at line start or after whitespace only
    const lineStart = beforeCursor.lastIndexOf('\n', lastSlashIndex - 1) + 1;
    const textBeforeSlash = beforeCursor.substring(lineStart, lastSlashIndex);

    // Only allow / at line start or after whitespace
    if (textBeforeSlash.trim() !== '') {
      return null; // / is not at line start or after whitespace
    }

    // Extract query text between / and cursor
    const queryText = content.substring(lastSlashIndex + 1, cursorPosition);

    // Validate query (no spaces, reasonable length)
    if (queryText.includes(' ') || queryText.includes('\n')) {
      return null; // Query contains invalid characters
    }
    if (queryText.length > 20) {
      return null; // Query too long for commands
    }

    return {
      trigger: '/',
      query: queryText,
      startPosition: lastSlashIndex,
      endPosition: cursorPosition,
      element: this.element,
      isValid: true,
      metadata: {}
    };
  }

  /**
   * Insert slash command content at current cursor position
   */
  public insertSlashCommand(content: string): void {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return;

    const range = selection.getRangeAt(0);
    const cursorPosition = this.getTextOffsetFromElement(range.startContainer, range.startOffset);
    const currentText = this.element.textContent || '';

    // Find the slash command to replace
    const slashContext = this.detectSlashCommand(currentText, cursorPosition);
    if (!slashContext) return;

    // Replace the /command with the new content
    const beforeSlash = currentText.substring(0, slashContext.startPosition);
    const afterCommand = currentText.substring(slashContext.endPosition);
    const newContent = beforeSlash + content + afterCommand;

    // Update content and position cursor after inserted content
    this.originalContent = newContent;
    this.setLiveFormattedContent(newContent);
    this.events.contentChanged(newContent);

    // Position cursor at end of inserted content
    const newCursorPos = slashContext.startPosition + content.length;
    setTimeout(() => {
      this.restoreCursorPosition(newCursorPos);
    }, 0);
  }
}
