<!--
  Minimal BaseNode - Single-line by default, Enter creates new node
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import Icon, { type IconName } from '$lib/design/icons';

  // Type aliases for DOM APIs to satisfy linter
  type DOMElement = globalThis.Element;
  type DOMNode = globalThis.Node;
  type DOMSelection = globalThis.Selection;

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
  export let canHaveChildren: boolean = true; // Whether this node type can have child nodes

  // Use the canHaveChildren property for future functionality
  $: nodeCapabilities = { canHaveChildren };
  // Export capabilities for parent component access
  $: if (nodeCapabilities) {
    /* Used in reactive context */
  }

  // Generic function to determine if this node can be combined
  // Derived nodes should override this by implementing their own logic
  export let canBeCombined: () => boolean = () => true; // Default: allow combination

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

  // Manual formatting with toggle support - restored working implementation
  function applyManualFormatting(
    formatType: 'bold' | 'italic' | 'underline',
    selection: DOMSelection
  ) {
    const range = selection.getRangeAt(0);

    // Get the start container to check for existing formatting
    const startContainer = range.startContainer;

    // Find if selection is entirely within a formatting span of the target type
    let formatSpan = null;

    // Check if the selection starts and ends within the same formatting span
    let currentNode =
      startContainer.nodeType === globalThis.Node.TEXT_NODE
        ? startContainer.parentElement
        : startContainer;
    while (currentNode && currentNode !== contentEditableElement) {
      if ((currentNode as DOMElement).matches && (currentNode as DOMElement).matches(`span.${formatClasses[formatType]}`)) {
        // Check if the entire selection is within this span
        const spanRange = document.createRange();
        spanRange.selectNodeContents(currentNode);

        if (
          spanRange.comparePoint(range.startContainer, range.startOffset) >= 0 &&
          spanRange.comparePoint(range.endContainer, range.endOffset) <= 0
        ) {
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
      if (!parent) return;

      // Unwrap the span
      const unwrappedNodes = [];
      while (formatSpan.firstChild) {
        const child = formatSpan.firstChild;
        unwrappedNodes.push(child);
        parent.insertBefore(child, formatSpan);
      }
      parent.removeChild(formatSpan);

      // Restore selection using the working approach - no setTimeout needed
      if (selectedText && unwrappedNodes.length > 0) {
        // For single text node (most common case)
        if (
          unwrappedNodes.length === 1 &&
          unwrappedNodes[0].nodeType === globalThis.Node.TEXT_NODE
        ) {
          const textNode = unwrappedNodes[0];
          const newRange = document.createRange();
          newRange.setStart(textNode, 0);
          newRange.setEnd(textNode, textNode.textContent?.length || 0);
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



  // Split HTML content at cursor position with formatting carry-over
  function splitHtmlAtCursor(): { currentNodeHtml: string; newNodeHtml: string } {
    if (!contentEditableElement) {
      return { currentNodeHtml: '', newNodeHtml: '' };
    }

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return { currentNodeHtml: '', newNodeHtml: '' };
    }

    // Create a temporary container to work with ranges
    const tempDiv = document.createElement('div');
    tempDiv.innerHTML = contentEditableElement.innerHTML;

    // Create range in temp container matching cursor position
    const cursorOffset = getCursorPosition();
    const tempRange = document.createRange();

    // Find the text position in the temp container
    let currentOffset = 0;
    let splitNode: DOMNode | null = null;
    let splitOffset = 0;

    const walker = document.createTreeWalker(tempDiv, globalThis.NodeFilter.SHOW_TEXT, null);

    let textNode = walker.nextNode();
    while (textNode) {
      const nodeLength = textNode.textContent?.length || 0;
      if (currentOffset + nodeLength >= cursorOffset) {
        splitNode = textNode;
        splitOffset = cursorOffset - currentOffset;
        break;
      }
      currentOffset += nodeLength;
      textNode = walker.nextNode();
    }

    if (!splitNode) {
      return { currentNodeHtml: contentEditableElement.innerHTML, newNodeHtml: '' };
    }

    // Split at the found position
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
      afterHtml = afterHtml.replace(
        new RegExp(`<${headerTag}[^>]*>(.*?)</${headerTag}>`, 'g'),
        '$1'
      );
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
    requestNodeJoining: {
      currentNodeId: string;
      currentContent: string;
      currentNodeType: string;
      canBeCombined: boolean;
    };
  }>();

  // Content update handling - allows external content changes (like merging) but avoids typing interference
  let lastContentProp = '';
  let isExternalUpdate = false;
  
  $: if (contentEditableElement && !isApplyingHeaderFormatting) {
    // Only update if the content prop has actually changed from external source (not from user typing)
    if (content !== lastContentProp) {
      const currentText = contentEditableElement.textContent || '';
      const currentHtml = contentEditableElement.innerHTML || '';
      
      // Convert current DOM to markdown to compare with incoming content
      const currentMarkdown = htmlToMarkdown(currentHtml);
      
      // Only override DOM if this is truly an external change (like node merging)
      // Don't override if user is in the middle of typing/formatting
      if (content !== currentMarkdown && content !== currentText) {
        isExternalUpdate = true; // Flag to prevent reactive loops
        
        // Check if content is already HTML (from content splitting) or markdown
        if (content.includes('<span class="markdown-')) {
          // Content is already HTML from splitting, use directly
          contentEditableElement.innerHTML = content;
        } else {
          // Content is markdown, convert to HTML
          const htmlContent = markdownToHtml(content);
          contentEditableElement.innerHTML = htmlContent;
        }
        
        // Reset flag after DOM update
        setTimeout(() => { isExternalUpdate = false; }, 0);
      }
      
      lastContentProp = content; // Track the last prop value we processed
    }
    
    // Handle autoFocus
    if (autoFocus) {
      contentEditableElement.focus();
    }
  }

  // Apply WYSIWYG formatting when content changes (disabled to prevent loops)
  // TODO: Implement WYSIWYG formatting without causing reactive loops
  // $: if (contentEditableElement && content !== undefined) {
  //   applyWYSIWYGFormatting();
  // }

  // Get cursor position in contenteditable
  function getCursorPosition(): number {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return 0;

    const range = selection.getRangeAt(0);
    const preCaretRange = range.cloneRange();
    preCaretRange.selectNodeContents(contentEditableElement);
    preCaretRange.setEnd(range.startContainer, range.startOffset);

    return preCaretRange.toString().length;
  }

  // Handle input to update content prop (convert HTML to markdown for storage)
  function handleInput(event: Event & { currentTarget: HTMLDivElement }) {
    // Skip input handling if this is from an external update (like node merging)
    if (isExternalUpdate) return;
    
    // Regular content update
    const htmlContent = event.currentTarget.innerHTML || '';
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
    const textNode = document.createTextNode('');
    headerElement.appendChild(textNode);
    contentEditableElement.appendChild(headerElement);

    // Position cursor inside the header using more direct DOM manipulation
    requestAnimationFrame(() => {
      const range = document.createRange();
      const selection = window.getSelection();

      // Position cursor at the start of the text node inside header
      range.setStart(textNode, 0);
      range.setEnd(textNode, 0);

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

    // Handle Backspace for node joining
    if (event.key === 'Backspace') {
      const cursorPosition = getCursorPosition();
      if (cursorPosition === 0) {
        handleAdvancedBackspace(event);
      }
      return;
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

  // Advanced backspace handler for node joining
  function handleAdvancedBackspace(event: KeyboardEvent) {
    event.preventDefault(); // Prevent default backspace behavior
    
    // Dispatch request for previous node information
    dispatch('requestNodeJoining', {
      currentNodeId: nodeId,
      currentContent: content,
      currentNodeType: nodeType,
      canBeCombined: canBeCombined() // Use the generic function provided by derived node
    });
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
    class="ns-node-content markdown-editor text-foreground {multiline
      ? 'ns-node-content--multiline markdown-editor--multiline'
      : 'ns-node-content--singleline markdown-editor--singleline'}"
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
    margin-top: 0.29rem; /* Align circle center to text middle - moved down slightly */
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
