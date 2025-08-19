<!--
  TextNode Component - Enhanced with ContentProcessor for dual-representation
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import MinimalBaseNode from '$lib/design/components/MinimalBaseNode.svelte';
  import type { NodeNavigationMethods } from '$lib/types/navigation.js';
  import { contentProcessor } from '$lib/services/contentProcessor.js';

  // Minimal props
  export let nodeId: string;
  export let autoFocus: boolean = false;
  export let content: string = '';
  export let inheritHeaderLevel: number = 0; // Header level inherited from parent node
  export let children: any[] = []; // Passthrough for MinimalBaseNode

  // Internal reactive state that tracks content changes
  let internalContent: string = content;

  // Header state for CSS styling - initialize with inherited level
  let headerLevel: number = inheritHeaderLevel;

  // Sync internalContent when content prop changes (reactive to parent updates)
  $: internalContent = content;

  // Parse header level from content using ContentProcessor
  $: {
    const detectedHeaderLevel = contentProcessor.parseHeaderLevel(internalContent);
    if (detectedHeaderLevel > 0) {
      headerLevel = detectedHeaderLevel;
      displayContent = contentProcessor.stripHeaderSyntax(internalContent);
    } else {
      headerLevel = inheritHeaderLevel;
      displayContent = internalContent;
    }
  }

  // Content to display (without markdown header syntax)
  let displayContent: string = internalContent;
  let baseNodeRef: any;

  // Expose navigation methods from MinimalBaseNode
  export const navigationMethods: NodeNavigationMethods = {
    canAcceptNavigation: () => baseNodeRef?.navigationMethods?.canAcceptNavigation() ?? true,
    enterFromTop: (columnHint?: number) =>
      baseNodeRef?.navigationMethods?.enterFromTop(columnHint) ?? false,
    enterFromBottom: (columnHint?: number) =>
      baseNodeRef?.navigationMethods?.enterFromBottom(columnHint) ?? false,
    exitToTop: () =>
      baseNodeRef?.navigationMethods?.exitToTop() ?? { canExit: false, columnPosition: 0 },
    exitToBottom: () =>
      baseNodeRef?.navigationMethods?.exitToBottom() ?? { canExit: false, columnPosition: 0 },
    getCurrentColumn: () => baseNodeRef?.navigationMethods?.getCurrentColumn() ?? 0
  };

  const dispatch = createEventDispatcher<{
    createNewNode: {
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      inheritHeaderLevel?: number; // Header level to inherit
    };
    contentChanged: { nodeId: string; content: string };
    indentNode: { nodeId: string };
    outdentNode: { nodeId: string };
    navigateArrow: {
      nodeId: string;
      direction: 'up' | 'down';
      columnHint: number;
    };
    combineWithPrevious: {
      nodeId: string;
      currentContent: string;
    };
    deleteNode: {
      nodeId: string;
    };
  }>();

  // Forward the createNewNode event from BaseNode with header inheritance
  function handleCreateNewNode(
    event: CustomEvent<{
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
    }>
  ) {
    // Preserve header formatting in current node when splitting
    let preservedCurrentContent = event.detail.currentContent;
    if (headerLevel > 0 && preservedCurrentContent !== undefined) {
      // If we had a header and the current content doesn't start with #, restore it
      if (!preservedCurrentContent.startsWith('#')) {
        // Only add header prefix if there's actual content or if content is empty (pure header)
        preservedCurrentContent = preservedCurrentContent.trim()
          ? `${'#'.repeat(headerLevel)} ${preservedCurrentContent}`
          : `${'#'.repeat(headerLevel)} `;
      }
    }

    // Add header level inheritance for new nodes
    const eventDetail = {
      ...event.detail,
      currentContent: preservedCurrentContent,
      inheritHeaderLevel: headerLevel // Pass current header level to new node
    };

    dispatch('createNewNode', eventDetail);
  }

  // Handle content changes with ContentProcessor validation
  function handleContentChange(newContent: string) {
    // If content becomes empty, reset header level
    if (!newContent.trim()) {
      headerLevel = 0;
      internalContent = '';
      dispatch('contentChanged', { nodeId, content: '' });
      return;
    }

    // Validate and sanitize content
    const sanitizedContent = contentProcessor.sanitizeContent(newContent);
    const validation = contentProcessor.validateContent(sanitizedContent);

    // Log validation warnings but don't block content
    if (validation.warnings.length > 0) {
      console.warn('Content validation warnings:', validation.warnings);
    }

    // If we have a header level, store with markdown header syntax
    // But also update displayContent for immediate UI feedback
    const finalContent =
      headerLevel > 0 ? `${'#'.repeat(headerLevel)} ${sanitizedContent}` : sanitizedContent;
    internalContent = finalContent;
    displayContent = sanitizedContent; // Update display without # symbols
    dispatch('contentChanged', { nodeId, content: finalContent });
  }

  // Reactive className computation
  $: nodeClassName = `text-node ${headerLevel ? `text-node--h${headerLevel}` : ''}`.trim();

  // TextNode-specific combination logic - determine if this node can be combined with others
  function canBeCombined(): boolean {
    if (internalContent.startsWith('#')) {
      const headerMatch = internalContent.match(/^#{1,6}\s/);
      if (headerMatch) return false;
    }
    return true;
  }

  // TextNode-specific Enter key handling
  function handleTextNodeKeyDown(event: KeyboardEvent) {
    if (event.key === 'Enter') {
      // Headers: block Shift+Enter, allow regular Enter to create new nodes
      if (headerLevel > 0) {
        if (event.shiftKey) {
          // Block Shift+Enter in headers
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        // Regular Enter in headers: let MinimalBaseNode handle new node creation
        return;
      }

      // Non-header text nodes: allow Shift+Enter for manual line breaks
      if (event.shiftKey) {
        // Allow Shift+Enter to create line breaks in non-header text
        return;
      }

      // Regular Enter: let MinimalBaseNode handle new node creation
      return;
    }

    // Handle header syntax detection on space key
    if (event.key === ' ') {
      // Get the MinimalBaseNode's contenteditable element
      const baseNodeElement = event.target as HTMLElement;
      const contentEditableElement = baseNodeElement.closest('.markdown-editor') || baseNodeElement;

      if (contentEditableElement && contentEditableElement.textContent) {
        const currentText = contentEditableElement.textContent;
        const selection = window.getSelection();

        if (selection && selection.rangeCount > 0) {
          const range = selection.getRangeAt(0);
          const preCaretRange = range.cloneRange();
          preCaretRange.selectNodeContents(contentEditableElement);
          preCaretRange.setEnd(range.startContainer, range.startOffset);
          const cursorPosition = preCaretRange.toString().length;

          const textBeforeCursor = currentText.substring(0, cursorPosition);

          // Check if we're about to complete header syntax using ContentProcessor
          if (/^#{1,6}$/.test(textBeforeCursor)) {
            event.preventDefault();
            event.stopPropagation();
            event.stopImmediatePropagation();

            // Set header level for CSS styling using ContentProcessor
            const detectedHeaderLevel = contentProcessor.parseHeaderLevel(textBeforeCursor + ' ');
            headerLevel = detectedHeaderLevel;

            // Preserve existing content after the cursor when switching header levels
            const freshElement = document.getElementById(`contenteditable-${nodeId}`);
            if (freshElement) {
              const fullText = freshElement.textContent || '';
              // After typing "## ", cursor position is at the end of "##"
              // We want to preserve everything after the "##" part (not after the space)
              const textAfterSpace = fullText.substring(cursorPosition);

              // Set content to just the text after the header syntax
              freshElement.textContent = textAfterSpace;

              // Position cursor at the beginning for continued typing
              const range = document.createRange();
              const selection = window.getSelection();

              if (freshElement.firstChild) {
                range.setStart(freshElement.firstChild, 0);
                range.setEnd(freshElement.firstChild, 0);
              } else {
                // If no text content, position at the element
                range.selectNodeContents(freshElement);
                range.collapse(true);
              }

              selection?.removeAllRanges();
              selection?.addRange(range);
              freshElement.focus();
            }

            return;
          }
        }
      }
    }
  }
</script>

<div class="text-node-container" role="textbox" tabindex="0">
  <MinimalBaseNode
    bind:this={baseNodeRef}
    {nodeId}
    nodeType="text"
    {autoFocus}
    content={displayContent}
    {children}
    allowNewNodeOnEnter={true}
    splitContentOnEnter={true}
    multiline={true}
    {headerLevel}
    className={nodeClassName}
    {canBeCombined}
    on:createNewNode={handleCreateNewNode}
    on:contentChanged={(e) => handleContentChange(e.detail.content)}
    on:indentNode={(e) => dispatch('indentNode', e.detail)}
    on:outdentNode={(e) => dispatch('outdentNode', e.detail)}
    on:navigateArrow={(e) => dispatch('navigateArrow', e.detail)}
    on:combineWithPrevious={(e) => dispatch('combineWithPrevious', e.detail)}
    on:deleteNode={(e) => dispatch('deleteNode', e.detail)}
    on:keydown={handleTextNodeKeyDown}
  />
</div>

<style>
  /* CSS-based header styling for entire contenteditable element */
  :global(.text-node--h1 .markdown-editor) {
    font-size: 2rem;
    font-weight: bold;
    line-height: 1.2;
    margin: 0;
  }

  /* Fix empty state cursor for headers - use consistent size instead of scaling */
  :global(.text-node--h1 .ns-node-content.markdown-editor:empty::before),
  :global(.text-node--h2 .ns-node-content.markdown-editor:empty::before),
  :global(.text-node--h3 .ns-node-content.markdown-editor:empty::before),
  :global(.text-node--h4 .ns-node-content.markdown-editor:empty::before),
  :global(.text-node--h5 .ns-node-content.markdown-editor:empty::before),
  :global(.text-node--h6 .ns-node-content.markdown-editor:empty::before) {
    height: 1.25rem !important; /* Fixed height instead of 1em that scales */
    background-color: hsl(var(--muted-foreground) / 0.2) !important; /* Slightly more subtle */
  }

  :global(.text-node--h2 .markdown-editor) {
    font-size: 1.5rem;
    font-weight: bold;
    line-height: 1.3;
    margin: 0;
  }

  :global(.text-node--h3 .markdown-editor) {
    font-size: 1.25rem;
    font-weight: bold;
    line-height: 1.4;
    margin: 0;
  }

  :global(.text-node--h4 .markdown-editor) {
    font-size: 1.125rem;
    font-weight: bold;
    line-height: 1.4;
    margin: 0;
  }

  :global(.text-node--h5 .markdown-editor) {
    font-size: 1rem;
    font-weight: bold;
    line-height: 1.4;
    margin: 0;
  }

  :global(.text-node--h6 .markdown-editor) {
    font-size: 0.875rem;
    font-weight: bold;
    line-height: 1.4;
    margin: 0;
  }

  /* Headers should wrap text properly to avoid horizontal scrolling */
  :global(.text-node--h1 .markdown-editor),
  :global(.text-node--h2 .markdown-editor),
  :global(.text-node--h3 .markdown-editor),
  :global(.text-node--h4 .markdown-editor),
  :global(.text-node--h5 .markdown-editor),
  :global(.text-node--h6 .markdown-editor) {
    white-space: pre-wrap !important;
    overflow-x: visible !important;
    overflow-y: visible !important;
    word-wrap: break-word !important;
    width: 100% !important;
  }
</style>
