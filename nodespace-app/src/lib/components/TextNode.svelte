<!--
  TextNode Component - Minimal wrapper around BaseNode
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import MinimalBaseNode from '$lib/design/components/MinimalBaseNode.svelte';
  import type { NodeNavigationMethods } from '$lib/types/navigation.js';

  // Minimal props
  export let nodeId: string;
  export let autoFocus: boolean = false;
  export let content: string = '';
  export let inheritHeaderLevel: number = 0; // Header level inherited from parent node
  export let children: any[] = []; // Passthrough for MinimalBaseNode
  
  // Header state for CSS styling - initialize with inherited level
  let headerLevel: number = inheritHeaderLevel;
  
  // Parse header level from content and get display text
  $: {
    if (content.startsWith('#')) {
      const headerMatch = content.match(/^(#{1,6})\s+(.*)$/);
      if (headerMatch) {
        headerLevel = headerMatch[1].length;
        displayContent = headerMatch[2];
      } else {
        headerLevel = inheritHeaderLevel;
        displayContent = content;
      }
    } else {
      headerLevel = inheritHeaderLevel;
      displayContent = content;
    }
  }
  
  // Content to display (without markdown header syntax)
  let displayContent: string = content;
  let baseNodeRef: any;

  // Expose navigation methods from MinimalBaseNode
  export const navigationMethods: NodeNavigationMethods = {
    canAcceptNavigation: () => baseNodeRef?.navigationMethods?.canAcceptNavigation() ?? true,
    enterFromTop: (columnHint?: number) => baseNodeRef?.navigationMethods?.enterFromTop(columnHint) ?? false,
    enterFromBottom: (columnHint?: number) => baseNodeRef?.navigationMethods?.enterFromBottom(columnHint) ?? false,
    exitToTop: () => baseNodeRef?.navigationMethods?.exitToTop() ?? { canExit: false, columnPosition: 0 },
    exitToBottom: () => baseNodeRef?.navigationMethods?.exitToBottom() ?? { canExit: false, columnPosition: 0 },
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
  }>();

  // Forward the createNewNode event from BaseNode with header inheritance
  function handleCreateNewNode(event: CustomEvent<{ 
    afterNodeId: string; 
    nodeType: string; 
    currentContent?: string; 
    newContent?: string; 
  }>) {
    // Add header level inheritance for new nodes
    const eventDetail = {
      ...event.detail,
      inheritHeaderLevel: headerLevel // Pass current header level to new node
    };
    
    dispatch('createNewNode', eventDetail);
  }

  // Handle content changes and notify parent
  function handleContentChange(newContent: string) {
    // If content becomes empty, reset header level
    if (!newContent.trim()) {
      headerLevel = 0;
      content = '';
      dispatch('contentChanged', { nodeId, content: '' });
      return;
    }
    
    // If we have a header level, store with markdown header syntax
    // But also update displayContent for immediate UI feedback
    const finalContent = headerLevel > 0 ? `${'#'.repeat(headerLevel)} ${newContent}` : newContent;
    content = finalContent;
    displayContent = newContent; // Update display without # symbols
    dispatch('contentChanged', { nodeId, content: finalContent });
  }

  // Reactive className computation
  $: nodeClassName = `text-node ${headerLevel ? `text-node--h${headerLevel}` : ''}`.trim();

  // TextNode-specific combination logic - determine if this node can be combined with others
  function canBeCombined(): boolean {
    if (content.startsWith('#')) {
      const headerMatch = content.match(/^#{1,6}\s/);
      if (headerMatch) return false;
    }
    return true;
  }

  // TextNode-specific header functionality with CSS approach
  function handleTextNodeKeyDown(event: KeyboardEvent) {
    // Prevent Shift+Enter in header mode (headers are single-line only)
    if (event.key === 'Enter' && event.shiftKey && headerLevel > 0) {
      event.preventDefault();
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
          
          // Check if we're about to complete header syntax
          if (/^#{1,6}$/.test(textBeforeCursor)) {
            event.preventDefault();
            event.stopPropagation();
            event.stopImmediatePropagation();
            
            // Set header level for CSS styling
            const detectedHeaderLevel = textBeforeCursor.length;
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

<div class="text-node-container" role="textbox" tabindex="0" on:keydown={handleTextNodeKeyDown}>
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
    className={nodeClassName}
    {canBeCombined}
    on:createNewNode={handleCreateNewNode}
    on:contentChanged={(e) => handleContentChange(e.detail.content)}
    on:indentNode={(e) => dispatch('indentNode', e.detail)}
    on:outdentNode={(e) => dispatch('outdentNode', e.detail)}
    on:navigateArrow={(e) => dispatch('navigateArrow', e.detail)}
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

