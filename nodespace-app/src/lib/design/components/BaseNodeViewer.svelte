<!--
  BaseNodeViewer - Container that manages a collection of nodes
  Handles node creation, deletion, and organization
-->

<script lang="ts">
  import TextNode from '$lib/components/TextNode.svelte';
  import Icon from '$lib/design/icons';
  import { htmlToMarkdown } from '$lib/utils/markdown.js';
  import { ReactiveNodeManager } from '$lib/services/ReactiveNodeManager.svelte';
  import { type NodeManagerEvents } from '$lib/services/NodeManager';
  import { demoNodes } from './DemoData';

  // Demo data imported from external file

  // NodeManager setup with event callbacks
  const nodeManagerEvents: NodeManagerEvents = {
    focusRequested: (nodeId: string, position?: number) => {
      // Use setTimeout to ensure DOM content has been updated before cursor positioning
      setTimeout(() => {
        const element = document.getElementById(`contenteditable-${nodeId}`);
        if (element) {
          element.focus();
          if (position !== undefined) {
            setCursorAtPosition(element, position);
          }
        }
      }, 0);
    },
    hierarchyChanged: () => {
      // Reactivity is handled automatically by Svelte 5 $state
    },
    nodeCreated: () => {
      // Additional logic for node creation if needed
    },
    nodeDeleted: () => {
      // Additional logic for node deletion if needed
    }
  };

  const nodeManager = new ReactiveNodeManager(nodeManagerEvents);

  // Initialize NodeManager with demo data
  nodeManager.initializeFromLegacyData(demoNodes);

  // Handle creating new nodes when Enter is pressed
  function handleCreateNewNode(
    event: CustomEvent<{
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      inheritHeaderLevel?: number;
    }>
  ) {
    const { afterNodeId, nodeType, currentContent, newContent, inheritHeaderLevel } = event.detail;

    // Update current node content if provided
    if (currentContent !== undefined) {
      nodeManager.updateNodeContent(afterNodeId, currentContent);
    }

    // Create new node using NodeManager with header inheritance
    const newNodeId = nodeManager.createNode(
      afterNodeId,
      newContent || '',
      nodeType,
      inheritHeaderLevel
    );

    // Handle HTML formatting conversion if needed
    if (newContent && newContent.includes('<span class="markdown-')) {
      setTimeout(() => {
        const markdownContent = htmlToMarkdown(newContent);
        nodeManager.updateNodeContent(newNodeId, markdownContent);
      }, 100);
    }
  }

  // Handle indenting nodes (Tab key)
  function handleIndentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;

    // Store cursor position before DOM changes
    const cursorPosition = saveCursorPosition(nodeId);

    // Use NodeManager to handle indentation
    const success = nodeManager.indentNode(nodeId);

    if (success) {
      // Restore cursor position after DOM update
      setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
    }
  }

  // Cursor position utilities
  function saveCursorPosition(nodeId: string): number {
    const element = document.getElementById(`contenteditable-${nodeId}`);
    if (!element) return 0;

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return 0;

    const range = selection.getRangeAt(0);
    const preCaretRange = range.cloneRange();
    preCaretRange.selectNodeContents(element);
    preCaretRange.setEnd(range.startContainer, range.startOffset);

    return preCaretRange.toString().length;
  }

  function restoreCursorPosition(nodeId: string, position: number) {
    const element = document.getElementById(`contenteditable-${nodeId}`);
    if (!element) return;

    try {
      const selection = window.getSelection();
      if (!selection) return;

      const range = document.createRange();
      const textNodes = getTextNodes(element);

      let currentOffset = 0;
      for (const textNode of textNodes) {
        const nodeLength = textNode.textContent?.length || 0;
        if (currentOffset + nodeLength >= position) {
          range.setStart(textNode, Math.max(0, position - currentOffset));
          range.collapse(true);
          selection.removeAllRanges();
          selection.addRange(range);
          element.focus();
          return;
        }
        currentOffset += nodeLength;
      }

      // If we couldn't find the exact position, place cursor at end
      if (textNodes.length > 0) {
        const lastNode = textNodes[textNodes.length - 1];
        range.setStart(lastNode, lastNode.textContent?.length || 0);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
        element.focus();
      }
    } catch {
      // Silently handle cursor restoration errors
    }
  }

  function getTextNodes(element: Element): Text[] {
    const textNodes: Text[] = [];
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);

    let node = walker.nextNode();
    while (node) {
      textNodes.push(node as Text);
      node = walker.nextNode();
    }

    return textNodes;
  }

  // Handle outdenting nodes (Shift+Tab key)
  function handleOutdentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;

    // Store cursor position before DOM changes
    const cursorPosition = saveCursorPosition(nodeId);

    // Use NodeManager to handle outdentation
    const success = nodeManager.outdentNode(nodeId);

    if (success) {
      // Restore cursor position after DOM update
      setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
    }
  }

  // Handle chevron click to toggle expand/collapse
  function handleToggleExpanded(nodeId: string) {
    nodeManager.toggleExpanded(nodeId);
  }

  // Get node type color from design system
  function getNodeColor(nodeType: string): string {
    return `hsl(var(--node-${nodeType}, var(--node-text)))`;
  }

  // Handle arrow key navigation between nodes using entry/exit methods
  function handleArrowNavigation(
    event: CustomEvent<{
      nodeId: string;
      direction: 'up' | 'down';
      columnHint: number;
    }>
  ) {
    const { nodeId, direction, columnHint } = event.detail;

    // Get visible nodes from NodeManager
    const visibleNodes = nodeManager.getVisibleNodes();
    const currentIndex = visibleNodes.findIndex((n) => n.id === nodeId);

    if (currentIndex === -1) return;

    // Find next navigable node that accepts navigation
    let targetIndex = direction === 'up' ? currentIndex - 1 : currentIndex + 1;

    while (targetIndex >= 0 && targetIndex < visibleNodes.length) {
      const candidateNode = visibleNodes[targetIndex];

      // Check if this node accepts navigation (skip if it doesn't)
      // For now, assume all nodes accept navigation (will be refined per node type)
      const acceptsNavigation = true; // candidateNode.navigationMethods?.canAcceptNavigation() ?? true;

      if (acceptsNavigation) {
        // Found a node that accepts navigation - try to enter it
        const success = enterNodeAtPosition(candidateNode.id, direction, columnHint);
        if (success) return;
      }

      // This node doesn't accept navigation or entry failed - try next one
      targetIndex = direction === 'up' ? targetIndex - 1 : targetIndex + 1;
    }
  }

  // Enter a node using its entry methods or fallback positioning
  function enterNodeAtPosition(
    targetNodeId: string,
    direction: 'up' | 'down',
    columnHint: number
  ): boolean {
    // Try component-level navigation methods first (future enhancement)
    // const nodeComponent = getNodeComponent(targetNodeId);
    // if (nodeComponent?.navigationMethods) {
    //   return direction === 'up'
    //     ? nodeComponent.navigationMethods.enterFromBottom(columnHint)
    //     : nodeComponent.navigationMethods.enterFromTop(columnHint);
    // }

    // Fallback: direct DOM entry point positioning
    setTimeout(() => {
      const targetElement = document.getElementById(`contenteditable-${targetNodeId}`);
      if (!targetElement) return;

      targetElement.focus();

      // Entry point logic: nodes define where cursor should land when entered
      const content = targetElement.textContent || '';
      const lines = content.split('\n');
      let targetPosition: number;

      if (direction === 'up') {
        // Entering from bottom: convert visual columnHint to logical position
        const lastLine = lines[lines.length - 1] || '';
        const lastLineStart = content.length - lastLine.length;

        // Apply same fixed assumptions as MinimalBaseNode
        // Get hierarchy level for target node
        let hierarchyLevel = 0;
        let element = targetElement.closest('.node-container');
        while (element && element.parentElement) {
          if (element.parentElement.classList.contains('node-children')) {
            hierarchyLevel++;
          }
          element = element.parentElement.closest('.node-container');
        }

        // Get font scaling for target node
        let fontScaling = 1.0;
        const nsContainer = targetElement.closest('.ns-node-container');
        if (nsContainer) {
          if (nsContainer.classList.contains('text-node--h1')) fontScaling = 2.0;
          else if (nsContainer.classList.contains('text-node--h2')) fontScaling = 1.5;
          else if (nsContainer.classList.contains('text-node--h3')) fontScaling = 1.25;
          else if (nsContainer.classList.contains('text-node--h4')) fontScaling = 1.125;
        }

        // Convert visual columnHint back to logical position
        const indentationOffset = hierarchyLevel * 4;
        let logicalColumn = Math.max(0, columnHint - indentationOffset);

        // Adjust for font scaling (reverse the scaling)
        if (fontScaling !== 1.0) {
          logicalColumn = Math.round(logicalColumn / fontScaling);
        }

        targetPosition = lastLineStart + Math.min(logicalColumn, lastLine.length);
      } else {
        // Entering from top: convert visual columnHint to logical position
        const firstLine = lines[0] || '';

        // Apply same fixed assumptions as MinimalBaseNode
        // Get hierarchy level for target node
        let hierarchyLevel = 0;
        let element = targetElement.closest('.node-container');
        while (element && element.parentElement) {
          if (element.parentElement.classList.contains('node-children')) {
            hierarchyLevel++;
          }
          element = element.parentElement.closest('.node-container');
        }

        // Get font scaling for target node
        let fontScaling = 1.0;
        const nsContainer = targetElement.closest('.ns-node-container');
        if (nsContainer) {
          if (nsContainer.classList.contains('text-node--h1')) fontScaling = 2.0;
          else if (nsContainer.classList.contains('text-node--h2')) fontScaling = 1.5;
          else if (nsContainer.classList.contains('text-node--h3')) fontScaling = 1.25;
          else if (nsContainer.classList.contains('text-node--h4')) fontScaling = 1.125;
        }

        // Convert visual columnHint back to logical position
        const indentationOffset = hierarchyLevel * 4;
        let logicalColumn = Math.max(0, columnHint - indentationOffset);

        // Adjust for font scaling (reverse the scaling)
        if (fontScaling !== 1.0) {
          logicalColumn = Math.round(logicalColumn / fontScaling);
        }

        targetPosition = Math.min(logicalColumn, firstLine.length);
      }

      // Set cursor at entry point
      setCursorAtPosition(targetElement, targetPosition);
    }, 0);

    return true;
  }

  // Utility to set cursor position in any contenteditable element
  function setCursorAtPosition(element: HTMLElement, position: number): void {
    const selection = window.getSelection();
    if (!selection) return;

    try {
      const range = document.createRange();
      const textNodes = getTextNodes(element);

      let currentOffset = 0;
      for (const textNode of textNodes) {
        const nodeLength = textNode.textContent?.length || 0;
        if (currentOffset + nodeLength >= position) {
          range.setStart(textNode, Math.max(0, position - currentOffset));
          range.collapse(true);
          selection.removeAllRanges();
          selection.addRange(range);
          return;
        }
        currentOffset += nodeLength;
      }

      // Position beyond content - place at end
      if (textNodes.length > 0) {
        const lastNode = textNodes[textNodes.length - 1];
        range.setStart(lastNode, lastNode.textContent?.length || 0);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      }
    } catch {
      // Cursor positioning failed silently
    }
  }

  // Handle combining current node with previous node (Backspace at start of node)
  // CLEAN DELEGATION: All logic handled by NodeManager
  function handleCombineWithPrevious(
    event: CustomEvent<{ nodeId: string; currentContent: string }>
  ) {
    console.log('DEBUG: handleCombineWithPrevious called with:', event.detail);
    const { nodeId, currentContent } = event.detail;
    const visibleNodes = nodeManager.getVisibleNodes();
    console.log('DEBUG: visibleNodes count:', visibleNodes.length);
    const currentIndex = visibleNodes.findIndex((n) => n.id === nodeId);
    console.log('DEBUG: currentIndex:', currentIndex);

    if (currentIndex <= 0) {
      console.log('DEBUG: No previous node to combine with, exiting');
      return; // No previous node to combine with
    }

    const previousNode = visibleNodes[currentIndex - 1];
    console.log('DEBUG: previousNode:', previousNode.id, 'content:', JSON.stringify(previousNode.content));

    if (currentContent.trim() === '') {
      console.log('DEBUG: Empty content - deleting node and focusing previous');
      // Empty node - delete and focus previous at end
      nodeManager.deleteNode(nodeId);
      nodeManagerEvents.focusRequested(previousNode.id, previousNode.content.length);
    } else {
      console.log('DEBUG: Combining nodes - calling nodeManager.combineNodes');
      // Combine nodes - NodeManager handles focus automatically
      nodeManager.combineNodes(nodeId, previousNode.id);
    }
  }

  // Handle deleting empty node (Backspace at start of empty node)
  function handleDeleteNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;
    const visibleNodes = nodeManager.getVisibleNodes();
    const currentIndex = visibleNodes.findIndex((n) => n.id === nodeId);

    if (currentIndex <= 0) return; // No previous node to focus

    const previousNode = visibleNodes[currentIndex - 1];

    // Delete node and focus previous at end
    nodeManager.deleteNode(nodeId);
    nodeManagerEvents.focusRequested(previousNode.id, previousNode.content.length);
  }

  // Helper functions removed - NodeManager handles all node operations
</script>

<div class="node-viewer">
  {#each nodeManager.visibleNodes as node (node.id)}
    <div
      class="node-container"
      data-has-children={node.children?.length > 0}
      style="margin-left: {(node.hierarchyDepth || 0) * 2.5}rem"
    >
      <div class="node-content-wrapper">
        <!-- Chevron for parent nodes (only visible on hover) -->
        {#if node.children && node.children.length > 0}
          <button
            class="node-chevron"
            class:expanded={node.expanded}
            onclick={() => handleToggleExpanded(node.id)}
            aria-label={node.expanded ? 'Collapse' : 'Expand'}
          >
            <Icon name="chevron-right" size={16} color={getNodeColor(node.nodeType)} />
          </button>
        {:else}
          <!-- Spacer for nodes without children to maintain alignment -->
          <div class="node-chevron-spacer"></div>
        {/if}

        {#if node.nodeType === 'text'}
          <TextNode
            nodeId={node.id}
            autoFocus={node.autoFocus}
            content={node.content}
            inheritHeaderLevel={node.inheritHeaderLevel}
            children={node.children}
            on:createNewNode={handleCreateNewNode}
            on:indentNode={handleIndentNode}
            on:outdentNode={handleOutdentNode}
            on:navigateArrow={handleArrowNavigation}
            on:contentChanged={(e) => {
              // Use NodeManager to update content
              nodeManager.updateNodeContent(node.id, e.detail.content);
            }}
            on:combineWithPrevious={handleCombineWithPrevious}
            on:deleteNode={handleDeleteNode}
          />
        {/if}
      </div>
    </div>
  {/each}
</div>

<style>
  .node-viewer {
    /* Container for nodes - document-like spacing */
    display: flex;
    flex-direction: column;
    gap: 0; /* 0px gap - all spacing from node padding for 8px total */
    padding-left: 2rem; /* Add left padding to accommodate chevrons positioned at negative left */

    /* Node indentation system - defines consistent spacing */
    --node-indent: 2.5rem; /* 40px indentation between parent and child circles */

    /* NodeSpace Extension Colors - Node type colors per design system */
    --node-text: 142 71% 45%; /* Green for text nodes */
    --node-task: 25 95% 53%; /* Orange for task nodes */
    --node-ai-chat: 221 83% 53%; /* Blue for AI chat nodes */
    --node-entity: 271 81% 56%; /* Purple for entity nodes */
    --node-query: 330 81% 60%; /* Pink for query nodes */
  }

  .node-container {
    /* Individual node wrapper - no additional spacing */
    /* Allow chevrons to extend outside container bounds */
    overflow: visible;
  }

  .node-content-wrapper {
    /* Wrapper for chevron + content */
    display: flex;
    align-items: flex-start;
    gap: 0.25rem; /* 4px gap between chevron/spacer and text content */
    position: relative; /* Enable absolute positioning for chevrons */
  }

  /* Chevron styling - anchored to circle position */
  .node-chevron {
    opacity: 0; /* Hidden by default - shows on hover */
    background: none;
    border: none;
    padding: 0.125rem; /* 2px padding for clickable area */
    cursor: pointer;
    border-radius: 0.125rem; /* 2px border radius */
    transition: opacity 0.15s ease-in-out; /* Smooth fade in/out */
    transform-origin: center;
    pointer-events: auto; /* Ensure chevron always receives pointer events */
    flex-shrink: 0;
    width: 1.25em; /* Responsive width: scales with text size (20px at 16px base) */
    height: 1.25em; /* Responsive height: scales with text size */
    display: flex;
    align-items: center;
    justify-content: center;
    /* Position chevron exactly halfway between parent and child circles */
    position: absolute;
    left: calc(
      -1 * var(--node-indent) / 2
    ); /* Exactly half the indentation distance back from current node */
    top: 50%; /* Center vertically relative to the text content area */
    transform: translateY(
      -50%
    ); /* Center vertically only - horizontal positioning is mathematically exact */
    z-index: 999; /* Very high z-index to ensure clickability over all other elements */
  }

  .node-chevron:focus {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
    opacity: 1; /* Always visible when focused */
  }

  /* Show chevron only when hovering directly over this node's content wrapper (not child nodes) */
  .node-content-wrapper:hover > .node-chevron {
    opacity: 1;
  }

  /* Expanded state: rotate 90 degrees to point down */
  .node-chevron.expanded {
    transform: translateY(-50%) rotate(90deg);
  }

  /* Spacer for alignment when no chevron - no longer needed since chevron is absolutely positioned */
  .node-chevron-spacer {
    display: none; /* Hide spacer since chevron doesn't affect layout flow */
  }

  /* Header-level text-visual-center definitions - exactly matches BaseNode.svelte for perfect chevron alignment */
  .node-content-wrapper {
    /* Base correction factor - matches BaseNode.svelte exactly */
    --baseline-correction: -0.06375em;
    /* Default text visual center for normal text (not headers) */
    --text-visual-center: calc(0.816em + var(--baseline-correction));
  }

  /* Set CSS variables directly on node-content-wrapper based on TextNode header levels */
  .node-content-wrapper:has(:global(.node--h1)) {
    --text-visual-center: calc(1.2em + var(--baseline-correction) + 0.053125em);
  }

  .node-content-wrapper:has(:global(.node--h2)) {
    --text-visual-center: calc(0.975em + var(--baseline-correction) + 0.0542em);
  }

  .node-content-wrapper:has(:global(.node--h3)) {
    --text-visual-center: calc(0.875em + var(--baseline-correction) + 0.1em);
  }

  .node-content-wrapper:has(:global(.node--h4)) {
    --text-visual-center: calc(0.7875em + var(--baseline-correction));
  }

  .node-content-wrapper:has(:global(.node--h5)) {
    --text-visual-center: calc(0.7em + var(--baseline-correction));
  }

  .node-content-wrapper:has(:global(.node--h6)) {
    --text-visual-center: calc(0.6125em + var(--baseline-correction));
  }
</style>
