<!--
  BaseNodeViewer - Container that manages a collection of nodes
  Handles node creation, deletion, and organization
  
  Now uses NodeServiceContext to provide @ autocomplete functionality
  to all TextNode components automatically via proper inheritance.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { htmlToMarkdown } from '$lib/utils/markdown.js';
  import { pluginRegistry } from '$lib/components/viewers/index';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import TextNodeViewer from '$lib/components/viewers/text-node-viewer.svelte';
  import { getNodeServices } from '$lib/contexts/node-service-context.svelte';
  import type { Snippet } from 'svelte';

  // Props
  let { header }: { header?: Snippet } = $props();

  // Get nodeManager from shared context
  const services = getNodeServices();
  if (!services) {
    throw new Error(
      'NodeServices not available. Make sure base-node-viewer is wrapped in NodeServiceContext.'
    );
  }

  const nodeManager = services.nodeManager;

  // Map to store cursor positions during node type changes
  const pendingCursorPositions = new Map<string, number>();

  // Focus handling function with proper cursor positioning using tree walker
  function requestNodeFocus(nodeId: string, position: number) {
    // Find the target node
    const node = nodeManager.findNode(nodeId);
    if (!node) {
      console.error(`Node ${nodeId} not found for focus request`);
      return;
    }

    // Use DOM API to focus the node directly with cursor positioning
    setTimeout(() => {
      const nodeElement = document.querySelector(
        `[data-node-id="${nodeId}"] [contenteditable]`
      ) as HTMLElement;
      if (nodeElement) {
        nodeElement.focus();

        // Set cursor position using tree walker (same approach as controller)
        if (position >= 0) {
          const selection = window.getSelection();
          if (!selection) return;

          // Use tree walker to find the correct text node and offset
          const walker = document.createTreeWalker(nodeElement, NodeFilter.SHOW_TEXT, null);

          let currentOffset = 0;
          let currentNode;

          while ((currentNode = walker.nextNode())) {
            const nodeLength = currentNode.textContent?.length || 0;

            if (currentOffset + nodeLength >= position) {
              const range = document.createRange();
              const offsetInNode = position - currentOffset;
              range.setStart(currentNode, Math.min(offsetInNode, nodeLength));
              range.setEnd(currentNode, Math.min(offsetInNode, nodeLength));

              selection.removeAllRanges();
              selection.addRange(range);
              return;
            }

            currentOffset += nodeLength;
          }

          // If we didn't find the position, position at the end
          const range = document.createRange();
          range.selectNodeContents(nodeElement);
          range.collapse(false);
          selection.removeAllRanges();
          selection.addRange(range);
        }
      } else {
        console.error(`Could not find contenteditable element for node ${nodeId}`);
      }
    }, 10);
  }

  /**
   * Add appropriate formatting syntax to content based on node type
   * Used when creating new nodes from splits to preserve formatting
   *
   * NOTE: Header syntax inheritance is now handled in createNode() in reactiveNodeService.svelte.ts
   * to avoid duplication and ensure consistent behavior.
   */
  function addFormattingSyntax(content: string): string {
    // Header syntax inheritance is now handled in the createNode function
    // to ensure consistent behavior and avoid duplication

    // Return content as-is if no formatting needed
    if (!content) return content;

    // For task nodes: no automatic syntax addition
    // Task checkbox syntax ([ ]) is only added when users type it as a shortcut
    // Splitting a task node preserves the visual task state but not the syntax

    // For other node types, return as-is
    return content;
  }

  // Handle creating new nodes when Enter is pressed
  function handleCreateNewNode(
    event: CustomEvent<{
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      originalContent?: string;
      inheritHeaderLevel?: number;
      cursorAtBeginning?: boolean;
      insertAtBeginning?: boolean;
      focusOriginalNode?: boolean;
    }>
  ) {
    const {
      afterNodeId,
      nodeType,
      currentContent,
      newContent,
      originalContent,
      inheritHeaderLevel,
      insertAtBeginning,
      focusOriginalNode
    } = event.detail;

    // Validate node creation parameters
    if (!afterNodeId || !nodeType) {
      console.error('Invalid node creation parameters:', { afterNodeId, nodeType });
      return;
    }

    // Verify the target node exists
    if (!nodeManager.nodes.has(afterNodeId)) {
      console.error('Target node does not exist:', afterNodeId);
      return;
    }

    // Update current node content if provided
    if (currentContent !== undefined) {
      // Use updateNodeContent for node splitting - with new reactive architecture no forcing needed
      nodeManager.updateNodeContent(afterNodeId, currentContent);
    }

    // Create new node using NodeManager - placeholder if empty, real if has content
    let newNodeId: string;

    if (!newContent || newContent.trim() === '') {
      // Create placeholder node for empty content (Enter key without splitting)
      newNodeId = nodeManager.createPlaceholderNode(
        afterNodeId,
        nodeType,
        inheritHeaderLevel,
        insertAtBeginning || false,
        originalContent,
        !focusOriginalNode // Focus new node when creating splits, original node when creating above
      );
    } else {
      // Create real node when splitting existing content
      // Add formatting syntax to the new content based on node type and header level
      const formattedNewContent = addFormattingSyntax(newContent);

      newNodeId = nodeManager.createNode(
        afterNodeId,
        formattedNewContent,
        nodeType,
        inheritHeaderLevel,
        insertAtBeginning || false,
        originalContent,
        !focusOriginalNode // Focus new node when creating splits, original node when creating above
      );
    }

    // Validate that node creation succeeded
    if (!newNodeId || !nodeManager.nodes.has(newNodeId)) {
      console.error('Node creation failed for afterNodeId:', afterNodeId, 'newNodeId:', newNodeId);
      return;
    }

    // Handle focus direction based on focusOriginalNode parameter
    if (focusOriginalNode) {
      // The hierarchy is correct (new node above, original below)
      // Use the nodeManager's update methods to properly trigger reactivity

      // Use updateNodeContent on original node to trigger focus
      const originalNode = nodeManager.nodes.get(afterNodeId);
      if (originalNode) {
        // Update the original node's content to itself, which should trigger focus
        nodeManager.updateNodeContent(afterNodeId, originalNode.content);
      }
    }

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

    try {
      // Validate node exists before indenting
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot indent non-existent node:', nodeId);
        return;
      }

      // Store cursor position before DOM changes
      const cursorPosition = saveCursorPosition(nodeId);

      // Use NodeManager to handle indentation
      const success = nodeManager.indentNode(nodeId);

      if (success) {
        // Restore cursor position after DOM update
        setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
      }
    } catch (error) {
      console.error('Error during node indentation:', error);
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

      // If we couldn\'t find the exact position, place cursor at end
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

    try {
      // Validate node exists before outdenting
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot outdent non-existent node:', nodeId);
        return;
      }

      // Store cursor position before DOM changes
      const cursorPosition = saveCursorPosition(nodeId);

      // Use NodeManager to handle outdentation
      const success = nodeManager.outdentNode(nodeId);

      if (success) {
        // Restore cursor position after DOM update
        setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
      }
    } catch (error) {
      console.error('Error during node outdentation:', error);
    }
  }

  // Handle chevron click to toggle expand/collapse
  function handleToggleExpanded(nodeId: string) {
    // Get the currently focused element before DOM changes
    const activeElement = document.activeElement as HTMLElement;
    const isTextEditor = activeElement && activeElement.id?.startsWith('contenteditable-');
    let focusedNodeId: string | null = null;
    let cursorPosition = 0;

    // Store cursor position if we have an active text editor
    if (isTextEditor) {
      focusedNodeId = activeElement.id.replace('contenteditable-', '');
      cursorPosition = saveCursorPosition(focusedNodeId);
    }

    // Perform the toggle operation
    nodeManager.toggleExpanded(nodeId);

    // Restore focus and cursor position after DOM update
    if (focusedNodeId && isTextEditor) {
      setTimeout(() => {
        const element = document.getElementById(`contenteditable-${focusedNodeId}`);
        if (element && document.body.contains(element)) {
          restoreCursorPosition(focusedNodeId, cursorPosition);
        }
      }, 0);
    }
  }

  // Handle arrow key navigation between nodes using entry/exit methods
  function handleArrowNavigation(
    event: CustomEvent<{
      nodeId: string;
      direction: 'up' | 'down';
      pixelOffset: number;
    }>
  ) {
    const { nodeId, direction, pixelOffset } = event.detail;

    // Get visible nodes from NodeManager
    const currentVisibleNodes = nodeManager.visibleNodes;
    const currentIndex = currentVisibleNodes.findIndex((n) => n.id === nodeId);

    if (currentIndex === -1) return;

    // Find next navigable node that accepts navigation
    let targetIndex = direction === 'up' ? currentIndex - 1 : currentIndex + 1;

    while (targetIndex >= 0 && targetIndex < currentVisibleNodes.length) {
      const candidateNode = currentVisibleNodes[targetIndex];

      // Check if this node accepts navigation (skip if it doesn\'t)
      // For now, assume all nodes accept navigation (will be refined per node type)
      const acceptsNavigation = true; // candidateNode.navigationMethods?.canAcceptNavigation() ?? true;

      if (acceptsNavigation) {
        // Found a node that accepts navigation - try to enter it
        const success = enterNodeAtPosition(candidateNode.id, direction, pixelOffset);
        if (success) return;
      }

      // This node doesn\'t accept navigation or entry failed - try next one
      targetIndex = direction === 'up' ? targetIndex - 1 : targetIndex + 1;
    }
  }

  /**
   * Position cursor at a target pixel offset within a line by iterating through characters.
   * This works correctly with proportional fonts without needing character width conversion.
   */
  function setCursorAtPixelOffset(
    element: HTMLElement,
    lineElement: Element,
    targetPixelOffset: number
  ) {
    // Calculate offset relative to the element's left edge, not the root container
    // This ensures the horizontal position is preserved when navigating between nodes
    const elementRect = element.getBoundingClientRect();

    const textNodes = getTextNodes(lineElement as HTMLElement);
    if (textNodes.length === 0) {
      // Empty line - position at start
      const selection = window.getSelection();
      if (selection) {
        const range = document.createRange();
        range.selectNodeContents(lineElement);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      }
      return;
    }

    // Binary search through character positions to find closest to target pixel offset
    let bestOffset = 0;
    let bestDistance = Infinity;

    for (const textNode of textNodes) {
      const textContent = textNode.textContent || '';
      for (let i = 0; i <= textContent.length; i++) {
        try {
          const testRange = document.createRange();
          testRange.setStart(textNode, i);
          testRange.setEnd(textNode, i);
          const testRect = testRange.getBoundingClientRect();
          const currentPixel = testRect.left - elementRect.left;
          const distance = Math.abs(currentPixel - targetPixelOffset);

          if (distance < bestDistance) {
            bestDistance = distance;
            bestOffset = i;
            // Save the node for later use
            (setCursorAtPixelOffset as { bestNode?: Text }).bestNode = textNode;
          }

          // Early exit if we've gone past the target
          if (currentPixel > targetPixelOffset) break;
        } catch {
          // Skip invalid positions
        }
      }
    }

    // Set cursor at best position
    const selection = window.getSelection();
    if (selection && (setCursorAtPixelOffset as { bestNode?: Text }).bestNode) {
      const range = document.createRange();
      range.setStart((setCursorAtPixelOffset as { bestNode?: Text }).bestNode!, bestOffset);
      range.collapse(true);
      selection.removeAllRanges();
      selection.addRange(range);
    }
  }

  // Enter a node using its entry methods or fallback positioning
  function enterNodeAtPosition(
    targetNodeId: string,
    direction: 'up' | 'down',
    pixelOffset: number
  ): boolean {
    // Try component-level navigation methods first (future enhancement)
    // const nodeComponent = getNodeComponent(targetNodeId);
    // if (nodeComponent?.navigationMethods) {
    //   return direction === 'up'
    //     ? nodeComponent.navigationMethods.enterFromBottom(columnHint)
    //     : nodeComponent.navigationMethods.enterFromTop(columnHint);
    // }

    // Pixel-based positioning - works correctly with proportional fonts
    setTimeout(() => {
      const targetElement = document.getElementById(`contenteditable-${targetNodeId}`);
      if (!targetElement) return;

      // Prepare the controller for arrow navigation to prevent content update
      const controller = (
        targetElement as unknown as {
          _contentEditableController?: { prepareForArrowNavigation?: () => void };
        }
      )._contentEditableController;
      if (controller && typeof controller.prepareForArrowNavigation === 'function') {
        controller.prepareForArrowNavigation();
      }

      // Hide caret during navigation to prevent visible cursor bounce
      targetElement.style.caretColor = 'transparent';

      targetElement.focus();

      // Wait for DOM to update after focus (transition to editing mode creates DIV structure)
      // Increased delay to ensure setRawMarkdown() completes and DIVs are created
      setTimeout(() => {
        // Check if multiline or single-line
        const divElements = targetElement.querySelectorAll(':scope > div');
        const isMultiline = divElements.length > 0;

        if (isMultiline) {
          // For multiline: position cursor at pixelOffset within the first/last line
          const lineElement =
            direction === 'up'
              ? divElements[divElements.length - 1] // Last line when entering from bottom
              : divElements[0]; // First line when entering from top

          setCursorAtPixelOffset(targetElement, lineElement, pixelOffset);
        } else {
          // For single-line: position cursor at pixelOffset within the single line
          setCursorAtPixelOffset(targetElement, targetElement, pixelOffset);
        }

        // Show caret after positioning is complete
        targetElement.style.caretColor = '';
      }, 20); // Delay for setRawMarkdown() to complete and create DIV structure
    }, 0);

    return true;
  }

  // Utility to set cursor position in any contenteditable element

  // Handle combining current node with previous node (Backspace at start of node)
  // CLEAN DELEGATION: All logic handled by NodeManager
  function handleCombineWithPrevious(
    event: CustomEvent<{ nodeId: string; currentContent: string }>
  ) {
    try {
      const { nodeId, currentContent } = event.detail;

      // Validate node exists before combining
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot combine non-existent node:', nodeId);
        return;
      }

      const currentVisibleNodes = nodeManager.visibleNodes;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === nodeId);

      if (currentIndex <= 0) {
        return; // No previous node to combine with
      }

      const previousNode = currentVisibleNodes[currentIndex - 1];

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        console.error('Previous node not found or invalid:', previousNode?.id);
        return;
      }

      if (currentContent.trim() === '') {
        // Empty node - delete and focus previous at end
        nodeManager.deleteNode(nodeId);
        requestNodeFocus(previousNode.id, previousNode.content.length);
      } else {
        // Combine nodes - NodeManager handles focus automatically
        nodeManager.combineNodes(nodeId, previousNode.id);
      }
    } catch (error) {
      console.error('Error during node combination:', error);
    }
  }

  // Handle deleting empty node (Backspace at start of empty node)
  function handleDeleteNode(event: CustomEvent<{ nodeId: string }>) {
    try {
      const { nodeId } = event.detail;

      // Validate node exists before deletion
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot delete non-existent node:', nodeId);
        return;
      }

      const currentVisibleNodes = nodeManager.visibleNodes;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === nodeId);

      if (currentIndex <= 0) return; // No previous node to focus

      const previousNode = currentVisibleNodes[currentIndex - 1];

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        console.error('Previous node not found for focus after deletion:', previousNode?.id);
        // Still delete the node even if we can\'t focus the previous one
        nodeManager.deleteNode(nodeId);
        return;
      }

      // Delete node and focus previous at end
      nodeManager.deleteNode(nodeId);
      requestNodeFocus(previousNode.id, previousNode.content.length);
    } catch (error) {
      console.error('Error during node deletion:', error);
    }
  }

  // Handle icon click events (for non-task node types)
  function handleIconClick(
    event: CustomEvent<{ nodeId: string; nodeType: string; currentState?: string }>
  ) {
    const { nodeType } = event.detail;

    // Skip task nodes - let TaskNode component handle its own state management
    if (nodeType === 'task') {
      return;
    }

    // For other node types, the click could trigger different behaviors
    // This makes the system extensible for future node types
  }

  // Helper functions removed - NodeManager handles all node operations

  // Simple reactive access - let template handle reactivity directly

  // Dynamic component loading - create stable component mapping for both viewers and nodes
  let loadedViewers = $state(new Map<string, unknown>());
  // Proper Svelte 5 reactivity: use object instead of Map for reactive tracking
  let loadedNodes = $state<Record<string, unknown>>({});

  // Track focused node for autoFocus after node type changes
  let focusedNodeId = $state<string | null>(null);

  // Clear focusedNodeId after a delay to prevent permanent focus
  $effect(() => {
    if (focusedNodeId) {
      // Check if there's a pending cursor position for this node
      const pendingPosition = pendingCursorPositions.get(focusedNodeId);
      if (pendingPosition !== undefined) {
        // Use requestNodeFocus to position cursor precisely
        // This bypasses autoFocus to avoid conflicts
        requestNodeFocus(focusedNodeId, pendingPosition);
        // Clear the pending position
        pendingCursorPositions.delete(focusedNodeId);
        // Clear focusedNodeId immediately since we've handled the focus
        focusedNodeId = null;
        return;
      }

      const timeoutId = setTimeout(() => {
        focusedNodeId = null;
      }, 100); // Clear after 100ms to allow autoFocus to trigger

      return () => clearTimeout(timeoutId);
    }
  });

  // Pre-load components when component mounts
  onMount(async () => {
    async function preloadComponents() {
      // Pre-load all known types
      const knownTypes = ['text', 'date', 'task', 'ai-chat'];

      for (const nodeType of knownTypes) {
        // Load viewers
        if (!loadedViewers.has(nodeType)) {
          try {
            const customViewer = await pluginRegistry.getViewer(nodeType);
            if (customViewer) {
              loadedViewers.set(nodeType, customViewer);
            } else {
              // Fallback to BaseNode for unknown types
              loadedViewers.set(nodeType, BaseNode);
            }
          } catch {
            loadedViewers.set(nodeType, BaseNode);
          }
        }

        // Load node components
        if (!(nodeType in loadedNodes)) {
          try {
            const customNode = await pluginRegistry.getNodeComponent(nodeType);
            if (customNode) {
              loadedNodes[nodeType] = customNode;
            } else {
              // Fallback to BaseNode for unknown types
              loadedNodes[nodeType] = BaseNode;
            }
          } catch (error) {
            console.error(`💥 Error loading node component for ${nodeType}:`, error);
            loadedNodes[nodeType] = BaseNode;
          }
        }
      }
    }

    await preloadComponents();
  });
</script>

<!-- Base Node Viewer: Header + Scrollable Children Area -->
<div class="base-node-viewer">
  <!-- Header Section (can be customized via snippet) -->
  {#if header}
    <div class="viewer-header">
      {@render header()}
    </div>
  {/if}

  <!-- Scrollable Node Content Area (children structure) -->
  <div class="node-content-area">
    {#each nodeManager.visibleNodes as node (node.id)}
      <div
        class="node-container"
        data-has-children={node.children?.length > 0}
        style="margin-left: {(node.depth || 0) * 2.5}rem"
      >
        <div class="node-content-wrapper">
          <!-- Chevron for parent nodes using design system approach -->
          {#if node.children && node.children.length > 0}
            <button
              class="chevron-icon"
              class:expanded={node.expanded}
              onclick={() => handleToggleExpanded(node.id)}
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  handleToggleExpanded(node.id);
                }
              }}
              aria-label={node.expanded ? 'Collapse node' : 'Expand node'}
              aria-expanded={node.expanded}
            >
              <svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg">
                <path d="M6 3l5 5-5 5-1-1 4-4-4-4 1-1z" />
              </svg>
            </button>
          {/if}

          <!-- Node viewer with stable component references -->
          {#if node.nodeType === 'text'}
            {#key `${node.id}-${node.nodeType}`}
              <TextNodeViewer
                nodeId={node.id}
                nodeType={node.nodeType}
                autoFocus={(node.autoFocus || node.id === focusedNodeId) &&
                  !pendingCursorPositions.has(node.id)}
                content={node.content}
                inheritHeaderLevel={node.inheritHeaderLevel || 0}
                children={node.children}
                on:createNewNode={handleCreateNewNode}
                on:indentNode={handleIndentNode}
                on:outdentNode={handleOutdentNode}
                on:navigateArrow={handleArrowNavigation}
                on:contentChanged={(e) => {
                  const content = e.detail.content;

                  // Update node content (placeholder flag is handled automatically)
                  nodeManager.updateNodeContent(node.id, content);
                }}
                on:slashCommandSelected={(
                  e: CustomEvent<{ command: string; nodeType: string; cursorPosition?: number }>
                ) => {
                  // Store cursor position before node type change
                  if (e.detail.cursorPosition !== null && e.detail.cursorPosition !== undefined) {
                    pendingCursorPositions.set(node.id, e.detail.cursorPosition);
                  }

                  if (node.isPlaceholder) {
                    // For placeholder nodes, just update the nodeType locally
                    if ('updatePlaceholderNodeType' in nodeManager) {
                      (nodeManager as any).updatePlaceholderNodeType(node.id, e.detail.nodeType);
                    }
                  } else {
                    // For real nodes, update node type with full persistence
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }

                  // Set autoFocus to restore focus after nodeType change
                  focusedNodeId = node.id;
                }}
                on:nodeTypeChanged={(
                  e: CustomEvent<{ nodeType: string; cleanedContent?: string }>
                ) => {
                  const newNodeType = e.detail.nodeType;
                  const cleanedContent = e.detail.cleanedContent;
                  const targetNode = nodeManager.nodes.get(node.id);
                  if (targetNode) {
                    // Update both the node manager and local state
                    targetNode.nodeType = newNodeType;

                    // If cleanedContent is provided, update the node content too
                    if (cleanedContent !== undefined) {
                      // Use requestAnimationFrame to ensure the update happens after component mounting
                      requestAnimationFrame(() => {
                        // Use proper node manager method to trigger reactivity
                        nodeManager.updateNodeContent(node.id, cleanedContent);
                      });
                    }

                    // Clean up task-specific metadata when converting to text
                    if (newNodeType === 'text' && targetNode.metadata.taskState) {
                      const { taskState, ...cleanMetadata } = targetNode.metadata;
                      void taskState; // Intentionally unused - extracted to remove from metadata
                      targetNode.metadata = { ...cleanMetadata, _forceUpdate: Date.now() };
                    } else {
                      targetNode.metadata = { ...targetNode.metadata, _forceUpdate: Date.now() };
                    }
                    nodeManager.updateNodeContent(targetNode.id, targetNode.content);
                  }
                  focusedNodeId = node.id;
                }}
                on:combineWithPrevious={handleCombineWithPrevious}
                on:deleteNode={handleDeleteNode}
              />
            {/key}
          {:else}
            <!-- Use plugin registry for non-text node types with key for re-rendering -->
            {#if node.nodeType in loadedNodes}
              {#key `${node.id}-${node.nodeType}`}
                {@const NodeComponent = loadedNodes[node.nodeType] as typeof BaseNode}
                <NodeComponent
                  nodeId={node.id}
                  nodeType={node.nodeType}
                  autoFocus={(node.autoFocus || node.id === focusedNodeId) &&
                    !pendingCursorPositions.has(node.id)}
                  content={node.content}
                  headerLevel={node.inheritHeaderLevel || 0}
                  children={node.children}
                  metadata={node.metadata || {}}
                  editableConfig={{ allowMultiline: true }}
                  on:createNewNode={handleCreateNewNode}
                  on:indentNode={handleIndentNode}
                  on:outdentNode={handleOutdentNode}
                  on:navigateArrow={handleArrowNavigation}
                  on:contentChanged={(e: CustomEvent<{ content: string }>) => {
                    const content = e.detail.content;
                    // Update node content (placeholder flag is handled automatically)
                    nodeManager.updateNodeContent(node.id, content);
                    // Set autoFocus to restore focus after nodeType change
                    focusedNodeId = node.id;
                  }}
                  on:headerLevelChanged={() => {
                    // Header level change is handled automatically through content updates
                    // Just restore focus after the change
                    focusedNodeId = node.id;
                  }}
                  on:nodeTypeChanged={(
                    e: CustomEvent<{ nodeType: string; cleanedContent?: string }>
                  ) => {
                    const nodeType = e.detail.nodeType;
                    const cleanedContent = e.detail.cleanedContent;
                    const targetNode = nodeManager.nodes.get(node.id);
                    if (targetNode) {
                      // Update the node type
                      targetNode.nodeType = nodeType;

                      // If cleanedContent is provided, update the node content too
                      if (cleanedContent !== undefined) {
                        // Use requestAnimationFrame to ensure the update happens after component mounting
                        requestAnimationFrame(() => {
                          // Use proper node manager method to trigger reactivity
                          nodeManager.updateNodeContent(node.id, cleanedContent);
                        });
                      }

                      // CRITICAL: Clean up type-specific metadata when changing node types
                      if (nodeType === 'text' && targetNode.metadata.taskState) {
                        // When converting from task to text, remove task-specific metadata
                        const { taskState, ...cleanMetadata } = targetNode.metadata;
                        void taskState; // Intentionally unused - extracted to remove from metadata
                        targetNode.metadata = { ...cleanMetadata, _forceUpdate: Date.now() };
                      } else {
                        // For other conversions, just force update
                        targetNode.metadata = { ...targetNode.metadata, _forceUpdate: Date.now() };
                      }

                      // Use the working sync mechanism from taskStateChanged
                      nodeManager.updateNodeContent(targetNode.id, targetNode.content);
                    }
                    // Set autoFocus to restore focus after nodeType change
                    focusedNodeId = node.id;
                  }}
                  on:slashCommandSelected={(
                    e: CustomEvent<{ command: string; nodeType: string; cursorPosition?: number }>
                  ) => {
                    // Store cursor position before node type change
                    if (e.detail.cursorPosition !== null && e.detail.cursorPosition !== undefined) {
                      pendingCursorPositions.set(node.id, e.detail.cursorPosition);
                    }

                    if (node.isPlaceholder) {
                      // For placeholder nodes, just update the nodeType locally
                      if ('updatePlaceholderNodeType' in nodeManager) {
                        (nodeManager as any).updatePlaceholderNodeType(node.id, e.detail.nodeType);
                      }
                    } else {
                      // For real nodes, update node type with full persistence
                      nodeManager.updateNodeType(node.id, e.detail.nodeType);
                    }

                    // Set autoFocus to restore focus after nodeType change
                    focusedNodeId = node.id;
                  }}
                  on:iconClick={handleIconClick}
                  on:taskStateChanged={(e) => {
                    const { nodeId, state } = e.detail;
                    const node = nodeManager.nodes.get(nodeId);
                    if (node) {
                      node.metadata = { ...node.metadata, taskState: state };
                      // Trigger sync to persist the change
                      nodeManager.updateNodeContent(nodeId, node.content);
                    }
                  }}
                  on:combineWithPrevious={handleCombineWithPrevious}
                  on:deleteNode={handleDeleteNode}
                />
              {/key}
            {:else}
              <!-- Final fallback to BaseNode with key for re-rendering -->
              {#key `${node.id}-${node.nodeType}`}
                <BaseNode
                  nodeId={node.id}
                  nodeType={node.nodeType}
                  autoFocus={(node.autoFocus || node.id === focusedNodeId) &&
                    !pendingCursorPositions.has(node.id)}
                  content={node.content}
                  headerLevel={node.inheritHeaderLevel || 0}
                  children={node.children}
                  metadata={node.metadata || {}}
                  editableConfig={{ allowMultiline: true }}
                  on:createNewNode={handleCreateNewNode}
                  on:indentNode={handleIndentNode}
                  on:outdentNode={handleOutdentNode}
                  on:navigateArrow={handleArrowNavigation}
                  on:contentChanged={(e: CustomEvent<{ content: string }>) => {
                    const content = e.detail.content;

                    // Update node content (placeholder flag is handled automatically)
                    nodeManager.updateNodeContent(node.id, content);
                  }}
                  on:slashCommandSelected={(
                    e: CustomEvent<{ command: string; nodeType: string; cursorPosition?: number }>
                  ) => {
                    // Store cursor position before node type change
                    if (e.detail.cursorPosition !== null && e.detail.cursorPosition !== undefined) {
                      pendingCursorPositions.set(node.id, e.detail.cursorPosition);
                    }

                    if (node.isPlaceholder) {
                      // For placeholder nodes, just update the nodeType locally
                      if ('updatePlaceholderNodeType' in nodeManager) {
                        (nodeManager as any).updatePlaceholderNodeType(node.id, e.detail.nodeType);
                      }
                    } else {
                      // For real nodes, update node type with full persistence
                      nodeManager.updateNodeType(node.id, e.detail.nodeType);
                    }

                    // Set autoFocus to restore focus after nodeType change
                    focusedNodeId = node.id;
                  }}
                  on:iconClick={handleIconClick}
                  on:taskStateChanged={(e) => {
                    const { nodeId, state } = e.detail;
                    const node = nodeManager.nodes.get(nodeId);
                    if (node) {
                      node.metadata = { ...node.metadata, taskState: state };
                      // Trigger sync to persist the change
                      nodeManager.updateNodeContent(nodeId, node.content);
                    }
                  }}
                  on:combineWithPrevious={handleCombineWithPrevious}
                  on:deleteNode={handleDeleteNode}
                />
              {/key}
            {/if}
          {/if}
        </div>
      </div>
    {/each}
  </div>
</div>

<!-- Template structure fixed -->

<style>
  /* Base container - full height layout */
  .base-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
  }

  /* Header section - fixed at top, doesn't scroll */
  .viewer-header {
    flex-shrink: 0;
  }

  /* Scrollable node content area for children structure */
  .node-content-area {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    min-height: 0; /* Critical for flex scrolling */
    padding: 1.5rem;
    padding-bottom: 3rem; /* Extra bottom padding to see last line fully */
    display: flex;
    flex-direction: column;
    gap: 0; /* 0px gap - all spacing from node padding for 8px total */

    /* Dynamic Circle Positioning System - All values configurable from here */
    --circle-offset: 22px; /* Circle center distance from container left edge - reserves space for chevrons */
    --circle-diameter: 20px; /* Circle size (width and height) */
    --circle-text-gap: 8px; /* Gap between circle edge and text content */
    --node-indent: 2.5rem; /* Indentation distance between parent and child levels */

    /* NodeSpace Extension Colors - Subtle Tint System (Scheme 3) */
    --node-text: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-task: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-ai-chat: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-entity: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-query: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
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

  /* 
    Chevron positioning system - matches circle positioning exactly
    
    POSITIONING FORMULA:
    The chevron must be vertically centered with the circles, which use:
    top: calc(0.25rem + (var(--font-size) * var(--line-height) / 2))
    
    Where:
    - 0.25rem = container top padding (.node has padding: 0.25rem)
    - line-height-px = font-size × line-height multiplier
    - This formula positions at the visual center of the first line of text
    
    HORIZONTAL POSITION:
    - Exactly halfway between parent and child circles
    - Parent is at current depth, child is at depth + 2.5rem (--node-indent)
    - Chevron positioned at -1.25rem (half of 2.5rem) from current node
    
    INHERITANCE:
    The --line-height-px variable is inherited from .node-content-wrapper
    which detects the header level of nested content using :has() selector
  */
  .chevron-icon {
    opacity: 0; /* Hidden by default - shows on hover */
    background: none;
    border: none;
    padding: 0.125rem; /* 2px padding for clickable area */
    cursor: pointer;
    border-radius: 0.125rem; /* 2px border radius */
    transition: opacity 0.15s ease-in-out; /* Smooth fade in/out */
    pointer-events: auto; /* Ensure chevron always receives pointer events */
    flex-shrink: 0;
    width: 1.25rem; /* Fixed 20px to match circle size */
    height: 1.25rem; /* Fixed 20px to match circle size */
    display: flex;
    align-items: center;
    justify-content: center;
    /* Position chevron exactly halfway between parent and child circles */
    position: absolute;
    left: calc(
      -1 * var(--node-indent) / 2 + var(--circle-offset)
    ); /* Halfway back to parent + parent circle offset */
    /* Use same CSS-first positioning as circles: containerPaddingPx + (lineHeightPx / 2) */
    top: calc(0.25rem + (var(--font-size) * var(--line-height) / 2));
    transform: translate(-50%, -50%); /* Center icon on coordinates, same as circles */
    z-index: 999; /* Very high z-index to ensure clickability over all other elements */
  }

  .chevron-icon svg {
    width: 16px;
    height: 16px;
    fill: hsl(var(--node-text) / 0.5);
    transition: fill 0.15s ease;
  }

  .chevron-icon:focus {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
    opacity: 1; /* Always visible when focused */
  }

  .chevron-icon:hover svg {
    fill: hsl(var(--node-text) / 0.5);
  }

  /* Show chevron only when hovering directly over this node's content wrapper (not child nodes) */
  .node-content-wrapper:hover > .chevron-icon {
    opacity: 1;
  }

  /* Expanded state: rotate 90 degrees to point down */
  .chevron-icon.expanded {
    transform: translate(-50%, -50%) rotate(90deg);
  }

  /* CSS-first positioning to match base-node.svelte implementation */
  .node-content-wrapper {
    /* Default values for normal text - adjusted for better circle alignment */
    --line-height: 1.875;
    --font-size: 1rem;
  }

  /* Inherit font-size and line-height from nested BaseNode header classes */
  .node-content-wrapper:has(:global(.node--h1)) {
    --font-size: 2rem;
    --line-height: 1.2;
  }

  .node-content-wrapper:has(:global(.node--h2)) {
    --font-size: 1.5rem;
    --line-height: 1.3;
  }

  .node-content-wrapper:has(:global(.node--h3)) {
    --font-size: 1.25rem;
    --line-height: 1.4;
  }

  .node-content-wrapper:has(:global(.node--h4)) {
    --font-size: 1.125rem;
    --line-height: 1.4;
  }

  .node-content-wrapper:has(:global(.node--h5)) {
    --font-size: 1rem;
    --line-height: 1.4;
  }

  .node-content-wrapper:has(:global(.node--h6)) {
    --font-size: 0.875rem;
    --line-height: 1.4;
  }
</style>
