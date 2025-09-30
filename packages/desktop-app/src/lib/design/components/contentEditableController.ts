/**
 * ContentEditableController
 *
 * Pure TypeScript controller for managing dual-representation text editor
 * Separates DOM manipulation from Svelte reactive logic to eliminate race conditions
 */

import ContentProcessor from '$lib/services/contentProcessor';
import type { TriggerContext } from '$lib/services/nodeReferenceService';
import type { SlashCommandContext } from '$lib/services/slashCommandService';
import { SlashCommandService } from '$lib/services/slashCommandService';
import { splitMarkdownContent } from '$lib/utils/markdownSplitter';
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
    originalContent?: string;
    cursorAtBeginning?: boolean;
    insertAtBeginning?: boolean;
    focusOriginalNode?: boolean;
  }) => void;
  indentNode: (data: { nodeId: string }) => void;
  directSlashCommand: (data: {
    command: string;
    nodeType: string;
    cursorPosition?: number;
  }) => void;
  outdentNode: (data: { nodeId: string }) => void;
  navigateArrow: (data: { nodeId: string; direction: 'up' | 'down'; pixelOffset: number }) => void;
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
  // Node Type Conversion Events
  nodeTypeConversionDetected: (data: {
    nodeId: string;
    newNodeType: string;
    cleanedContent: string;
  }) => void;
}

export interface ContentEditableConfig {
  allowMultiline?: boolean;
}

export class ContentEditableController {
  private element: HTMLDivElement;
  private nodeId: string;
  private nodeType: string;
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

  // Track if slash command dropdown is currently active
  private slashCommandDropdownActive: boolean = false;
  // Track if autocomplete dropdown is currently active
  private autocompleteDropdownActive: boolean = false;
  // Prevent pattern detection loops after conversion
  private skipPatternDetection: boolean = false;

  // Slash command session tracking
  private slashCommandSession: {
    active: boolean;
    startPosition: number;
    query: string;
    originalContent: string;
  } | null = null;

  // Bound event handlers for proper cleanup
  private boundHandleFocus = this.handleFocus.bind(this);
  private boundHandleBlur = this.handleBlur.bind(this);
  private boundHandleInput = this.handleInput.bind(this);
  private boundHandleKeyDown = this.handleKeyDown.bind(this);
  private boundHandleMouseDown = this.handleMouseDown.bind(this);

  constructor(
    element: HTMLDivElement,
    nodeId: string,
    nodeType: string,
    events: ContentEditableEvents,
    config: ContentEditableConfig = {}
  ) {
    this.element = element;
    this.nodeId = nodeId;
    this.nodeType = nodeType;
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
        if (content.startsWith(headerPrefix)) {
          // This is a header node - position cursor after the syntax
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
   * Force update content bypassing input guards for pattern conversions
   */
  public forceUpdateContent(content: string): void {
    // Update stored original content
    this.originalContent = content;

    // Defer the DOM update to avoid conflicts with ongoing input processing
    // This ensures the content update happens after the current input event completes
    requestAnimationFrame(() => {
      if (this.isEditing) {
        this.setRawMarkdown(content);
      } else {
        this.setFormattedContent(content);
      }

      // Position cursor at the end of the cleaned content
      this.positionCursorAtEnd();
    });
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

    // Detect context and position cursor appropriately
    const content = this.element.textContent || '';

    // For empty content or just header syntax (from Enter key), position at end
    // For header syntax with no additional content (from slash commands), position at end
    // The key insight: both scenarios want cursor at end, but for different reasons
    const isEmptyOrHeaderOnly = content.trim() === '' || /^#{1,6}\s*$/.test(content.trim());

    if (isEmptyOrHeaderOnly) {
      setTimeout(() => {
        this.positionCursorAtEnd();
      }, 10);
    }
    // For content that already has text, don't interfere with cursor positioning
  }

  /**
   * Position cursor at the end of the content
   */
  private positionCursorAtEnd(): void {
    const selection = window.getSelection();
    if (!selection) return;

    const range = document.createRange();

    // For multiline content: position cursor in the last line (including empty ones)
    if (this.config.allowMultiline) {
      const lineElements = Array.from(this.element.children).filter(
        child => child.tagName === 'DIV'
      );

      if (lineElements.length > 0) {
        const lastLine = lineElements[lineElements.length - 1];

        // Position cursor at the end of the last line
        if (lastLine.childNodes.length > 0) {
          // If the last line has content, position after the content
          range.selectNodeContents(lastLine);
          range.collapse(false);
        } else {
          // If the last line is empty, position inside it
          range.setStart(lastLine, 0);
          range.setEnd(lastLine, 0);
        }

        selection.removeAllRanges();
        selection.addRange(range);
        return;
      }
    }

    // Fallback for single-line content or no div structure
    range.selectNodeContents(this.element);
    range.collapse(false); // Collapse to end
    selection.removeAllRanges();
    selection.addRange(range);
  }

  /**
   * Position cursor at the beginning of the content
   */
  private positionCursorAtBeginning(): void {
    const selection = window.getSelection();
    if (!selection) return;

    const range = document.createRange();

    // For multiline content: position cursor in the first line
    if (this.config.allowMultiline) {
      const lineElements = Array.from(this.element.children).filter(
        child => child.tagName === 'DIV'
      );

      if (lineElements.length > 0) {
        const firstLine = lineElements[0];

        // Position cursor at the beginning of the first line
        if (firstLine.childNodes.length > 0) {
          // If the first line has content, position before the content
          range.selectNodeContents(firstLine);
          range.collapse(true);
        } else {
          // If the first line is empty, position inside it
          range.setStart(firstLine, 0);
          range.setEnd(firstLine, 0);
        }

        selection.removeAllRanges();
        selection.addRange(range);
        return;
      }
    }

    // Fallback for single-line content or no div structure
    range.selectNodeContents(this.element);
    range.collapse(true); // Collapse to beginning
    selection.removeAllRanges();
    selection.addRange(range);
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
    // Always return the stored content which has been properly converted
    return this.originalContent;
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
    let html = this.markdownToLiveHtml(escapedContent);

    // For multiline nodes: preserve newlines in appropriate format
    if (this.config.allowMultiline) {
      if (this.isEditing) {
        // During editing: convert \n to <div> structure for native browser editing
        // Split by newlines and wrap each part in a div
        const lines = html.split('\n');
        if (lines.length > 1) {
          html = lines.map(line => `<div>${line}</div>`).join('');
        }
      } else {
        // During display: convert \n to <br> tags for formatted display
        html = html.replace(/\n/g, '<br>');
      }
    }

    this.element.innerHTML = html;

    // Post-processing: ensure empty divs in multiline editing mode have <br> tags for visual rendering
    if (this.config.allowMultiline && this.isEditing) {
      const emptyDivs = this.element.querySelectorAll('div:empty');
      emptyDivs.forEach(div => {
        div.innerHTML = '<br>';
      });
    }
  }

  /**
   * Convert markdown to HTML for display mode (no visible syntax markers)
   * Keeps all visual styling but hides the raw markdown syntax
   */
  private markdownToDisplayHtml(content: string): string {
    // Find all formatting patterns including mixed syntax
    const patterns = this.findAllFormattingPatterns(content);

    if (patterns.length === 0) {
      return content; // No formatting patterns found
    }

    let result = '';
    let lastIndex = 0;

    // Process each pattern in order
    patterns.forEach((pattern) => {
      const { start, end, content: innerContent, type } = pattern;

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
      } else if (type === 'code') {
        cssClass = 'markdown-code';
      } else if (type === 'strikethrough') {
        cssClass = 'markdown-strikethrough';
      }

      // Create the display format WITHOUT visible syntax markers
      result += `<span class="${cssClass}">${innerContent}</span>`;

      lastIndex = end;
    });

    // Add any remaining text after the last pattern
    result += content.substring(lastIndex);

    return result;
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
      } else if (type === 'code') {
        cssClass = 'markdown-code';
      } else if (type === 'strikethrough') {
        cssClass = 'markdown-strikethrough';
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
    type: 'bold-italic' | 'bold' | 'italic' | 'code' | 'strikethrough';
  }> {
    const patterns: Array<{
      start: number;
      end: number;
      openMarker: string;
      closeMarker: string;
      content: string;
      type: 'bold-italic' | 'bold' | 'italic' | 'code' | 'strikethrough';
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

      // Code patterns (high precedence - must come before single * and _ for italic)
      { regex: /`([^`]+?)`/g, type: 'code' as const, openMarker: '`', closeMarker: '`' },

      // Strikethrough patterns (medium precedence) - both single and double tilde
      {
        regex: /~~([^~]+?)~~/g,
        type: 'strikethrough' as const,
        openMarker: '~~',
        closeMarker: '~~'
      },
      { regex: /~([^~\n]+?)~/g, type: 'strikethrough' as const, openMarker: '~', closeMarker: '~' },

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
      const htmlContent = this.markdownToDisplayHtml(cleanText);
      this.element.innerHTML = htmlContent;
    } else {
      // For non-headers: show formatted HTML with custom styling (bold, italic, code, strikethrough)
      let htmlContent = this.markdownToDisplayHtml(content);

      // For multiline nodes: ensure newlines are preserved as <br> tags
      if (this.config.allowMultiline) {
        // Convert any remaining \n characters to <br> tags for display
        htmlContent = htmlContent.replace(/\n/g, '<br>');

        // Ensure trailing line breaks are visible by adding a non-breaking space after trailing <br> tags
        if (htmlContent.endsWith('<br>')) {
          // Count consecutive trailing <br> tags
          const trailingBrMatch = htmlContent.match(/(<br>)+$/);
          if (trailingBrMatch) {
            // Replace the last <br> with <br>&nbsp; to ensure it renders visually
            htmlContent = htmlContent.slice(0, -4) + '<br>&nbsp;';
          }
        }
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

    // Special case: if we have <br> tags at the top level (not inside <div>s),
    // use innerText which properly converts <br> to newlines
    const hasBrTags = html.includes('<br>');
    const hasDivTags = html.includes('<div>');

    if (hasBrTags && !hasDivTags) {
      // Remove syntax marker elements before extracting text
      const syntaxMarkers = tempDiv.querySelectorAll('.code-marker, .quote-marker, .syntax-marker');
      syntaxMarkers.forEach((marker) => marker.remove());

      // Use custom BR to text conversion instead of innerText to avoid extra newlines
      return this.convertBrToText(tempDiv);
    }

    // Original logic for <div> structure
    // Remove syntax marker elements before extracting text
    const syntaxMarkers = tempDiv.querySelectorAll('.code-marker, .quote-marker, .syntax-marker');
    syntaxMarkers.forEach((marker) => marker.remove());
    let result = '';

    // Walk through all child nodes
    const childNodes = Array.from(tempDiv.childNodes);

    // Process each node
    for (let i = 0; i < childNodes.length; i++) {
      const node = childNodes[i];

      if (node.nodeType === Node.TEXT_NODE) {
        // Text node: decode HTML entities and add the text content
        const textContent = node.textContent || '';
        const decodedText = textContent.replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>');
        result += decodedText;
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        const element = node as Element;
        if (element.tagName === 'DIV') {
          // Get the content of this div
          const divContent = this.getTextContentIgnoringSyntax(element);

          // If this is not the first DIV, add a newline before it
          if (i > 0 && result.length > 0) {
            result += '\n';
          }

          // Add the content
          result += divContent;
        } else {
          // Other elements: just add their text content (excluding syntax markers)
          const elementContent = this.getTextContentIgnoringSyntax(element);
          result += elementContent;
        }
      }
    }

    return result;
  }

  /**
   * Convert BR tags to text with precise newline control
   */
  private convertBrToText(element: Element): string {
    let result = '';

    for (const node of element.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) {
        result += node.textContent || '';
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        const elem = node as Element;
        if (elem.tagName === 'BR') {
          result += '\n';
        } else {
          // For other elements, get their text content recursively
          result += this.convertBrToText(elem);
        }
      }
    }

    return result;
  }

  /**
   * Convert display mode BR structure to text with newlines for edit mode
   */
  private convertBrDisplayToTextWithNewlines(element: Element): string {
    let result = '';

    // Walk through all child nodes
    for (const node of element.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) {
        // Text node: decode HTML entities (like &nbsp;) and add the text content
        const textContent = node.textContent || '';
        // Decode HTML entities properly
        const decodedText = textContent.replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>');
        result += decodedText;
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        const childElement = node as Element;

        if (childElement.tagName === 'BR') {
          // BR tag: convert to newline
          result += '\n';
        } else {
          // Other elements: extract text content recursively
          // This preserves the text but removes HTML formatting (which is correct for edit mode)
          result += this.getTextContentWithEntitiesDecoded(childElement);
        }
      }
    }

    return result;
  }

  /**
   * Get text content from an element with HTML entities properly decoded
   */
  private getTextContentWithEntitiesDecoded(element: Element): string {
    const textContent = element.textContent || '';
    // Decode HTML entities properly
    return textContent.replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>');
  }

  /**
   * Get text content from an element, ignoring syntax marker elements but preserving line breaks
   */
  private getTextContentIgnoringSyntax(element: Element): string {
    // Special case: if this div contains only a <br> tag, return empty string
    // The DIV processing will handle the newline, so we don't want to double-count it
    if (element.tagName === 'DIV' && element.childNodes.length === 1 &&
        element.firstChild?.nodeName === 'BR') {
      return '';
    }

    let result = '';

    for (const node of element.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) {
        result += node.textContent || '';
      } else if (node.nodeType === Node.ELEMENT_NODE) {
        const childElement = node as Element;

        // Handle <br> tags as newlines
        if (childElement.tagName === 'BR') {
          result += '\n';
        }
        // Skip syntax marker elements
        else if (
          !childElement.classList.contains('code-marker') &&
          !childElement.classList.contains('quote-marker') &&
          !childElement.classList.contains('syntax-marker')
        ) {
          // Recursively process child elements
          result += this.getTextContentIgnoringSyntax(childElement);
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
    // For multiline nodes: The reactive system should handle BR->DIV conversion automatically
    // We just need to ensure originalContent has the correct text with newlines
    // Don't manually convert - let setRawMarkdown and the reactive system handle it

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

      // When editing (focused), extract content - preserve HTML structure for multiline nodes
      let textContent: string;
      if (this.config.allowMultiline) {
        // For multiline nodes: preserve the HTML structure directly to maintain line breaks
        const hasLineBreaks =
          this.element.innerHTML.includes('<div>') || this.element.innerHTML.includes('<br>');
        if (hasLineBreaks) {
          // Store the HTML structure directly to preserve line breaks
          textContent = this.convertHtmlToTextWithNewlines(this.element.innerHTML);
          // But don't overwrite the innerHTML since it's already correct
        } else {
          // No line breaks, use textContent
          textContent = this.element.textContent || '';
        }
      } else {
        // For single-line nodes: use textContent as before
        textContent = this.element.textContent || '';
      }
      this.originalContent = textContent; // Update stored content immediately

      // Check for @ trigger and / slash command detection
      this.checkForTrigger(textContent, cursorOffset);

      // Check for header level changes
      const newHeaderLevel = ContentProcessor.getInstance().parseHeaderLevel(textContent);
      if (newHeaderLevel !== this.currentHeaderLevel) {
        this.currentHeaderLevel = newHeaderLevel;
        this.events.headerLevelChanged(newHeaderLevel);
      }

      // Check for patterns - header pattern takes precedence over task pattern
      this.checkForHeaderPattern(textContent);
      this.checkForTaskShortcut(textContent);

      // Apply live formatting while preserving cursor, unless we just had a Shift+Enter or regular Enter
      // Also skip formatting for multiline nodes with line breaks to preserve <div><br></div> structure
      const hasLineBreaks =
        this.config.allowMultiline &&
        (this.element.innerHTML.includes('<div>') || this.element.innerHTML.includes('<br>'));

      if (!this.recentShiftEnter && !this.recentEnter && !hasLineBreaks) {
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

      // Check if this completes a slash command (e.g., "/task" becomes "/task ")
      if (this.checkForDirectSlashCommand(currentText, futureText)) {
        event.preventDefault(); // Prevent the space from being added
      }
    }

    // Enter key handling - distinguish between regular Enter and Shift+Enter
    if (event.key === 'Enter') {
      // If slash command dropdown is active, let it handle the Enter key
      // Check both the state variable and if dropdown is actually visible in DOM
      const slashDropdownExists = document.querySelector(
        '[role="listbox"][aria-label="Slash command palette"]'
      );
      const autocompleteDropdownExists = document.querySelector(
        '[role="listbox"][aria-label="Node reference autocomplete"]'
      );
      if (
        this.slashCommandDropdownActive ||
        this.autocompleteDropdownActive ||
        slashDropdownExists ||
        autocompleteDropdownExists
      ) {
        return; // Don't preventDefault, let the dropdown handle it
      }

      if (event.shiftKey && this.config.allowMultiline) {
        // Shift+Enter for multiline nodes: allow default browser behavior
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

        // Set flag to prevent cursor restoration during node creation
        this.recentEnter = true;
        setTimeout(() => {
          this.recentEnter = false;
        }, 100);

        // Check if we should create new node above (at beginning/syntax area)
        const shouldCreateAbove = this.shouldCreateNodeAbove(currentContent, cursorPosition);

        if (shouldCreateAbove) {
          // Create new empty node above, preserve original node unchanged
          // Cursor should focus the original node (now below) after creation
          this.events.createNewNode({
            afterNodeId: this.nodeId,
            nodeType: this.nodeType,
            currentContent: currentContent, // Original node keeps its content unchanged
            newContent: '', // New node above starts empty
            originalContent: currentContent,
            cursorAtBeginning: true, // Focus at beginning of bottom node (original node)
            insertAtBeginning: true, // This tells the service to insert BEFORE the current node
            focusOriginalNode: true // Focus the original node (bottom) instead of new node (top)
          });
        } else {
          // Normal splitting behavior for middle/end positions
          const splitResult = splitMarkdownContent(currentContent, cursorPosition);

          // Update current element immediately to show completed syntax
          this.originalContent = splitResult.beforeContent;
          this.element.textContent = splitResult.beforeContent;

          this.events.createNewNode({
            afterNodeId: this.nodeId,
            nodeType: this.nodeType, // Preserve original node's type
            currentContent: splitResult.beforeContent,
            newContent: splitResult.afterContent,
            originalContent: currentContent, // Pass original content before split for inheritance
            cursorAtBeginning: false,
            insertAtBeginning: false // Normal splitting creates nodes after, not above
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
      // If any modal is active, let the modal handle the arrow keys
      if (this.slashCommandDropdownActive || this.autocompleteDropdownActive) {
        return;
      }
      const direction = event.key === 'ArrowUp' ? 'up' : 'down';

      // Determine if we should navigate between nodes
      let shouldNavigate = false;

      if (this.config.allowMultiline) {
        // For multiline nodes, navigate between nodes when on first/last line
        // preserving horizontal position (columnHint)
        if (direction === 'up') {
          // Navigate if on the first line (regardless of horizontal position)
          shouldNavigate = this.isAtFirstLine();
        } else {
          // Navigate if on the last line (regardless of horizontal position)
          shouldNavigate = this.isAtLastLine();
        }

        if (!shouldNavigate) {
          // Let the browser handle line-by-line navigation within the multiline node
          // Don't prevent default - let browser handle within-node navigation
          return;
        }
      } else {
        // For single-line nodes, always navigate on arrow up/down
        // (there's only one line, so we're always on first/last line)
        shouldNavigate = true;
      }

      // Navigate between nodes
      event.preventDefault(); // Prevent browser from handling this
      const pixelOffset = this.getCurrentPixelOffset();

      console.log('[NAVIGATION TEST] Exiting node:', {
        nodeId: this.nodeId,
        direction,
        pixelOffset,
        elementLeft: this.element.getBoundingClientRect().left,
        containerLeft: this.element.closest('.node-container')?.getBoundingClientRect().left,
        rootLeft: (this.element.closest('.base-node-viewer') || document.body).getBoundingClientRect().left
      });

      this.events.navigateArrow({
        nodeId: this.nodeId,
        direction,
        pixelOffset
      });
      return;
    }

    // Backspace at start of node
    if (event.key === 'Backspace' && this.isAtStart()) {
      // For multi-line nodes, check if we're at the start of the first line or just at the start of a line
      if (this.config.allowMultiline) {
        const isAtStartOfFirstLine = this.isAtStartOfFirstLine();
        if (!isAtStartOfFirstLine) {
          // We're at the start of a line other than the first line
          // Allow default backspace behavior to delete the line break
          return;
        }
        // We're at the start of the first line, so combine with previous node
      }

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

    if (!range.collapsed) {
      return false; // Not at start if there's a selection
    }

    // Use the same logic as getCurrentColumn to handle complex HTML structures
    const currentPosition = this.getCurrentColumn();
    return currentPosition === 0;
  }

  private isAtEnd(): boolean {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return false;
    }

    const range = selection.getRangeAt(0);

    if (!range.collapsed) {
      return false; // Not at end if there's a selection
    }

    // For multiline content, check if we're at the end of the last line
    if (this.config.allowMultiline) {
      // Check if we're on the last line
      if (!this.isAtLastLine()) {
        return false;
      }

      // Then check if we're at the end of that last line
      // We do this by checking if we're at the end of the text node we're in
      if (range.startContainer.nodeType === Node.TEXT_NODE) {
        const textNode = range.startContainer as Text;
        // Check if cursor is at the end of this text node
        if (range.startOffset < textNode.length) {
          return false; // Not at end of text node
        }

        // We're at the end of a text node - check if there are any more text nodes after this one
        const currentLineIndex = this.getCurrentLineIndex(range);
        if (currentLineIndex === -1) return false;

        const lineElements = Array.from(this.element.children).filter(
          child => child.tagName === 'DIV'
        );
        const lastLineElement = lineElements[currentLineIndex];
        if (!lastLineElement) return false;

        // Check if our text node is the last text-containing node in the line
        const walker = document.createTreeWalker(
          lastLineElement,
          NodeFilter.SHOW_TEXT,
          {
            acceptNode: (node) => {
              // Skip empty text nodes and whitespace-only nodes
              const text = node.textContent || '';
              return text.trim().length > 0 ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP;
            }
          }
        );

        let lastTextNode: Node | null = null;
        while (walker.nextNode()) {
          lastTextNode = walker.currentNode;
        }

        // We're at the end if our text node is the last text node
        return textNode === lastTextNode;
      } else {
        // Cursor is not in a text node - check if we're at the end of an empty line
        return range.startOffset >= range.startContainer.childNodes.length;
      }
    } else {
      // For single-line content, check if we're at the end of the entire element
      const textContent = this.element.textContent || '';
      const currentPosition = this.getCurrentColumn();
      return currentPosition === textContent.length;
    }
  }

  private isAtBeginningOfFirstLine(): boolean {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return false;
    }

    const range = selection.getRangeAt(0);

    if (!range.collapsed) {
      return false; // Not at beginning if there's a selection
    }

    // For multiline content, check if we're at the beginning of the first line
    const lineElements = Array.from(this.element.children).filter(
      child => child.tagName === 'DIV'
    );

    if (lineElements.length === 0) {
      // No DIV structure yet - check if there are line breaks in the content
      const textContent = this.element.textContent || '';
      if (!textContent.includes('\n')) {
        // Single-line content - use isAtStart
        return this.isAtStart();
      }
      // Has line breaks but no DIV structure yet - check cursor position in text
      const preCaretRange = range.cloneRange();
      preCaretRange.selectNodeContents(this.element);
      preCaretRange.setEnd(range.startContainer, range.startOffset);
      const textBeforeCursor = preCaretRange.toString();
      // At beginning of first line if no text before cursor OR only whitespace before first newline
      return textBeforeCursor.length === 0 || !textBeforeCursor.includes('\n');
    }

    // Has DIV children - this is multiline content
    // First line could be a text node before the first DIV, or the first DIV itself
    const firstChild = this.element.childNodes[0];

    // Check if cursor is before any DIV (in leading text node)
    let currentElement: Node | null = range.startContainer;
    let isBeforeDivs = true;

    // Walk up to find if we're inside a DIV
    while (currentElement && currentElement !== this.element) {
      if (currentElement.nodeType === Node.ELEMENT_NODE &&
          (currentElement as Element).tagName === 'DIV' &&
          currentElement.parentNode === this.element) {
        isBeforeDivs = false;
        break;
      }
      currentElement = currentElement.parentNode;
    }

    if (isBeforeDivs) {
      // Cursor is in text before any DIVs - check if at start of that text
      return this.isAtStart();
    }

    // Cursor is inside a DIV - check if it's the first DIV and we're at its start
    const firstLine = lineElements[0];
    currentElement = range.startContainer;
    let isInFirstLine = false;

    while (currentElement && currentElement !== this.element) {
      if (currentElement === firstLine) {
        isInFirstLine = true;
        break;
      }
      currentElement = currentElement.parentNode;
    }

    if (!isInFirstLine) {
      return false;
    }

    // Check if there's text content before the first DIV
    // If yes, the first DIV is NOT the first line - the text before it is
    const firstDivIndex = Array.from(this.element.childNodes).indexOf(firstLine as ChildNode);
    let hasTextBeforeFirstDiv = false;

    for (let i = 0; i < firstDivIndex; i++) {
      const node = this.element.childNodes[i];
      if (node.nodeType === Node.TEXT_NODE && node.textContent?.trim()) {
        hasTextBeforeFirstDiv = true;
        console.log('[isAtBeginningOfFirstLine] Found text before DIV:', node.textContent?.trim());
        break;
      }
    }

    console.log('[isAtBeginningOfFirstLine] hasTextBeforeFirstDiv:', hasTextBeforeFirstDiv, 'firstDivIndex:', firstDivIndex);

    if (hasTextBeforeFirstDiv) {
      // There's text before this DIV, so we're NOT at the beginning of the first line
      console.log('[isAtBeginningOfFirstLine] Returning FALSE - not at beginning because text exists before DIV');
      return false;
    }

    // No text before first DIV - check if we're at the beginning of this first DIV
    const lineRange = document.createRange();
    lineRange.selectNodeContents(firstLine);
    lineRange.setEnd(range.startContainer, range.startOffset);

    return lineRange.toString().length === 0;
  }

  private isAtEndOfLastLine(): boolean {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return false;
    }

    const range = selection.getRangeAt(0);

    if (!range.collapsed) {
      return false; // Not at end if there's a selection
    }

    // For multiline content, check if we're at the end of the last line
    const lineElements = Array.from(this.element.children).filter(
      child => child.tagName === 'DIV'
    );

    if (lineElements.length === 0) {
      // No DIV structure yet - check if there are line breaks in the content
      const textContent = this.element.textContent || '';
      if (!textContent.includes('\n')) {
        // Single-line content - use isAtEnd
        return this.isAtEnd();
      }
      // Has line breaks but no DIV structure yet - check cursor position in text
      const postCaretRange = range.cloneRange();
      postCaretRange.selectNodeContents(this.element);
      postCaretRange.setStart(range.startContainer, range.startOffset);
      const textAfterCursor = postCaretRange.toString();
      // At end of last line if no text after cursor OR no newline after cursor
      return textAfterCursor.length === 0 || !textAfterCursor.includes('\n');
    }

    // Has DIV children - this is multiline content
    // Last line is always the last DIV (trailing text after DIVs would be in a new line)
    const lastLine = lineElements[lineElements.length - 1];

    // Check if cursor is inside the last DIV
    let currentElement: Node | null = range.startContainer;
    let isInLastLine = false;

    while (currentElement && currentElement !== this.element) {
      if (currentElement === lastLine) {
        isInLastLine = true;
        break;
      }
      currentElement = currentElement.parentNode;
    }

    if (!isInLastLine) {
      // Not in the last DIV - check if we're in trailing text after all DIVs
      // If cursor is after all DIVs (in trailing text), we're at end of last line
      const lastChildIndex = Array.from(this.element.childNodes).indexOf(
        range.startContainer as ChildNode
      );
      const lastDivIndex = Array.from(this.element.childNodes).indexOf(lastLine as ChildNode);

      if (lastChildIndex > lastDivIndex) {
        // Cursor is in text after the last DIV - check if at end
        return this.isAtEnd();
      }

      return false;
    }

    // Check if we're at the end of this last line
    const lineRange = document.createRange();
    lineRange.selectNodeContents(lastLine);
    lineRange.setStart(range.startContainer, range.startOffset);

    return lineRange.toString().length === 0;
  }

  /**
   * Get current cursor position as pixel offset from root container.
   * This eliminates character width conversion issues with proportional fonts.
   */
  private getCurrentPixelOffset(): number {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return 0;

    const range = selection.getRangeAt(0);

    try {
      // Measure where cursor is currently
      const cursorRange = document.createRange();
      cursorRange.setStart(range.startContainer, range.startOffset);
      cursorRange.setEnd(range.startContainer, range.startOffset);

      // Check if getBoundingClientRect is available (not in jsdom tests)
      if (typeof cursorRange.getBoundingClientRect !== 'function') {
        console.log('[getCurrentPixelOffset] getBoundingClientRect not available (test environment)');
        return 0;
      }

      const cursorRect = cursorRange.getBoundingClientRect();

      // Get root container for absolute positioning
      const rootContainer = this.element.closest('.base-node-viewer') ||
                           this.element.closest('.node-viewer-container') ||
                           document.body;
      const rootRect = rootContainer.getBoundingClientRect();

      const pixelOffset = cursorRect.left - rootRect.left;

      console.log('[getCurrentPixelOffset] pixelOffset:', Math.round(pixelOffset));
      return pixelOffset;
    } catch (e) {
      console.warn('[getCurrentPixelOffset] Error measuring pixel offset:', e);
      return 0;
    }
  }

  private getFirstTextNode(element: HTMLElement): Text | null {
    for (const node of element.childNodes) {
      if (node.nodeType === Node.TEXT_NODE && node.textContent?.trim()) {
        return node as Text;
      }
      if (node.nodeType === Node.ELEMENT_NODE) {
        const found = this.getFirstTextNode(node as HTMLElement);
        if (found) return found;
      }
    }
    return null;
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

  /**
   * Get the current line index (0-based) for a given range in multiline content
   * Returns -1 if unable to determine line index
   */
  private getCurrentLineIndex(range: Range): number {
    if (!this.config.allowMultiline) return 0;

    // Get all direct div children (lines) of this contenteditable element
    const lineElements = Array.from(this.element.children).filter(
      child => child.tagName === 'DIV'
    );

    if (lineElements.length === 0) {
      return -1; // No div structure found
    }

    // First check if range.startContainer is itself one of the line divs
    if (range.startContainer.nodeType === Node.ELEMENT_NODE &&
        (range.startContainer as Element).tagName === 'DIV' &&
        range.startContainer.parentNode === this.element) {
      const index = lineElements.indexOf(range.startContainer as Element);
      if (index !== -1) {
        return index;
      }
    }

    // Find which div contains the cursor
    let currentElement: Node | null = range.startContainer;

    // Walk up to find the containing div
    while (currentElement && currentElement !== this.element) {
      if (
        currentElement.nodeType === Node.ELEMENT_NODE &&
        (currentElement as Element).tagName === 'DIV' &&
        currentElement.parentNode === this.element
      ) {
        // Found the containing div - return its index
        return lineElements.indexOf(currentElement as Element);
      }
      currentElement = currentElement.parentNode;
    }

    // Special case: if cursor is directly at the contenteditable element itself
    if (range.startContainer === this.element) {
      // Check if we're at the very beginning (before first child)
      if (range.startOffset === 0) {
        return 0; // First line
      }
      // Check if we're at the very end (after last child)
      if (range.startOffset >= this.element.childNodes.length) {
        return lineElements.length - 1; // Last line
      }
      // Try to determine based on offset
      const childAtOffset = this.element.childNodes[range.startOffset];
      if (childAtOffset && childAtOffset.nodeType === Node.ELEMENT_NODE) {
        const index = lineElements.indexOf(childAtOffset as Element);
        return index >= 0 ? index : -1;
      }
    }

    return -1; // Unable to determine
  }

  private isAtFirstLine(): boolean {
    if (!this.config.allowMultiline) return true;

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return true;

    const range = selection.getRangeAt(0);

    // Get the current line index using a more robust approach
    const currentLineIndex = this.getCurrentLineIndex(range);

    // If we can determine the line index, check if it's 0 (first line)
    if (currentLineIndex !== -1) {
      return currentLineIndex === 0;
    }

    // Fallback to the original approach
    // Find the containing div for the current cursor position
    let containingDiv: Element | null = null;
    let currentElement: Node | null = range.startContainer;

    // Walk up to find the containing div within our contenteditable
    while (currentElement && currentElement !== this.element) {
      if (
        currentElement.nodeType === Node.ELEMENT_NODE &&
        (currentElement as Element).tagName === 'DIV' &&
        currentElement.parentNode === this.element
      ) {
        containingDiv = currentElement as Element;
        break;
      }
      currentElement = currentElement.parentNode;
    }

    // If we found a containing div, check if it's the first line
    if (containingDiv) {
      // Check if there's text content before this DIV
      const divIndex = Array.from(this.element.childNodes).indexOf(containingDiv as ChildNode);
      for (let i = 0; i < divIndex; i++) {
        const node = this.element.childNodes[i];
        if (node.nodeType === Node.TEXT_NODE && node.textContent?.trim()) {
          // There's text before this DIV, so this DIV is NOT the first line
          return false;
        }
      }
      // No text before this DIV - it IS the first line
      return containingDiv === this.element.firstElementChild;
    }

    // Special handling: if cursor is directly at a BR element, check its parent
    if (range.startContainer.nodeType === Node.ELEMENT_NODE &&
        (range.startContainer as Element).tagName === 'BR') {
      const brParent = range.startContainer.parentNode;
      if (brParent && brParent.nodeType === Node.ELEMENT_NODE &&
          (brParent as Element).tagName === 'DIV' &&
          brParent.parentNode === this.element) {
        return brParent === this.element.firstElementChild;
      }
    }

    // Additional fallback: if cursor is at the very beginning of the element
    if (range.startContainer === this.element && range.startOffset === 0) {
      return true;
    }

    // If no div structure found, assume single line (first line)
    return true;
  }

  private isAtLastLine(): boolean {
    if (!this.config.allowMultiline) return true;

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return true;

    const range = selection.getRangeAt(0);

    // Get the current line index using a more robust approach
    const currentLineIndex = this.getCurrentLineIndex(range);

    // If we can determine the line index, check if it's the last line
    if (currentLineIndex !== -1) {
      const lineElements = Array.from(this.element.children).filter(
        child => child.tagName === 'DIV'
      );
      return currentLineIndex === lineElements.length - 1;
    }

    // Fallback to the original approach
    // Check if cursor is within the last div element (last line)
    let currentElement: Node | null = range.startContainer;

    // Walk up to find the containing div within our contenteditable
    while (currentElement && currentElement !== this.element) {
      if (
        currentElement.nodeType === Node.ELEMENT_NODE &&
        (currentElement as Element).tagName === 'DIV' &&
        currentElement.parentNode === this.element
      ) {
        // Found the containing div - check if it's the last one
        return currentElement === this.element.lastElementChild;
      }
      currentElement = currentElement.parentNode;
    }

    // Special case: if cursor is in a BR element, check if the BR is in the last div
    if (range.startContainer.nodeType === Node.ELEMENT_NODE &&
        (range.startContainer as Element).tagName === 'BR') {
      const brParent = range.startContainer.parentNode;
      if (brParent && brParent.nodeType === Node.ELEMENT_NODE &&
          (brParent as Element).tagName === 'DIV' &&
          brParent.parentNode === this.element) {
        return brParent === this.element.lastElementChild;
      }
    }

    // If no div structure found, assume single line (last line)
    return true;
  }

  private isAtStartOfFirstLine(): boolean {
    if (!this.config.allowMultiline) return true;

    // First check if we're at the first line
    if (!this.isAtFirstLine()) {
      return false;
    }

    // Then check if we're at the start of that first line
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return false;

    const range = selection.getRangeAt(0);
    if (!range.collapsed) return false;

    // For the first line, we need to check if we're at position 0 of the entire element
    const preCaretRange = range.cloneRange();
    preCaretRange.selectNodeContents(this.element);
    preCaretRange.setEnd(range.startContainer, range.startOffset);

    return preCaretRange.toString().length === 0;
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
    // Validate that selection is within a genuine *** pattern, not nested ** + _ patterns
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

    // Verify this is a genuine ***text*** pattern, not nested **_text_** combinations
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
   * Handles nested formatting properly without interfering with normal operations
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

    // For nesting behavior, only check for exact marker matches
    // This allows different but equivalent markers to nest properly
    // e.g., __bold__ + Cmd+B (**) should nest to **__bold__**
    let equivalentMarkers: string[];
    equivalentMarkers = [marker]; // Only check for exact marker match

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
   * Prevents ** from being detected as * markers in sequential operations
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
   * Prevents ** from being incorrectly detected as * markers by checking adjacent characters
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
   * Handles double-click selection that includes formatting markers
   * When user double-clicks "__bold__", the selection includes underscores and should be toggled off
   */
  private isTextAlreadyFormatted(text: string, targetMarker: string): boolean {
    // Handle case where user selects "__bold__" and applies different formatting ("**")
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
   * Handles equivalent markers when removing formatting (for double-click scenarios)
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
        this.clearSlashCommandSession();
        this.events.slashCommandHidden();
      }
    } else {
      // Hide slash commands when @ is active
      this.clearSlashCommandSession();
      this.events.slashCommandHidden();
    }
  }

  /**
   * Check for task shortcut patterns and convert node type if detected
   * Patterns: [ ], [x], [X], [~], [o]
   */
  private checkForTaskShortcut(content: string): void {
    // Don't process task patterns if there's a header pattern - header takes precedence
    const headerPattern = /^(#{1,6})\s/;
    if (headerPattern.test(content)) {
      return;
    }

    // Only trigger task conversion when pattern is complete with space after bracket
    const taskPattern = /^\s*-?\s*\[(x|X|~|o|\s)\]\s+/; // Requires space after bracket
    const hasTaskPattern = taskPattern.test(content);

    if (this.nodeType === 'task' && !hasTaskPattern) {
      // Allow task-to-text conversion ONLY for header patterns
      // Check if user is typing a header pattern
      const headerPattern = /^(#{1,6})\s/;
      const hasHeaderPattern = headerPattern.test(content);

      if (hasHeaderPattern) {
        // Task node with header pattern - convert to text (KEEP header syntax)

        this.events.nodeTypeConversionDetected({
          nodeId: this.nodeId,
          newNodeType: 'text',
          cleanedContent: content // Keep the full content including "# "
        });
      }
      // For non-header content, task nodes stay as tasks (sticky behavior)
    } else if (this.nodeType !== 'task' && hasTaskPattern) {
      // Non-task node that now has task pattern - convert to task using slash command flow
      const cleanedContent = content.replace(taskPattern, '').trim();

      // Use the exact same pattern as header detection - let event system handle everything
      this.events.nodeTypeConversionDetected({
        nodeId: this.nodeId,
        newNodeType: 'task',
        cleanedContent
      });
    }
  }

  /**
   * Check for header pattern and convert node type if detected
   * Patterns: # , ## , ###
   */
  private checkForHeaderPattern(content: string): void {
    // Check if content starts with header pattern (# followed by space)
    const headerPattern = /^(#{1,6})\s/;
    const match = content.match(headerPattern);
    const hasHeaderPattern = !!match;
    const headerLevel = match ? match[1].length : 0;

    // Only convert to text node with header if we're not already in that state
    if (hasHeaderPattern && (this.nodeType !== 'text' || this.currentHeaderLevel !== headerLevel)) {
      // For headers, KEEP the syntax visible in the editor
      const cleanedContent = content; // Keep "# " syntax for proper header formatting

      // Convert to text node and update header level
      this.events.nodeTypeConversionDetected({
        nodeId: this.nodeId,
        newNodeType: 'text',
        cleanedContent
      });

      // Also emit header level change
      this.currentHeaderLevel = headerLevel;
      this.events.headerLevelChanged(headerLevel);
    } else if (!hasHeaderPattern && this.nodeType === 'text' && this.currentHeaderLevel > 0) {
      // Text node that no longer has header pattern - reset to normal text
      this.currentHeaderLevel = 0;
      this.events.headerLevelChanged(0);
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
    } catch {
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
    } catch {
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
    // For multiline content: if positioning at end, use enhanced positioning
    if (this.config.allowMultiline) {
      const textContent = this.element.textContent || '';

      // If trying to position at or beyond the end of text content
      if (characterIndex >= textContent.length) {
        this.positionCursorAtEnd();
        return;
      }

      // If positioning at the beginning
      if (characterIndex === 0) {
        this.positionCursorAtBeginning();
        return;
      }
    }

    // Fallback to character-based positioning
    this.restoreCursorPosition(characterIndex);
  }

  /**
   * Position cursor at the end of content, respecting multiline structure (public API)
   */
  public setCursorAtEnd(): void {
    this.positionCursorAtEnd();
  }

  /**
   * Position cursor at the beginning of content, respecting multiline structure (public API)
   */
  public setCursorAtBeginning(): void {
    this.positionCursorAtBeginning();
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

    // Start or update slash command session tracking
    if (!this.slashCommandSession) {
      // New session - capture the content before the slash
      this.slashCommandSession = {
        active: true,
        startPosition: lastSlashIndex,
        query: queryText,
        originalContent: content
      };
    } else {
      // Update existing session query
      this.slashCommandSession.query = queryText;
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
   * Set whether the slash command dropdown is currently active
   * This prevents Enter key from creating new nodes when dropdown should handle it
   */
  public setSlashCommandDropdownActive(active: boolean): void {
    this.slashCommandDropdownActive = active;
  }

  /**
   * Set whether the autocomplete dropdown is currently active
   * This prevents arrow keys from moving the cursor when autocomplete should handle them
   */
  public setAutocompleteDropdownActive(active: boolean): void {
    this.autocompleteDropdownActive = active;
  }

  /**
   * Insert slash command content at current cursor position
   */
  public insertSlashCommand(
    content: string,
    skipCursorPositioning = false,
    targetNodeType?: string
  ): void {
    const currentText = this.element.textContent || '';

    // Use session tracking if available, otherwise fallback to legacy logic
    if (this.slashCommandSession && this.slashCommandSession.active) {
      // Session-based replacement - we know exactly what to replace
      const session = this.slashCommandSession;
      const replaceStart = session.startPosition;
      const replaceEnd = session.startPosition + 1 + session.query.length; // "/" + query length

      let beforeSlash = currentText.substring(0, replaceStart);
      let afterQuery = currentText.substring(replaceEnd);

      // Clean up header syntax if converting from text node to non-text node
      if (targetNodeType && this.nodeType === 'text' && targetNodeType !== 'text') {
        const headerPattern = /^(#{1,6})\s+/;
        // Clean from both beforeSlash and afterQuery since we don't know where the header syntax is
        beforeSlash = beforeSlash.replace(headerPattern, '').trim();
        afterQuery = afterQuery.replace(headerPattern, '').trim();
      }

      const newContent = beforeSlash + content + afterQuery;

      // Clear session after successful replacement
      this.clearSlashCommandSession();

      // Update content and position cursor
      this.originalContent = newContent;
      this.setLiveFormattedContent(newContent);
      this.events.contentChanged(newContent);

      // Position cursor where the "/" was originally typed
      let newCursorPos = beforeSlash.length + content.length;

      // Special case: for header content (like "# "), position cursor after the syntax for ready typing
      const contentHeaderMatch = content.match(/^(#{1,6}\s+)(.*)$/);
      if (contentHeaderMatch) {
        // Position cursor after the header syntax (e.g., after "# " in "# content")
        newCursorPos = beforeSlash.length + contentHeaderMatch[1].length;
      }
      // Special case: for empty content (like task conversion), position cursor where "/" was
      else if (content === '') {
        newCursorPos = beforeSlash.length;
      }

      if (!skipCursorPositioning) {
        setTimeout(() => {
          this.restoreCursorPosition(newCursorPos);
        }, 0);
      }
    } else {
      // Fallback to legacy logic for cases without session tracking
      const lastSlashIndex = currentText.lastIndexOf('/');
      if (lastSlashIndex === -1) {
        return;
      }

      // Simple fallback: only replace the "/" itself
      const beforeSlash = currentText.substring(0, lastSlashIndex);
      const afterSlash = currentText.substring(lastSlashIndex + 1);
      const newContent = beforeSlash + content + afterSlash;

      this.originalContent = newContent;
      this.setLiveFormattedContent(newContent);
      this.events.contentChanged(newContent);

      if (!skipCursorPositioning) {
        setTimeout(() => {
          // Position cursor where the "/" was originally typed
          this.restoreCursorPosition(beforeSlash.length + content.length);
        }, 0);
      }
    }
  }

  /**
   * Clear slash command session tracking
   */
  private clearSlashCommandSession(): void {
    if (this.slashCommandSession) {
      this.slashCommandSession = null;
    }
  }

  /**
   * Check if typing a space completes a direct slash command like "/task "
   * Returns true if a command was executed (so the space should be prevented)
   */
  private checkForDirectSlashCommand(_currentText: string, _futureText: string): boolean {
    // Only check if we have an active slash command session
    if (!this.slashCommandSession || !this.slashCommandSession.active) {
      return false;
    }

    const session = this.slashCommandSession;
    const query = session.query;

    // Use SlashCommandService to check if this is a valid command
    const slashCommandService = SlashCommandService.getInstance();
    const command = slashCommandService.findCommand(query.toLowerCase());

    if (!command) {
      return false; // Not a known command
    }

    // Use setTimeout to execute after the current event handling
    setTimeout(() => {
      // Execute the command properly using SlashCommandService (like the dropdown does)
      const result = slashCommandService.executeCommand(command);

      // Execute the slash command replacement (skip cursor positioning since parent will handle it)
      // Pass the target node type so insertSlashCommand can clean header syntax appropriately
      this.insertSlashCommand(result.content, true, result.nodeType);

      // For slash commands, cursor always goes to beginning (position 0) since commands only work at start
      const cursorPosition = 0;

      // Hide the slash command dropdown immediately since command is complete
      this.events.slashCommandHidden();

      // Clear the slash command session since it's complete
      this.clearSlashCommandSession();

      // Emit the direct slash command event that will be handled by base-node to dispatch to parent
      this.events.directSlashCommand({
        command: command.id,
        nodeType: result.nodeType,
        cursorPosition: cursorPosition
      });
    }, 0);

    return true; // Command was executed, prevent space
  }

  /**
   * Determine if we should create a new node above instead of splitting
   * This preserves the original node's identity and relationships
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
      // (e.g., `|#`, `#|`, `# |`, `## |` - all considered "beginning")
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
        // (e.g., `|**`, `*|*`, `**|` for bold)
        if (position <= format.length) {
          return true;
        }
      }
    }

    // For all other cases, use normal splitting
    return false;
  }
}
