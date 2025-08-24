/**
 * ContentEditableController
 *
 * Pure TypeScript controller for managing dual-representation text editor
 * Separates DOM manipulation from Svelte reactive logic to eliminate race conditions
 */

import ContentProcessor from '$lib/services/contentProcessor.js';
import type { TriggerContext } from '$lib/services/NodeReferenceService.js';

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
    // Mark DOM element as having a controller attached
    (this.element as any)._contentEditableController = this;
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
    // Clean up the controller reference from DOM element
    delete (this.element as any)._contentEditableController;
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
    // Use sequential parser to handle nested patterns correctly
    const escapedContent = this.escapeHtml(content);
    const html = this.markdownToLiveHtml(escapedContent);
    this.element.innerHTML = html;
  }

  /**
   * Convert markdown to HTML with syntax highlighting for editing mode
   * Uses sequential parsing to handle nested patterns correctly
   */
  private markdownToLiveHtml(content: string): string {
    // Standard Markdown formatting parser for editing mode with syntax highlighting
    let result = '';
    let i = 0;

    while (i < content.length) {
      const remaining = content.substring(i);

      // Check for triple underscores (bold + italic) - ___text___
      if (remaining.startsWith('___')) {
        const match = remaining.match(/^___([^_]+)___/);
        if (match) {
          result += `<span class="markdown-syntax">___<span class="markdown-bold markdown-italic">${match[1]}</span>___</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for triple stars (bold + italic) - ***text***
      if (remaining.startsWith('***')) {
        const match = remaining.match(/^\*\*\*([^*]+)\*\*\*/);
        if (match) {
          result += `<span class="markdown-syntax">***<span class="markdown-bold markdown-italic">${match[1]}</span>***</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for double underscores (bold) - __text__
      if (remaining.startsWith('__')) {
        const match = remaining.match(/^__([^_]+)__/);
        if (match) {
          result += `<span class="markdown-syntax">__<span class="markdown-bold">${match[1]}</span>__</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for double stars (bold) - **text**
      if (remaining.startsWith('**')) {
        const match = remaining.match(/^\*\*([^*]+)\*\*/);
        if (match) {
          result += `<span class="markdown-syntax">**<span class="markdown-bold">${match[1]}</span>**</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for single underscores (italic) - _text_
      if (remaining.startsWith('_') && !remaining.startsWith('__')) {
        const match = remaining.match(/^_([^_]+)_/);
        if (match) {
          result += `<span class="markdown-syntax">_<span class="markdown-italic">${match[1]}</span>_</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for single stars (italic) - *text*
      if (remaining.startsWith('*') && !remaining.startsWith('**')) {
        const match = remaining.match(/^\*([^*]+)\*/);
        if (match) {
          result += `<span class="markdown-syntax">*<span class="markdown-italic">${match[1]}</span>*</span>`;
          i += match[0].length;
          continue;
        }
      }

      // If no pattern matches, add the character as-is
      result += content[i];
      i++;
    }

    return result;
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
      const htmlContent = this.markdownToHtml(content);
      this.element.innerHTML = htmlContent;
    }
  }

  // ============================================================================
  // Private Methods - Content Conversion
  // ============================================================================

  private markdownToHtml(markdownContent: string): string {
    // Standard Markdown formatting parser
    let result = '';
    let i = 0;

    while (i < markdownContent.length) {
      const remaining = markdownContent.substring(i);

      // Check for triple underscores (bold + italic) - ___text___
      if (remaining.startsWith('___')) {
        const match = remaining.match(/^___([^_]+)___/);
        if (match) {
          result += `<span class="markdown-bold markdown-italic">${match[1]}</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for triple stars (bold + italic) - ***text***
      if (remaining.startsWith('***')) {
        const match = remaining.match(/^\*\*\*([^*]+)\*\*\*/);
        if (match) {
          result += `<span class="markdown-bold markdown-italic">${match[1]}</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for double underscores (bold) - __text__
      if (remaining.startsWith('__')) {
        const match = remaining.match(/^__([^_]+)__/);
        if (match) {
          result += `<span class="markdown-bold">${match[1]}</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for double stars (bold) - **text**
      if (remaining.startsWith('**')) {
        const match = remaining.match(/^\*\*([^*]+)\*\*/);
        if (match) {
          result += `<span class="markdown-bold">${match[1]}</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for single underscores (italic) - _text_
      if (remaining.startsWith('_') && !remaining.startsWith('__')) {
        const match = remaining.match(/^_([^_]+)_/);
        if (match) {
          result += `<span class="markdown-italic">${match[1]}</span>`;
          i += match[0].length;
          continue;
        }
      }

      // Check for single stars (italic) - *text*
      if (remaining.startsWith('*') && !remaining.startsWith('**')) {
        const match = remaining.match(/^\*([^*]+)\*/);
        if (match) {
          result += `<span class="markdown-italic">${match[1]}</span>`;
          i += match[0].length;
          continue;
        }
      }

      // If no pattern matches, add the character as-is
      result += markdownContent[i];
      i++;
    }

    return result;
  }

  private htmlToMarkdown(htmlContent: string): string {
    let markdown = htmlContent;

    // Convert span classes to markdown syntax
    // Handle most complex combinations first (underline + others) - use single underscores
    markdown = markdown.replace(
      /<span class="markdown-underline markdown-bold markdown-italic">(.*?)<\/span>/g,
      '_***$1***_'
    );
    markdown = markdown.replace(
      /<span class="markdown-underline markdown-italic markdown-bold">(.*?)<\/span>/g,
      '_***$1***_'
    );
    markdown = markdown.replace(
      /<span class="markdown-underline markdown-bold">(.*?)<\/span>/g,
      '_**$1**_'
    );
    markdown = markdown.replace(
      /<span class="markdown-underline markdown-italic">(.*?)<\/span>/g,
      '_*$1*_'
    );

    // Handle remaining bold + italic combinations
    markdown = markdown.replace(
      /<span class="markdown-bold markdown-italic">(.*?)<\/span>/g,
      '***$1***'
    );
    markdown = markdown.replace(
      /<span class="markdown-italic markdown-bold">(.*?)<\/span>/g,
      '***$1***'
    );

    // Handle individual formatting
    markdown = markdown.replace(/<span class="markdown-bold">(.*?)<\/span>/g, '**$1**');
    markdown = markdown.replace(/<span class="markdown-italic">(.*?)<\/span>/g, '*$1*');
    markdown = markdown.replace(/<span class="markdown-underline">(.*?)<\/span>/g, '_$1_');

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

      // Check for @ trigger detection
      this.checkForTrigger(textContent, cursorOffset);

      // Check for header level changes
      const newHeaderLevel = ContentProcessor.getInstance().parseHeaderLevel(textContent);
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
    // Debug logging for Tab and Backspace keys
    if (event.key === 'Tab') {
      console.log('🔍 ContentEditableController Tab event:', {
        key: event.key,
        shiftKey: event.shiftKey,
        nodeId: this.nodeId,
        isEditing: this.isEditing,
        elementId: this.element.id
      });
    }
    
    if (event.key === 'Backspace') {
      console.log('🔍 ContentEditableController Backspace event:', {
        key: event.key,
        nodeId: this.nodeId,
        isEditing: this.isEditing,
        isAtStart: this.isAtStart(),
        currentContent: this.element.textContent || ''
      });
    }

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

    // Enter key creates new node with smart text splitting
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();

      const currentContent = this.element.textContent || '';
      const cursorPosition = this.getCurrentColumn();
      const cursorAtBeginning = cursorPosition === 0;

      if (cursorAtBeginning) {
        // SOPHISTICATED LOGIC: Cursor at beginning - create node above
        this.events.createNewNode({
          afterNodeId: this.nodeId,
          nodeType: 'text',
          currentContent: currentContent, // Keep all content in original node
          newContent: '', // Empty new node above
          cursorAtBeginning: true
        });
      } else {
        // SOPHISTICATED LOGIC: Normal split with formatting preservation
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

    // Tab key indents node
    if (event.key === 'Tab' && !event.shiftKey) {
      event.preventDefault();
      console.log('🔍 Firing indentNode event for nodeId:', this.nodeId);
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
      console.log('🔍 Backspace at start detected:', { nodeId: this.nodeId, currentContent, isEmpty: currentContent.trim() === '' });

      if (currentContent.trim() === '') {
        console.log('🔍 Empty node - calling deleteNode');
        this.events.deleteNode({ nodeId: this.nodeId });
      } else {
        console.log('🔍 Non-empty node - calling combineWithPrevious');
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
      // Handle case where selection includes formatting markers (like double-clicking "__word__")
      let adjustedSelectedText = selectedText;
      let markerIncludedInSelection = false;
      let detectedMarker = marker; // Track which marker was actually detected

      // Check for all possible markers based on the requested format type
      let possibleMarkers: string[] = [];
      if (marker === '*') {
        possibleMarkers = ['*', '_'];
      } else if (marker === '**') {
        possibleMarkers = ['**', '__'];
      } else if (marker === '***') {
        possibleMarkers = ['***', '___'];
      } else {
        possibleMarkers = [marker];
      }

      // Check if selection includes any of the possible markers
      for (const possibleMarker of possibleMarkers) {
        if (
          selectedText.startsWith(possibleMarker) &&
          selectedText.endsWith(possibleMarker) &&
          selectedText.length > possibleMarker.length * 2
        ) {
          // Selection includes the markers - extract just the content
          adjustedSelectedText = selectedText.substring(
            possibleMarker.length,
            selectedText.length - possibleMarker.length
          );
          markerIncludedInSelection = true;
          detectedMarker = possibleMarker; // Remember which marker was found
          break;
        }
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

      // Check if already formatted by looking for surrounding markers (handles nested formatting)
      const isAlreadyFormatted = this.isSelectionAlreadyFormatted(textContent, actualStart, actualEnd, marker);

      let newContent: string;
      let newSelectionStart: number;
      let newSelectionEnd: number;

      if (isAlreadyFormatted || markerIncludedInSelection) {
        // Remove formatting - find the actual markers present around the selection
        const actualMarkers = this.findActualMarkersAroundSelection(textContent, actualStart, actualEnd, marker);
        if (actualMarkers) {
          // Remove the detected markers
          const beforeMarker = textContent.substring(0, actualMarkers.startPos);
          const afterMarker = textContent.substring(actualMarkers.endPos + actualMarkers.marker.length);
          newContent = beforeMarker + workingSelectedText + afterMarker;
          newSelectionStart = beforeMarker.length;
          newSelectionEnd = newSelectionStart + workingSelectedText.length;
        } else {
          // Fallback to original logic - use detected marker when available
          const markerToRemove = markerIncludedInSelection ? detectedMarker : marker;
          const beforeMarker = beforeSelection.endsWith(markerToRemove)
            ? beforeSelection.substring(0, beforeSelection.length - markerToRemove.length)
            : beforeSelection;
          const afterMarker = afterSelection.startsWith(markerToRemove)
            ? afterSelection.substring(markerToRemove.length)
            : afterSelection;
          newContent = beforeMarker + workingSelectedText + afterMarker;
          newSelectionStart = beforeMarker.length;
          newSelectionEnd = newSelectionStart + workingSelectedText.length;
        }
      } else {
        // Add formatting - check for existing formatting layers to position markers correctly
        const analysis = this.analyzeFormattingLayers(textContent, actualStart, actualEnd);
        
        if (analysis.layers.length > 0) {
          // There's existing formatting - add new formatting outside the outermost layer
          const outermostLayer = analysis.layers.reduce((outermost, current) => {
            if (current.startPos <= outermost.startPos && current.endPos >= outermost.endPos) {
              return current;
            }
            return outermost;
          });
          
          // Place new markers outside the outermost existing formatting
          const beforeOuterFormat = textContent.substring(0, outermostLayer.startPos);
          const existingFormat = textContent.substring(outermostLayer.startPos, outermostLayer.endPos + outermostLayer.marker.length);
          const afterOuterFormat = textContent.substring(outermostLayer.endPos + outermostLayer.marker.length);
          
          newContent = beforeOuterFormat + marker + existingFormat + marker + afterOuterFormat;
          newSelectionStart = beforeOuterFormat.length + marker.length + (actualStart - outermostLayer.startPos);
          newSelectionEnd = beforeOuterFormat.length + marker.length + (actualEnd - outermostLayer.startPos);
        } else {
          // No existing formatting - add markers normally
          newContent = beforeSelection + marker + workingSelectedText + marker + afterSelection;
          newSelectionStart = beforeSelection.length + marker.length;
          newSelectionEnd = newSelectionStart + workingSelectedText.length;
        }
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
   * Get active formatting markers at a specific position
   * Returns formatting that is "open" at the cursor position
   */
  private getActiveFormattingAtPosition(
    content: string,
    position: number
  ): Array<{
    marker: string;
    type: 'bold' | 'italic' | 'underline';
  }> {
    const activeFormatting: Array<{ marker: string; type: 'bold' | 'italic' | 'underline' }> = [];

    // Check for bold (**text**)
    if (this.isInsideFormatting(content, position, '**')) {
      activeFormatting.push({ marker: '**', type: 'bold' });
    }

    // Check for italic (*text*) - but not if already inside bold
    if (
      !this.isInsideFormatting(content, position, '**') &&
      this.isInsideFormatting(content, position, '*')
    ) {
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
    const beforeCount = (beforeCursor.match(new RegExp(this.escapeRegex(marker), 'g')) || [])
      .length;
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

  /**
   * Find the specific formatting layer to remove based on the requested marker type
   * Uses the new formatting analysis to handle complex mixed scenarios
   */
  private findActualMarkersAroundSelection(text: string, start: number, end: number, requestedMarker: string): { startPos: number; endPos: number; marker: string } | null {
    const analysis = this.analyzeFormattingLayers(text, start, end);
    
    // Find the specific layer type to remove
    let targetType: 'italic' | 'bold' | 'bold-italic';
    if (requestedMarker === '*') targetType = 'italic';
    else if (requestedMarker === '**') targetType = 'bold';  
    else if (requestedMarker === '***') targetType = 'bold-italic';
    else return null;

    // Find the first layer of the target type
    const layerToRemove = analysis.layers.find(layer => layer.type === targetType);
    if (layerToRemove) {
      return {
        startPos: layerToRemove.startPos,
        endPos: layerToRemove.endPos,
        marker: layerToRemove.marker
      };
    }

    return null;
  }

  /**
   * Analyze all formatting layers around a selection
   * Returns information about existing formatting that can be used for smart toggle decisions
   */
  private analyzeFormattingLayers(text: string, start: number, end: number): {
    hasItalic: boolean;
    hasBold: boolean;
    hasBoldItalic: boolean;
    layers: { type: 'italic' | 'bold' | 'bold-italic'; startPos: number; endPos: number; marker: string }[];
  } {
    const layers: { type: 'italic' | 'bold' | 'bold-italic'; startPos: number; endPos: number; marker: string }[] = [];
    let hasItalic = false;
    let hasBold = false;
    let hasBoldItalic = false;

    // Define all possible markers and their types
    const markerTypes: { marker: string; type: 'italic' | 'bold' | 'bold-italic'; priority: number }[] = [
      { marker: '___', type: 'bold-italic', priority: 3 },
      { marker: '***', type: 'bold-italic', priority: 3 },
      { marker: '__', type: 'bold', priority: 2 },
      { marker: '**', type: 'bold', priority: 2 },
      { marker: '_', type: 'italic', priority: 1 },
      { marker: '*', type: 'italic', priority: 1 },
    ];

    // Sort by priority (longest markers first to avoid conflicts)
    markerTypes.sort((a, b) => b.priority - a.priority);

    for (const { marker, type } of markerTypes) {
      // Look backwards from start position to find opening marker
      let openingPos = -1;
      for (let i = start - marker.length; i >= 0; i--) {
        if (text.substring(i, i + marker.length) === marker) {
          openingPos = i;
          break;
        }
      }

      // Look forwards from end position to find closing marker
      let closingPos = -1;
      for (let i = end; i <= text.length - marker.length; i++) {
        if (text.substring(i, i + marker.length) === marker) {
          closingPos = i;
          break;
        }
      }

      // If we found both markers, record this layer
      if (openingPos !== -1 && closingPos !== -1) {
        layers.push({ type, startPos: openingPos, endPos: closingPos, marker });
        
        if (type === 'italic') hasItalic = true;
        if (type === 'bold') hasBold = true;
        if (type === 'bold-italic') hasBoldItalic = true;
      }
    }

    return { hasItalic, hasBold, hasBoldItalic, layers };
  }

  /**
   * Check if a selection is already formatted with the given marker type
   * Uses the new formatting layer analysis for better mixed-marker support
   */
  private isSelectionAlreadyFormatted(text: string, start: number, end: number, marker: string): boolean {
    const analysis = this.analyzeFormattingLayers(text, start, end);
    
    // Map marker to format type
    if (marker === '*') return analysis.hasItalic;
    if (marker === '**') return analysis.hasBold;
    if (marker === '***') return analysis.hasBoldItalic;
    
    return false;
  }


  // ============================================================================
  // Private Methods - @ Trigger Detection
  // ============================================================================

  /**
   * Check for @ trigger and emit events for autocomplete modal
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
   */
  private getCursorScreenPosition(): { x: number; y: number } | null {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return null;
    }

    const range = selection.getRangeAt(0);
    const rect = range.getBoundingClientRect();

    return {
      x: rect.left,
      y: rect.bottom + 5 // Position modal slightly below cursor
    };
  }

  /**
   * Insert node reference at current cursor position
   */
  public insertNodeReference(nodeId: string, nodeTitle: string): void {
    if (!this.isEditing) {
      return;
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
  }
}
