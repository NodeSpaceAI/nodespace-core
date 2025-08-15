<!--
  Minimal BaseNode - Single-line by default, Enter creates new node
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import Icon, { type IconName } from '$lib/design/icons';
  import { isAtNodeBoundary } from '$lib/utils/navigationUtils.js';
  import type { NodeNavigationMethods } from '$lib/types/navigation.js';

  export let nodeId: string = '';
  export let nodeType: string = 'text'; // Type of node for creating new instances
  export let autoFocus: boolean = false; // Focus this node when created
  export let allowNewNodeOnEnter: boolean = true; // Allow Enter to create new node
  export let splitContentOnEnter: boolean = false; // Split content at cursor position
  export let multiline: boolean = false; // Allow multiline content (Shift+Enter for line breaks)
  export let content: string = ''; // Content of the node
  export let className: string = ''; // Additional CSS classes for the container
  export let children: any[] = []; // Child nodes for parent indicator
  export let iconName: IconName | undefined = undefined; // Custom icon override
  export const canBeCombined: (() => boolean) | undefined = undefined; // Function to check if node can be combined

  let contentEditableElement: HTMLDivElement;
  let isApplyingHeaderFormatting = false; // Flag to prevent reactive interference
  
  // Compute if this node has children
  $: hasChildren = children?.length > 0;
  
  // Determine which icon to display
  $: displayIcon = iconName || (hasChildren ? 'circle-ring' : 'circle');
  
  // Always use 16px since both icons have same structure now
  $: iconSize = 16;
  
  // Node type color mapping - uses design system CSS custom properties
  $: nodeColor = `hsl(var(--node-${nodeType}, var(--node-text)))`; // Fallback to text node color

  // Formatting state for keyboard shortcuts only
  const formatClasses = {
    bold: 'markdown-bold',
    italic: 'markdown-italic',
    underline: 'markdown-underline'
  };

  // Convert HTML formatting to markdown for storage (BaseNode: inline formatting only)
  function htmlToMarkdown(htmlContent: string): string {
    let markdown = htmlContent;
    
    // Convert span classes to markdown syntax
    markdown = markdown.replace(/<span class="markdown-bold">(.*?)<\/span>/g, '**$1**');
    markdown = markdown.replace(/<span class="markdown-italic">(.*?)<\/span>/g, '*$1*');
    markdown = markdown.replace(/<span class="markdown-underline">(.*?)<\/span>/g, '__$1__');
    
    // Clean up any remaining HTML tags
    markdown = markdown.replace(/<[^>]*>/g, '');
    
    return markdown;
  }

  // Convert markdown to HTML formatting for display (BaseNode: inline formatting only)
  function markdownToHtml(markdownContent: string): string {
    let html = markdownContent;
    
    // Convert inline formatting only
    html = html.replace(/\*\*(.*?)\*\*/g, '<span class="markdown-bold">$1</span>');
    html = html.replace(/(?<!\*)\*([^*]+)\*(?!\*)/g, '<span class="markdown-italic">$1</span>');
    html = html.replace(/__(.*?)__/g, '<span class="markdown-underline">$1</span>');
    
    return html;
  }

  // Apply formatting directly via DOM manipulation (consistent span classes)
  function applyDirectFormatting(formatType: 'bold' | 'italic' | 'underline') {
    if (!contentEditableElement) return;
    
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return;
    
    // Always use manual formatting for consistency (no execCommand)
    applyManualFormatting(formatType, selection);
  }

  // Manual formatting with toggle support
  function applyManualFormatting(formatType: 'bold' | 'italic' | 'underline', selection: Selection) {
    const range = selection.getRangeAt(0);
    
    // Get the start container to check for existing formatting
    const startContainer = range.startContainer;
    
    // Find if selection is entirely within a formatting span of the target type
    let formatSpan = null;
    
    // Check if the selection starts and ends within the same formatting span
    let currentNode = startContainer.nodeType === Node.TEXT_NODE ? startContainer.parentElement : startContainer;
    while (currentNode && currentNode !== contentEditableElement) {
      if (currentNode.matches && currentNode.matches(`span.${formatClasses[formatType]}`)) {
        // Check if the entire selection is within this span
        const spanRange = document.createRange();
        spanRange.selectNodeContents(currentNode);
        
        if (spanRange.comparePoint(range.startContainer, range.startOffset) >= 0 &&
            spanRange.comparePoint(range.endContainer, range.endOffset) <= 0) {
          formatSpan = currentNode;
          break;
        }
      }
      currentNode = currentNode.parentElement;
    }
    
    if (formatSpan) {
      // Remove formatting: unwrap the span while preserving selection
      const selectedText = selection.toString();
      const parent = formatSpan.parentNode;
      
      // Unwrap the span
      const unwrappedNodes = [];
      while (formatSpan.firstChild) {
        const child = formatSpan.firstChild;
        unwrappedNodes.push(child);
        parent.insertBefore(child, formatSpan);
      }
      parent.removeChild(formatSpan);
      
      // Restore selection using a simpler approach
      if (selectedText && unwrappedNodes.length > 0) {
        // For single text node (most common case)
        if (unwrappedNodes.length === 1 && unwrappedNodes[0].nodeType === Node.TEXT_NODE) {
          const textNode = unwrappedNodes[0];
          const newRange = document.createRange();
          newRange.setStart(textNode, 0);
          newRange.setEnd(textNode, textNode.textContent.length);
          selection.removeAllRanges();
          selection.addRange(newRange);
        } else {
          // For multiple nodes, select all content
          const newRange = document.createRange();
          newRange.setStartBefore(unwrappedNodes[0]);
          newRange.setEndAfter(unwrappedNodes[unwrappedNodes.length - 1]);
          selection.removeAllRanges();
          selection.addRange(newRange);
        }
      }
    } else {
      // Add formatting: wrap selection in span
      const selectedContent = range.extractContents();
      
      const span = document.createElement('span');
      span.className = formatClasses[formatType];
      span.appendChild(selectedContent);
      
      range.insertNode(span);
      
      // Keep selection on the new span content
      const newRange = document.createRange();
      newRange.selectNodeContents(span);
      selection.removeAllRanges();
      selection.addRange(newRange);
    }
    
    // Update content prop with new innerHTML converted to markdown
    const htmlContent = contentEditableElement.innerHTML || '';
    const markdownContent = htmlToMarkdown(htmlContent);
    if (markdownContent !== content) {
      content = markdownContent;
      dispatch('contentChanged', { content: markdownContent });
    }
  }
  
  // Get all text nodes within an element
  function getTextNodes(element: Element): Text[] {
    const textNodes: Text[] = [];
    const walker = document.createTreeWalker(
      element,
      NodeFilter.SHOW_TEXT,
      null
    );
    
    let node;
    while (node = walker.nextNode()) {
      textNodes.push(node as Text);
    }
    
    return textNodes;
  }

  // Split HTML content at cursor position with formatting carry-over
  function splitHtmlAtCursor(): { currentNodeHtml: string; newNodeHtml: string } {
    if (!contentEditableElement) {
      return { currentNodeHtml: '', newNodeHtml: '' };
    }

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return { currentNodeHtml: '', newNodeHtml: '' };
    }

    const range = selection.getRangeAt(0);
    
    // Create a temporary container to work with ranges
    const tempDiv = document.createElement('div');
    tempDiv.innerHTML = contentEditableElement.innerHTML;
    
    // Create range in temp container matching cursor position
    const cursorOffset = getCursorPosition();
    
    // Find the text position in the temp container
    let currentOffset = 0;
    let splitNode: Node | null = null;
    let splitOffset = 0;
    
    const walker = document.createTreeWalker(
      tempDiv,
      NodeFilter.SHOW_TEXT,
      null
    );
    
    let textNode;
    while (textNode = walker.nextNode()) {
      const nodeLength = textNode.textContent?.length || 0;
      if (currentOffset + nodeLength >= cursorOffset) {
        splitNode = textNode;
        splitOffset = cursorOffset - currentOffset;
        break;
      }
      currentOffset += nodeLength;
    }
    
    if (!splitNode) {
      return { currentNodeHtml: tempDiv.innerHTML, newNodeHtml: '' };
    }
    
    // Split at the found position
    const tempRange = document.createRange();
    tempRange.setStart(splitNode, splitOffset);
    tempRange.setEndAfter(tempDiv.lastChild!);
    
    // Extract the "after" content
    const afterContent = tempRange.extractContents();
    const afterDiv = document.createElement('div');
    afterDiv.appendChild(afterContent);
    
    // The remaining content is the "before"
    let beforeHtml = tempDiv.innerHTML;
    let afterHtml = afterDiv.innerHTML;
    
    // Special handling for headers - if splitting within a header, remove header from new node
    const headerMatch = beforeHtml.match(/<(h[1-6])[^>]*>/);
    if (headerMatch) {
      const headerTag = headerMatch[1];
      // Remove header tags from the after content since headers are "all or none"
      afterHtml = afterHtml.replace(new RegExp(`<${headerTag}[^>]*>(.*?)</${headerTag}>`, 'g'), '$1');
      afterHtml = afterHtml.replace(new RegExp(`</${headerTag}>`, 'g'), '');
      afterHtml = afterHtml.replace(new RegExp(`<${headerTag}[^>]*>`, 'g'), '');
    }
    
    return { currentNodeHtml: beforeHtml, newNodeHtml: afterHtml };
  }

  const dispatch = createEventDispatcher<{
    createNewNode: { 
      afterNodeId: string; 
      nodeType: string; 
      currentContent?: string; 
      newContent?: string; 
    };
    contentChanged: { content: string };
    indentNode: { nodeId: string };
    outdentNode: { nodeId: string };
    navigateArrow: { 
      nodeId: string; 
      direction: 'up' | 'down'; 
      columnHint: number;
    };
  }>();

  // Auto-focus when autoFocus prop is true and initialize content
  $: if (contentEditableElement && !isApplyingHeaderFormatting) {
    // Initialize content if empty but prop has content
    const currentText = contentEditableElement.textContent || '';
    const currentHtml = contentEditableElement.innerHTML || '';
    
    // Reactive content initialization
    
    // Don't interfere if we have header elements - they manage their own content
    const hasHeaders = currentHtml.includes('<h1') || currentHtml.includes('<h2') || 
                      currentHtml.includes('<h3') || currentHtml.includes('<h4') || 
                      currentHtml.includes('<h5') || currentHtml.includes('<h6');
    
    if (hasHeaders) {
      // Skip reactive updates when headers are present to prevent interference
      if (autoFocus) {
        contentEditableElement.focus();
      }
    } else {
      // Only do content initialization if no headers present
      if (!currentText && content) {
        // Check if content is already HTML (from content splitting) or markdown
        if (content.includes('<span class="markdown-')) {
          // Content is already HTML from splitting, use directly
          contentEditableElement.innerHTML = content;
        } else {
          // Content is markdown, convert to HTML
          const htmlContent = markdownToHtml(content);
          contentEditableElement.innerHTML = htmlContent;
        }
      }
      
      // Focus if requested
      if (autoFocus) {
        contentEditableElement.focus();
      }
    }
  }

  // Apply WYSIWYG formatting when content changes (disabled to prevent loops)
  // TODO: Implement WYSIWYG formatting without causing reactive loops
  // $: if (contentEditableElement && content !== undefined) {
  //   applyWYSIWYGFormatting();
  // }

  // Get cursor position in visible text only (ignoring HTML markup)
  function getVisibleTextPosition(): number {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return 0;
    
    const range = selection.getRangeAt(0);
    
    // Walk through all text nodes to find the actual cursor position in visible text
    const textNodes = getTextNodes(contentEditableElement);
    let textPosition = 0;
    
    for (const textNode of textNodes) {
      if (range.startContainer === textNode) {
        return textPosition + range.startOffset;
      }
      
      // Check if cursor is before this text node
      const nodeRange = document.createRange();
      nodeRange.selectNode(textNode);
      
      if (range.compareBoundaryPoints(Range.START_TO_START, nodeRange) < 0) {
        // Cursor is before this text node, return current position
        return textPosition;
      }
      
      textPosition += textNode.textContent?.length || 0;
    }
    
    return textPosition;
  }

  // Legacy method for backward compatibility
  function getCursorPosition(): number {
    return getVisibleTextPosition();
  }

  // Get hierarchy level (number of parent .node-children elements)
  function getHierarchyLevel(): number {
    if (!contentEditableElement) return 0;
    let level = 0;
    let element = contentEditableElement.closest('.node-container');
    
    while (element && element.parentElement) {
      if (element.parentElement.classList.contains('node-children')) {
        level++;
      }
      element = element.parentElement.closest('.node-container');
    }
    
    return level;
  }

  // Get font scaling factor based on node styling
  function getFontScaling(): number {
    if (!contentEditableElement) return 1;
    
    // Look for header classes in the ns-node-container (direct parent of contenteditable)
    const container = contentEditableElement.closest('.ns-node-container');
    if (!container) return 1;
    
    if (container.classList.contains('text-node--h1')) return 2.0;
    if (container.classList.contains('text-node--h2')) return 1.5;
    if (container.classList.contains('text-node--h3')) return 1.25;
    if (container.classList.contains('text-node--h4')) return 1.125;
    if (container.classList.contains('text-node--h5')) return 1.0;
    if (container.classList.contains('text-node--h6')) return 0.875;
    
    return 1.0; // Normal text
  }


  // Navigation Methods Implementation (GitHub Issue #28)
  function canAcceptNavigation(): boolean {
    return true; // MinimalBaseNode accepts navigation
  }

  function enterFromTop(columnHint: number = 0): boolean {
    if (!contentEditableElement) return false;
    
    contentEditableElement.focus();
    
    // Calculate visual position accounting for this node's indentation and font
    const visualPosition = calculateVisualCursorPosition(columnHint, 'first-line');
    setCursorPosition(visualPosition);
    return true;
  }

  function enterFromBottom(columnHint: number = 0): boolean {
    if (!contentEditableElement) return false;
    
    contentEditableElement.focus();
    
    // Calculate visual position accounting for this node's indentation and font
    const visualPosition = calculateVisualCursorPosition(columnHint, 'last-line');
    setCursorPosition(visualPosition);
    return true;
  }

  function exitToTop(): { canExit: boolean; columnPosition: number } {
    if (!contentEditableElement) return { canExit: false, columnPosition: 0 };
    
    const content = contentEditableElement.textContent || '';
    const cursorPosition = getCursorPosition();
    
    const boundaryInfo = isAtNodeBoundary(content, cursorPosition, true);
    
    return {
      canExit: boundaryInfo.isAtBoundary,
      columnPosition: calculateVisualColumnPosition()
    };
  }

  function exitToBottom(): { canExit: boolean; columnPosition: number } {
    if (!contentEditableElement) return { canExit: false, columnPosition: 0 };
    
    const content = contentEditableElement.textContent || '';
    const cursorPosition = getCursorPosition();
    
    const boundaryInfo = isAtNodeBoundary(content, cursorPosition, false);
    
    return {
      canExit: boundaryInfo.isAtBoundary,
      columnPosition: calculateVisualColumnPosition()
    };
  }

  function getCurrentColumn(): number {
    return calculateVisualColumnPosition();
  }

  // Calculate visual column position with fixed assumptions
  function calculateVisualColumnPosition(): number {
    if (!contentEditableElement) return 0;
    
    const content = contentEditableElement.textContent || '';
    const cursorPosition = getVisibleTextPosition();
    
    // Get current line and column in visible text
    const textBeforeCursor = content.substring(0, cursorPosition);
    const currentLineStartIndex = textBeforeCursor.lastIndexOf('\n') + 1;
    const columnInLine = cursorPosition - currentLineStartIndex;
    
    // Fixed assumptions approach
    const hierarchyLevel = getHierarchyLevel();
    const fontScaling = getFontScaling();
    
    // Visual column = logical column + (hierarchy * 4 spaces) + font scaling adjustment
    const indentationOffset = hierarchyLevel * 4;
    const fontAdjustment = Math.round((fontScaling - 1.0) * columnInLine); // Scale the column position
    
    return columnInLine + indentationOffset + fontAdjustment;
  }

  // Calculate cursor position for entry, converting visual column hint to logical position
  function calculateVisualCursorPosition(columnHint: number, lineTarget: 'first-line' | 'last-line'): number {
    if (!contentEditableElement) return 0;
    
    const content = contentEditableElement.textContent || '';
    const lines = content.split('\n');
    
    // Get target line
    const targetLine = lineTarget === 'first-line' ? lines[0] || '' : lines[lines.length - 1] || '';
    const targetLineStart = lineTarget === 'first-line' ? 0 : content.length - targetLine.length;
    
    // Reverse the visual column calculation: convert visual columnHint back to logical position
    const hierarchyLevel = getHierarchyLevel();
    const fontScaling = getFontScaling();
    
    const indentationOffset = hierarchyLevel * 4;
    
    // Remove indentation offset from columnHint to get logical column
    let logicalColumn = Math.max(0, columnHint - indentationOffset);
    
    // Adjust for font scaling (reverse the scaling)
    if (fontScaling !== 1.0) {
      logicalColumn = Math.round(logicalColumn / fontScaling);
    }
    
    // Position within the target line (visible text)
    const positionInLine = Math.min(logicalColumn, targetLine.length);
    
    return targetLineStart + positionInLine;
  }

  function setCursorPosition(position: number): void {
    if (!contentEditableElement) return;
    
    const textNodes = getTextNodes(contentEditableElement);
    let currentOffset = 0;
    
    for (const textNode of textNodes) {
      const nodeLength = textNode.textContent?.length || 0;
      if (currentOffset + nodeLength >= position) {
        const range = document.createRange();
        const selection = window.getSelection();
        
        range.setStart(textNode, Math.max(0, position - currentOffset));
        range.collapse(true);
        selection?.removeAllRanges();
        selection?.addRange(range);
        return;
      }
      currentOffset += nodeLength;
    }
    
    // If position is beyond content, place at end
    if (textNodes.length > 0) {
      const lastNode = textNodes[textNodes.length - 1];
      const range = document.createRange();
      const selection = window.getSelection();
      
      range.setStart(lastNode, lastNode.textContent?.length || 0);
      range.collapse(true);
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
  }

  // Expose navigation methods for external access
  export const navigationMethods: NodeNavigationMethods = {
    canAcceptNavigation,
    enterFromTop,
    enterFromBottom,
    exitToTop,
    exitToBottom,
    getCurrentColumn
  };

  // Handle input to update content prop (convert HTML to markdown for storage)
  function handleInput(event: Event & { currentTarget: HTMLDivElement }) {
    // Regular content update
    const htmlContent = event.currentTarget.innerHTML || '';
    const textContent = event.currentTarget.textContent || '';
    const markdownContent = htmlToMarkdown(htmlContent);
    
    // Only update if content actually changed to avoid reactive loops
    if (markdownContent !== content) {
      content = markdownContent;
      dispatch('contentChanged', { content: markdownContent });
    }
  }

  // Force update content externally (for split content)
  export function updateContent(newContent: string) {
    if (contentEditableElement) {
      contentEditableElement.textContent = newContent;
      content = newContent;
    }
  }

  // Apply header formatting (for TextNode)
  export function applyHeaderFormatting(headerLevel: number) {
    if (!contentEditableElement) return false;
    
    isApplyingHeaderFormatting = true;
    
    // Completely clear the element first
    contentEditableElement.innerHTML = '';
    
    // Create the header element and position cursor
    const headerElement = document.createElement(`h${headerLevel}`);
    const headerTextNode = document.createTextNode('');
    headerElement.appendChild(headerTextNode);
    contentEditableElement.appendChild(headerElement);
    
    // Position cursor inside the header using more direct DOM manipulation
    requestAnimationFrame(() => {
      const range = document.createRange();
      const selection = window.getSelection();
      
      // Position cursor at the start of the text node inside header
      range.setStart(headerTextNode, 0);
      range.setEnd(headerTextNode, 0);
      
      selection?.removeAllRanges();
      selection?.addRange(range);
      
      // Focus the contenteditable element
      contentEditableElement.focus();
      
      // Reset the flag after positioning
      setTimeout(() => {
        isApplyingHeaderFormatting = false;
      }, 50);
    });
    
    // Update the content prop with markdown (will be empty initially)
    const markdownContent = htmlToMarkdown(contentEditableElement.innerHTML);
    content = markdownContent;
    dispatch('contentChanged', { content: markdownContent });
    
    return true;
  }

  // Handle keydown for Enter behavior and keyboard shortcuts
  function handleKeyDown(event: KeyboardEvent) {
    // Handle formatting shortcuts (Ctrl+B, Ctrl+I, Ctrl+U)
    if (event.ctrlKey || event.metaKey) {
      switch (event.key.toLowerCase()) {
        case 'b':
          event.preventDefault();
          applyDirectFormatting('bold');
          return;
        case 'i':
          event.preventDefault();
          applyDirectFormatting('italic');
          return;
        case 'u':
          event.preventDefault();
          applyDirectFormatting('underline');
          return;
      }
    }

    // Handle Tab and Shift+Tab for indentation
    if (event.key === 'Tab') {
      event.preventDefault();
      if (event.shiftKey) {
        // Shift+Tab: Outdent (move to sibling level)
        dispatch('outdentNode', { nodeId });
      } else {
        // Tab: Indent (move to be child of previous sibling)
        dispatch('indentNode', { nodeId });
      }
      return;
    }

    // Handle Arrow Up/Down for node navigation using entry/exit methods
    if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
      const isUpArrow = event.key === 'ArrowUp';
      
      // Check if node can exit in this direction
      const exitInfo = isUpArrow ? exitToTop() : exitToBottom();
      
      if (exitInfo.canExit) {
        // Node allows exit - dispatch navigation event
        event.preventDefault();
        dispatch('navigateArrow', {
          nodeId,
          direction: isUpArrow ? 'up' : 'down',
          columnHint: exitInfo.columnPosition
        });
        return;
      }
      
      // Not at exit boundary - allow normal arrow key behavior within the node
      return;
    }

    if (event.key === 'Enter') {
      // Handle Shift+Enter for line breaks in multiline mode
      if (event.shiftKey && multiline) {
        // Allow default behavior for Shift+Enter (creates line break)
        return;
      }
      
      // Regular Enter key handling
      event.preventDefault();
      
      // Only create new node if allowed
      if (allowNewNodeOnEnter) {
        if (splitContentOnEnter) {
          // Split content at cursor position with HTML formatting carry-over
          const { currentNodeHtml, newNodeHtml } = splitHtmlAtCursor();
          
          // Update current node content immediately
          contentEditableElement.innerHTML = currentNodeHtml;
          const currentMarkdown = htmlToMarkdown(currentNodeHtml);
          content = currentMarkdown;
          dispatch('contentChanged', { content: currentMarkdown });
          
          // Keep new node HTML as-is to preserve formatting during initialization
          // It will be converted to markdown after the new node is initialized
          
          dispatch('createNewNode', { 
            afterNodeId: nodeId, 
            nodeType: nodeType,
            currentContent: currentMarkdown,
            newContent: newNodeHtml // Pass HTML directly to preserve formatting
          });
        } else {
          // Create empty new node
          dispatch('createNewNode', { 
            afterNodeId: nodeId, 
            nodeType: nodeType 
          });
        }
      }
      // If not allowed, Enter key is consumed but nothing happens
    }
  }

</script>

<div class="ns-node-container {className}">
  <!-- SVG icon indicator - positioned relative to first line -->
  <div class="ns-node-indicator">
    <Icon name={displayIcon} size={iconSize} color={nodeColor} />
  </div>
  
  <!-- Minimal contenteditable with unique ID for reliable selection -->
  <div
    id="contenteditable-{nodeId}"
    bind:this={contentEditableElement}
    contenteditable="true"
    role="textbox"
    tabindex="0"
    on:input={handleInput}
    on:keydown={handleKeyDown}
    class="ns-node-content markdown-editor text-foreground {multiline ? 'ns-node-content--multiline markdown-editor--multiline' : 'ns-node-content--singleline markdown-editor--singleline'}"
  ></div>
</div>

<style>
  /* NodeSpace MinimalBaseNode - Design System Architecture */
  
  /* Container using shadcn-svelte design tokens */
  .ns-node-container {
    display: flex;
    align-items: flex-start;
    gap: 1.5rem; /* 24px spacing: 4px + 16px chevron + 4px = room for chevron between circle and text */
    padding: 0.25rem; /* 4px minimal padding for document flow */
  }
  
  /* SVG icon indicator using NodeSpace extension colors */
  .ns-node-indicator {
    flex-shrink: 0;
    margin-top: 0.28rem; /* Align circle center to text middle */
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 16px; /* Accommodate largest size (parent ring) */
    height: 16px;
  }
  
  /* Content area using shadcn-svelte design tokens */
  .ns-node-content {
    flex: 1;
    outline: none;
    border: none;
    border-radius: 0; /* Remove rounded corners */
    background: transparent;
    width: 0; /* Force flex item to respect container width */
    
    /* Typography from design system */
    font-family: 'SF Mono', Monaco, 'Cascadia Code', 'Roboto Mono', Consolas, monospace;
    font-size: 1rem; /* 16px base */
    line-height: 1.6; /* Design system line height */
    /* Color handled by Tailwind text-foreground class */
    
    /* Base contenteditable styling */
    min-height: 1.25rem; /* 20px converted to rem */
    
    /* Ensure empty nodes remain visible and clickable */
    min-width: 2rem; /* 32px minimum width for empty nodes */
  }
  
  /* Empty state styling */
  .ns-node-content:empty::before {
    content: '';
    display: inline-block;
    width: 1px;
    height: 1em;
    background-color: hsl(var(--muted-foreground) / 0.3);
    cursor: text;
  }
  
  
  /* Single-line mode (default) */
  .ns-node-content--singleline {
    white-space: nowrap;
    overflow-x: auto;
    overflow-y: hidden;
  }
  
  /* Multiline mode */
  .ns-node-content--multiline {
    white-space: pre-wrap;
    overflow-y: auto;
    overflow-x: hidden;
    word-wrap: break-word;
    width: 100%;
  }
  
  /* Markdown formatting classes using design system */
  :global(.markdown-bold),
  :global(b),
  :global(strong) {
    font-weight: 700; /* Explicit weight for monospace fonts */
    /* Make bold more visible in monospace */
    text-shadow: 0.5px 0 0 currentColor; /* Subtle text doubling effect */
  }
  
  :global(.markdown-italic) {
    font-style: italic;
  }
  
  :global(.markdown-underline) {
    text-decoration: underline;
  }
</style>