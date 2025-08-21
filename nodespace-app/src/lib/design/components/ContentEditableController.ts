/**
 * ContentEditableController
 *
 * Pure TypeScript controller for managing dual-representation text editor
 * Separates DOM manipulation from Svelte reactive logic to eliminate race conditions
 */

import { contentProcessor } from '$lib/services/contentProcessor.js';

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
  }) => void;
  indentNode: (data: { nodeId: string }) => void;
  outdentNode: (data: { nodeId: string }) => void;
  navigateArrow: (data: { nodeId: string; direction: 'up' | 'down'; columnHint: number }) => void;
  combineWithPrevious: (data: { nodeId: string; currentContent: string }) => void;
  deleteNode: (data: { nodeId: string }) => void;
}

export class ContentEditableController {
  private element: HTMLDivElement;
  private nodeId: string;
  private isEditing: boolean = false;
  private isInitialized: boolean = false;
  private events: ContentEditableEvents;
  private originalContent: string = ''; // Store original markdown content
  private isUpdatingFromInput: boolean = false; // Flag to prevent reactive loops
  private currentHeaderLevel: number = 0; // Track header level for CSS updates

  // Bound event handlers for proper cleanup
  private boundHandleFocus = this.handleFocus.bind(this);
  private boundHandleBlur = this.handleBlur.bind(this);
  private boundHandleInput = this.handleInput.bind(this);
  private boundHandleKeyDown = this.handleKeyDown.bind(this);

  constructor(element: HTMLDivElement, nodeId: string, events: ContentEditableEvents) {
    this.element = element;
    this.nodeId = nodeId;
    this.events = events;
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
    this.currentHeaderLevel = contentProcessor.parseHeaderLevel(content);

    // Set initial content based on editing state
    if (autoFocus) {
      this.isEditing = true;
      this.setRawMarkdown(content);
      this.focus();
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
    this.element.focus();
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
  }

  // ============================================================================
  // Private Methods - DOM Manipulation
  // ============================================================================

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
    let html = this.escapeHtml(content);

    // Apply live formatting with consistent structure to fix double-click selection
    // Wrap entire formatted text (including syntax) in spans for consistent selection behavior
    html = html.replace(
      /\*\*([^*]+)\*\*/g,
      '<span class="markdown-syntax">**<span class="markdown-bold">$1</span>**</span>'
    );
    html = html.replace(
      /(?<!\*)\*([^*]+)\*(?!\*)/g,
      '<span class="markdown-syntax">*<span class="markdown-italic">$1</span>*</span>'
    );
    html = html.replace(
      /__([^_]+)__/g,
      '<span class="markdown-syntax">__<span class="markdown-underline">$1</span>__</span>'
    );

    this.element.innerHTML = html;
  }

  private escapeHtml(text: string): string {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }

  private setFormattedContent(content: string): void {
    const headerLevel = contentProcessor.parseHeaderLevel(content);

    if (headerLevel > 0) {
      // For headers: strip # symbols but preserve inline formatting
      const cleanText = contentProcessor.stripHeaderSyntax(content);
      const htmlContent = this.markdownToHtml(cleanText);
      this.element.innerHTML = htmlContent;
    } else {
      // For non-headers: show formatted HTML (bold, italic, etc.)
      const htmlContent = this.markdownToHtml(content);
      this.element.innerHTML = htmlContent;
    }
  }

  // ============================================================================
  // Private Methods - Content Conversion
  // ============================================================================

  private markdownToHtml(markdownContent: string): string {
    let html = markdownContent;

    // Convert inline formatting only
    html = html.replace(/\*\*(.*?)\*\*/g, '<span class="markdown-bold">$1</span>');
    html = html.replace(/(?<!\*)\*([^*]+)\*(?!\*)/g, '<span class="markdown-italic">$1</span>');
    html = html.replace(/__(.*?)__/g, '<span class="markdown-underline">$1</span>');

    return html;
  }

  private htmlToMarkdown(htmlContent: string): string {
    let markdown = htmlContent;

    // Convert span classes to markdown syntax
    markdown = markdown.replace(/<span class="markdown-bold">(.*?)<\/span>/g, '**$1**');
    markdown = markdown.replace(/<span class="markdown-italic">(.*?)<\/span>/g, '*$1*');
    markdown = markdown.replace(/<span class="markdown-underline">(.*?)<\/span>/g, '__$1__');

    // Clean up any remaining HTML tags
    markdown = markdown.replace(/<[^>]*>/g, '');

    return markdown;
  }

  // ============================================================================
  // Private Methods - Event Handlers
  // ============================================================================

  private handleFocus(): void {
    this.isEditing = true;

    // On focus: show raw markdown for editing (use stored original content)
    this.setRawMarkdown(this.originalContent);

    this.events.focus();
  }

  private handleBlur(): void {
    this.isEditing = false;

    // On blur: update original content and show formatted display
    const currentText = this.element.textContent || '';
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

      // Check for header level changes
      const newHeaderLevel = contentProcessor.parseHeaderLevel(textContent);
      if (newHeaderLevel !== this.currentHeaderLevel) {
        this.currentHeaderLevel = newHeaderLevel;
        this.events.headerLevelChanged(newHeaderLevel);
      }

      // Apply live formatting while preserving cursor
      this.setLiveFormattedContent(textContent);
      this.restoreCursorPosition(cursorOffset);

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
    // Handle formatting shortcuts (Cmd+B, Cmd+I, Cmd+U)
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
      if (event.key === 'u' || event.key === 'U') {
        event.preventDefault();
        this.toggleFormatting('__');
        return;
      }
    }

    // Check for immediate header detection when space is typed
    if (event.key === ' ' && this.isEditing) {
      // Get content that will exist after this space is added
      const currentText = this.element.textContent || '';
      const futureText = currentText + ' ';

      // Check if this completes a header pattern (e.g., "#" becomes "# ")
      const newHeaderLevel = contentProcessor.parseHeaderLevel(futureText);
      if (newHeaderLevel !== this.currentHeaderLevel) {
        // Use setTimeout to let the space character be added first
        setTimeout(() => {
          this.currentHeaderLevel = newHeaderLevel;
          this.events.headerLevelChanged(newHeaderLevel);
        }, 0);
      }
    }

    // Enter key creates new node with smart text splitting
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();

      const currentContent = this.element.textContent || '';
      const cursorPosition = this.getCurrentColumn();
      
      // Perform smart split that preserves formatting
      const splitResult = this.smartTextSplit(currentContent, cursorPosition);

      this.events.createNewNode({
        afterNodeId: this.nodeId,
        nodeType: 'text',
        currentContent: splitResult.beforeCursor,
        newContent: splitResult.afterCursor
      });
      return;
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

  private toggleFormatting(marker: string): void {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return;

    const range = selection.getRangeAt(0);
    const selectedText = range.toString();
    const textContent = this.element.textContent || '';

    // Simple approach: work with the markdown text content directly
    if (selectedText) {
      // Handle case where selection includes the formatting markers (like double-clicking "__word__")
      let adjustedSelectedText = selectedText;
      let markerIncludedInSelection = false;

      if (
        selectedText.startsWith(marker) &&
        selectedText.endsWith(marker) &&
        selectedText.length > marker.length * 2
      ) {
        // Selection includes the markers - extract just the content
        adjustedSelectedText = selectedText.substring(
          marker.length,
          selectedText.length - marker.length
        );
        markerIncludedInSelection = true;
      }

      // Find the selected text in the markdown content
      const searchText = markerIncludedInSelection ? selectedText : adjustedSelectedText;
      const selectionStart = textContent.indexOf(searchText);
      if (selectionStart === -1) return; // Selection not found in text content

      let actualStart: number;
      let actualEnd: number;
      let workingSelectedText: string;

      if (markerIncludedInSelection) {
        // Selection includes markers - we want to work with just the content inside
        actualStart = selectionStart + marker.length;
        actualEnd = actualStart + adjustedSelectedText.length;
        workingSelectedText = adjustedSelectedText;
      } else {
        // Selection is just the content
        actualStart = selectionStart;
        actualEnd = selectionStart + adjustedSelectedText.length;
        workingSelectedText = adjustedSelectedText;
      }

      const beforeSelection = textContent.substring(0, actualStart);
      const afterSelection = textContent.substring(actualEnd);

      // Check if already formatted by looking at surrounding text
      const isAlreadyFormatted =
        beforeSelection.endsWith(marker) && afterSelection.startsWith(marker);

      let newContent: string;
      let newSelectionStart: number;
      let newSelectionEnd: number;

      if (isAlreadyFormatted || markerIncludedInSelection) {
        // Remove formatting
        const beforeMarker = beforeSelection.endsWith(marker)
          ? beforeSelection.substring(0, beforeSelection.length - marker.length)
          : beforeSelection;
        const afterMarker = afterSelection.startsWith(marker)
          ? afterSelection.substring(marker.length)
          : afterSelection;
        newContent = beforeMarker + workingSelectedText + afterMarker;
        newSelectionStart = beforeMarker.length;
        newSelectionEnd = newSelectionStart + workingSelectedText.length;
      } else {
        // Add formatting
        newContent = beforeSelection + marker + workingSelectedText + marker + afterSelection;
        newSelectionStart = beforeSelection.length + marker.length;
        newSelectionEnd = newSelectionStart + workingSelectedText.length;
      }

      // Update content and maintain selection
      this.originalContent = newContent;
      this.setLiveFormattedContent(newContent);
      this.events.contentChanged(newContent);

      // Restore selection on the text (not the markers)
      setTimeout(() => {
        this.setSelection(newSelectionStart, newSelectionEnd);
      }, 0);
    } else {
      // No selection - insert markers at cursor position
      const cursorPos = this.getTextOffsetFromElement(range.startContainer, range.startOffset);
      const beforeCursor = textContent.substring(0, cursorPos);
      const afterCursor = textContent.substring(cursorPos);

      // Check if cursor is inside existing formatting
      const surroundingMarkers = this.findSurroundingFormatting(textContent, cursorPos, marker);

      let newContent: string;
      let newCursorPos: number;

      if (surroundingMarkers) {
        // Remove surrounding formatting
        const beforeFormatting = textContent.substring(0, surroundingMarkers.startPos);
        const insideFormatting = textContent.substring(
          surroundingMarkers.startPos + marker.length,
          surroundingMarkers.endPos
        );
        const afterFormatting = textContent.substring(surroundingMarkers.endPos + marker.length);

        newContent = beforeFormatting + insideFormatting + afterFormatting;
        newCursorPos = cursorPos - marker.length;
      } else {
        // Insert formatting markers
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
  private smartTextSplit(content: string, cursorPosition: number): {
    beforeCursor: string;
    afterCursor: string;
  } {
    // Check if we're in a header node
    const headerLevel = contentProcessor.parseHeaderLevel(content);
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
   * If cursor is inside **bold text|more bold**, should produce:
   * - beforeCursor: "**bold text**"
   * - afterCursor: "**more bold**" 
   */
  private preserveInlineFormatting(content: string, cursorPosition: number): {
    beforeCursor: string;
    afterCursor: string;
  } {
    const beforeCursor = content.substring(0, cursorPosition);
    const afterCursor = content.substring(cursorPosition);

    // Track active formatting at cursor position
    const activeFormatting = this.getActiveFormattingAtPosition(content, cursorPosition);

    let processedBefore = beforeCursor;
    let processedAfter = afterCursor;

    // For each active formatting, close in before and reopen in after
    for (const formatting of activeFormatting) {
      processedBefore += formatting.marker;
      processedAfter = formatting.marker + processedAfter;
    }

    return {
      beforeCursor: processedBefore,
      afterCursor: processedAfter
    };
  }

  /**
   * Get active formatting markers at a specific position
   * Returns formatting that is "open" at the cursor position
   */
  private getActiveFormattingAtPosition(content: string, position: number): Array<{
    marker: string;
    type: 'bold' | 'italic' | 'underline';
  }> {
    const activeFormatting: Array<{ marker: string; type: 'bold' | 'italic' | 'underline' }> = [];
    
    // Check for bold (**text**)
    if (this.isInsideFormatting(content, position, '**')) {
      activeFormatting.push({ marker: '**', type: 'bold' });
    }
    
    // Check for italic (*text*) - but not if already inside bold
    if (!this.isInsideFormatting(content, position, '**') && this.isInsideFormatting(content, position, '*')) {
      activeFormatting.push({ marker: '*', type: 'italic' });
    }
    
    // Check for underline (__text__)
    if (this.isInsideFormatting(content, position, '__')) {
      activeFormatting.push({ marker: '__', type: 'underline' });
    }

    return activeFormatting;
  }

  /**
   * Check if position is inside a specific formatting marker
   */
  private isInsideFormatting(content: string, position: number, marker: string): boolean {
    const beforeCursor = content.substring(0, position);
    const afterCursor = content.substring(position);

    // Count occurrences of marker before and after cursor
    const beforeCount = (beforeCursor.match(new RegExp(this.escapeRegex(marker), 'g')) || []).length;
    const afterCount = (afterCursor.match(new RegExp(this.escapeRegex(marker), 'g')) || []).length;

    // If we have an odd number before cursor and at least one after, we're inside
    return beforeCount % 2 === 1 && afterCount > 0;
  }

  /**
   * Escape special regex characters
   */
  private escapeRegex(string: string): string {
    return string.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
}
