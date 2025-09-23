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

  // Get nodeManager from shared context
  const services = getNodeServices();
  if (!services) {
    throw new Error('NodeServices not available. Make sure base-node-viewer is wrapped in NodeServiceContext.');
  }

  const nodeManager = services.nodeManager as unknown;

  // Focus handling function
  function requestNodeFocus(nodeId: string, position: number) {
    console.log(`🎯 requestNodeFocus called: node ${nodeId} at position ${position}`);

    // Find the node in the visible nodes
    const node = nodeManager.findNode(nodeId);
    if (!node) {
      console.error(`❌ requestNodeFocus: node ${nodeId} not found`);
      return;
    }

    // Use DOM API to focus the node directly with cursor positioning
    setTimeout(() => {
      const nodeElement = document.querySelector(`[data-node-id="${nodeId}"] [contenteditable]`) as HTMLElement;
      if (nodeElement) {
        nodeElement.focus();

        // Set cursor position
        if (position >= 0) {
          const range = document.createRange();
          const selection = window.getSelection();

          // Find the text node and set cursor position
          if (nodeElement.firstChild && nodeElement.firstChild.nodeType === Node.TEXT_NODE) {
            const textNode = nodeElement.firstChild;
            const actualPosition = Math.min(position, textNode.textContent?.length || 0);
            range.setStart(textNode, actualPosition);
            range.setEnd(textNode, actualPosition);
            selection?.removeAllRanges();
            selection?.addRange(range);
            console.log(`✅ Cursor positioned at ${actualPosition} in node ${nodeId}`);
          } else {
            console.log(`⚠️ Could not position cursor - no text node found in ${nodeId}`);
          }
        }

        console.log(`✅ Focus set on node ${nodeId}`);
      } else {
        console.error(`❌ Could not find contenteditable element for node ${nodeId}`);
      }
    }, 10);
  }

  /**
   * Add appropriate formatting syntax to content based on node type
   * Used when creating new nodes from splits to preserve formatting
   */
  function addFormattingSyntax(
    content: string,
    nodeType: string,
    inheritHeaderLevel?: number
  ): string {
    // For text nodes with header levels, add markdown header syntax
    if (nodeType === 'text' && inheritHeaderLevel && inheritHeaderLevel > 0) {
      const headerPrefix = '#'.repeat(inheritHeaderLevel) + ' ';
      // Add prefix if content is empty or doesn't already have it
      if (!content || !content.startsWith(headerPrefix.trim())) {
        return headerPrefix + content;
      }
    }

    // Return content as-is if no formatting needed
    if (!content) return content;

    // For task nodes: no automatic syntax addition
    // Task checkbox syntax ([ ]) is only added when users type it as a shortcut
    // Splitting a task node preserves the visual task state but not the syntax

    // For other node types or if syntax already exists, return as-is
    return content;
  }

  // Handle creating new nodes when Enter is pressed
  function handleCreateNewNode(
    event: CustomEvent<{
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      inheritHeaderLevel?: number;
      cursorAtBeginning?: boolean;
    }>
  ) {
    const {
      afterNodeId,
      nodeType,
      currentContent,
      newContent,
      inheritHeaderLevel,
      cursorAtBeginning
    } = event.detail;

    console.log(`📥 handleCreateNewNode received:`, { afterNodeId, nodeType, currentContent, newContent, inheritHeaderLevel, cursorAtBeginning });

    // CRITICAL FIX: Add validation to prevent circular reference issues
    if (!afterNodeId || !nodeType) {
      console.error('❌ VALIDATION FAILED: Invalid node creation parameters:', {
        afterNodeId,
        nodeType
      });
      return;
    }

    // Debug: Show what nodes actually exist
    console.log('🔍 Available nodes:', Array.from(nodeManager.nodes.keys()));
    console.log('🔍 Looking for node:', afterNodeId);

    // Verify the after node exists before creating
    if (!nodeManager.nodes.has(afterNodeId)) {
      console.error('❌ VALIDATION FAILED: After node does not exist:', afterNodeId);
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
      newNodeId = nodeManager.createPlaceholderNode(afterNodeId, nodeType, inheritHeaderLevel, cursorAtBeginning || false);
    } else {
      // Create real node when splitting existing content
      // Add formatting syntax to the new content based on node type and header level
      const formattedNewContent = addFormattingSyntax(newContent, nodeType, inheritHeaderLevel);

      newNodeId = nodeManager.createNode(
        afterNodeId,
        formattedNewContent,
        nodeType,
        inheritHeaderLevel,
        cursorAtBeginning || false
      );
    }

    // CRITICAL FIX: Validate that node creation was successful
    if (!newNodeId || !nodeManager.nodes.has(newNodeId)) {
      console.error('❌ NODE CREATION FAILED: Node creation failed for afterNodeId:', afterNodeId);
      console.error('❌ newNodeId:', newNodeId);
      console.error(
        '❌ nodeManager.nodes.has(newNodeId):',
        newNodeId ? nodeManager.nodes.has(newNodeId) : 'newNodeId is falsy'
      );
      return;
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
    console.log(`📥 handleIndentNode received for node: ${nodeId}`);

    try {
      // CRITICAL FIX: Add error recovery for node operations
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
    console.log(`📥 handleOutdentNode received for node: ${nodeId}`);

    try {
      // CRITICAL FIX: Add error recovery for node operations
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
      columnHint: number;
    }>
  ) {
    const { nodeId, direction, columnHint } = event.detail;

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
        const success = enterNodeAtPosition(candidateNode.id, direction, columnHint);
        if (success) return;
      }

      // This node doesn\'t accept navigation or entry failed - try next one
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

      // Helper function to get line information for multiline contenteditable
      function getLineInfo(element: HTMLElement) {
        const divElements = element.querySelectorAll(':scope > div');
        if (divElements.length > 0) {
          // Multiline content with div structure
          const lines = Array.from(divElements).map((div) => div.textContent || '');
          return { lines, isMultiline: true };
        } else {
          // Single line or fallback
          const lines = content.split('\n');
          return { lines, isMultiline: false };
        }
      }

      const { lines, isMultiline } = getLineInfo(targetElement);
      let targetPosition: number;

      if (direction === 'up') {
        // Entering from bottom: convert visual columnHint to logical position
        const lastLine = lines[lines.length - 1] || '';

        // Calculate position of last line start
        let lastLineStart: number;
        if (isMultiline && lines.length > 1) {
          // For multiline, sum up all previous lines
          lastLineStart = lines.slice(0, -1).reduce((sum, line) => sum + line.length, 0);
        } else {
          lastLineStart = content.length - lastLine.length;
        }

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
    try {
      const { nodeId, currentContent } = event.detail;
      console.log(`🔗 handleCombineWithPrevious called for node ${nodeId} with content: "${currentContent}"`);

      // CRITICAL FIX: Add error recovery for node operations
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot combine non-existent node:', nodeId);
        return;
      }

      const currentVisibleNodes = nodeManager.visibleNodes;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === nodeId);
      console.log(`📍 Current node index: ${currentIndex} of ${currentVisibleNodes.length} visible nodes`);

      if (currentIndex <= 0) {
        console.log(`⚠️ No previous node to combine with (currentIndex: ${currentIndex})`);
        return; // No previous node to combine with
      }

      const previousNode = currentVisibleNodes[currentIndex - 1];
      console.log(`🎯 Previous node found: ${previousNode.id} with content: "${previousNode.content}"`);

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        console.error('Previous node not found or invalid:', previousNode?.id);
        return;
      }

      if (currentContent.trim() === '') {
        // Empty node - delete and focus previous at end
        console.log(`🗑️ Empty node - deleting ${nodeId} and focusing ${previousNode.id}`);
        nodeManager.deleteNode(nodeId);
        requestNodeFocus(previousNode.id, previousNode.content.length);
      } else {
        // Combine nodes - NodeManager handles focus automatically
        console.log(`🔗 Combining nodes: ${nodeId} → ${previousNode.id}`);
        console.log(`🔧 About to call nodeManager.combineNodes(${nodeId}, ${previousNode.id})`);
        const result = nodeManager.combineNodes(nodeId, previousNode.id);
        console.log(`✅ nodeManager.combineNodes completed, result:`, result);
      }
    } catch (error) {
      console.error('Error during node combination:', error);
    }
  }

  // Handle deleting empty node (Backspace at start of empty node)
  function handleDeleteNode(event: CustomEvent<{ nodeId: string }>) {
    try {
      const { nodeId } = event.detail;

      // CRITICAL FIX: Add error recovery for node operations
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

  // Handle icon click events (for task state changes, etc.)
  function handleIconClick(
    event: CustomEvent<{ nodeId: string; nodeType: string; currentState?: string }>
  ) {
    const { nodeId, nodeType, currentState } = event.detail;

    // For task nodes, cycle through states: pending -> inProgress -> completed -> pending
    if (nodeType === 'task') {
      let newState: string;
      switch (currentState) {
        case 'pending':
          newState = 'inProgress';
          break;
        case 'inProgress':
          newState = 'completed';
          break;
        case 'completed':
        default:
          newState = 'pending';
          break;
      }

      // Update the node's metadata to store the task state
      const node = nodeManager.nodes.get(nodeId);
      if (node) {
        node.metadata = { ...node.metadata, taskState: newState };
        // Trigger an event to force reactivity updates
        nodeManager.updateNodeContent(nodeId, node.content); // This will trigger a syncStores
      }
    }

    // For other node types, the click could trigger different behaviors
    // This makes the system extensible for future node types
  }

  // Helper functions removed - NodeManager handles all node operations

  console.log(`🔧 BaseNodeViewer component initializing with nodeManager:`, nodeManager);

  // Simple reactive access - let template handle reactivity directly

  // Dynamic component loading - create stable component mapping for both viewers and nodes
  let loadedViewers = $state(new Map<string, unknown>());
  let loadedNodes = $state(new Map<string, unknown>());

  // Track focused node for autoFocus after node type changes
  let focusedNodeId = $state<string | null>(null);


  // Clear focusedNodeId after a delay to prevent permanent focus
  $effect(() => {
    if (focusedNodeId) {
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
        if (!loadedNodes.has(nodeType)) {
          try {
            console.log(`🔍 Loading node component for type: ${nodeType}`);
            const customNode = await pluginRegistry.getNodeComponent(nodeType);
            if (customNode) {
              console.log(`✅ Found custom node component for ${nodeType}:`, customNode);
              loadedNodes.set(nodeType, customNode);
            } else {
              console.log(`❌ No custom node component found for ${nodeType}, using BaseNode fallback`);
              // Fallback to BaseNode for unknown types
              loadedNodes.set(nodeType, BaseNode);
            }
          } catch (error) {
            console.error(`💥 Error loading node component for ${nodeType}:`, error);
            loadedNodes.set(nodeType, BaseNode);
          }
        }
      }
    }

    await preloadComponents();
  });
</script>

<!-- Node viewer content -->
<div class="node-viewer">
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
            <TextNodeViewer
              nodeId={node.id}
              nodeType={node.nodeType}
              autoFocus={node.autoFocus || node.id === focusedNodeId}
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
              on:slashCommandSelected={(e) => {
                console.log(`🎛️ Slash command selected for node ${node.id}:`, e.detail, 'isPlaceholder:', node.isPlaceholder);

                if (node.isPlaceholder) {
                  // For placeholder nodes, just update the nodeType locally
                  console.log(`📝 Updating placeholder node ${node.id} to type: ${e.detail.nodeType}`);
                  if ('updatePlaceholderNodeType' in nodeManager) {
                    (nodeManager as any).updatePlaceholderNodeType(node.id, e.detail.nodeType);
                  }
                } else {
                  // For real nodes, update node type with full persistence
                  console.log(`🔄 Updating real node ${node.id} to type: ${e.detail.nodeType}`);
                  nodeManager.updateNodeType(node.id, e.detail.nodeType);
                }

                // Set autoFocus to restore focus after nodeType change
                console.log(`🎯 Setting autoFocus for node ${node.id} after slash command`);
                focusedNodeId = node.id;
              }}
              on:combineWithPrevious={handleCombineWithPrevious}
              on:deleteNode={handleDeleteNode}
            />
          {:else}
            <!-- Use registered node component from plugin registry -->
            {#if loadedNodes.has(node.nodeType)}
              {@const NodeComponent = loadedNodes.get(node.nodeType)}
              <!-- Debug: Log node type and component -->
              {console.log(`🎯 Rendering node ${node.id} with type: ${node.nodeType}, component:`, NodeComponent)}
              <NodeComponent
                nodeId={node.id}
                nodeType={node.nodeType}
                autoFocus={node.autoFocus || node.id === focusedNodeId}
                content={node.content}
                headerLevel={node.inheritHeaderLevel || 0}
                children={node.children}
                metadata={node.metadata || {}}
                editableConfig={{ allowMultiline: true }}
                on:createNewNode={handleCreateNewNode}
                on:indentNode={handleIndentNode}
                on:outdentNode={handleOutdentNode}
                on:navigateArrow={handleArrowNavigation}
                on:contentChanged={(e) => {
                  const content = e.detail.content;

                  // Update node content (placeholder flag is handled automatically)
                  nodeManager.updateNodeContent(node.id, content);
                }}
                on:slashCommandSelected={(e) => {
                  console.log(`🎛️ Slash command selected for node ${node.id}:`, e.detail, 'isPlaceholder:', node.isPlaceholder);

                  // Store cursor position before node type change
                  if (e.detail.cursorPosition !== null && e.detail.cursorPosition !== undefined) {
                    console.log(`💾 Storing cursor position ${e.detail.cursorPosition} for node ${node.id}`);
                    pendingCursorPositions.set(node.id, e.detail.cursorPosition);
                  }

                  if (node.isPlaceholder) {
                    // For placeholder nodes, just update the nodeType locally
                    console.log(`📝 Updating placeholder node ${node.id} to type: ${e.detail.nodeType}`);
                    if ('updatePlaceholderNodeType' in nodeManager) {
                      (nodeManager as any).updatePlaceholderNodeType(node.id, e.detail.nodeType);
                    }
                  } else {
                    // For real nodes, update node type with full persistence
                    console.log(`🔄 Updating real node ${node.id} to type: ${e.detail.nodeType}`);
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }

                  // Set autoFocus to restore focus after nodeType change
                  console.log(`🎯 Setting autoFocus for node ${node.id} after slash command`);
                  focusedNodeId = node.id;
                }}
                on:iconClick={handleIconClick}
                on:combineWithPrevious={handleCombineWithPrevious}
                on:deleteNode={handleDeleteNode}
              />
            {:else}
              <!-- Final fallback to BaseNode -->
              <BaseNode
                nodeId={node.id}
                nodeType={node.nodeType}
                autoFocus={node.autoFocus || node.id === focusedNodeId}
                content={node.content}
                headerLevel={node.inheritHeaderLevel || 0}
                children={node.children}
                metadata={node.metadata || {}}
                editableConfig={{ allowMultiline: true }}
                on:createNewNode={handleCreateNewNode}
                on:indentNode={handleIndentNode}
                on:outdentNode={handleOutdentNode}
                on:navigateArrow={handleArrowNavigation}
                on:contentChanged={(e) => {
                  const content = e.detail.content;

                  // Update node content (placeholder flag is handled automatically)
                  nodeManager.updateNodeContent(node.id, content);
                }}
                on:slashCommandSelected={(e) => {
                  console.log(`🎛️ Slash command selected for node ${node.id}:`, e.detail, 'isPlaceholder:', node.isPlaceholder);

                  // Store cursor position before node type change
                  if (e.detail.cursorPosition !== null && e.detail.cursorPosition !== undefined) {
                    console.log(`💾 Storing cursor position ${e.detail.cursorPosition} for node ${node.id}`);
                    pendingCursorPositions.set(node.id, e.detail.cursorPosition);
                  }

                  if (node.isPlaceholder) {
                    // For placeholder nodes, just update the nodeType locally
                    console.log(`📝 Updating placeholder node ${node.id} to type: ${e.detail.nodeType}`);
                    if ('updatePlaceholderNodeType' in nodeManager) {
                      (nodeManager as any).updatePlaceholderNodeType(node.id, e.detail.nodeType);
                    }
                  } else {
                    // For real nodes, update node type with full persistence
                    console.log(`🔄 Updating real node ${node.id} to type: ${e.detail.nodeType}`);
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }

                  // Set autoFocus to restore focus after nodeType change
                  console.log(`🎯 Setting autoFocus for node ${node.id} after slash command`);
                  focusedNodeId = node.id;
                }}
                on:iconClick={handleIconClick}
                on:combineWithPrevious={handleCombineWithPrevious}
                on:deleteNode={handleDeleteNode}
              />
            {/if}
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
    /* No left padding - match patterns.html exactly */

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
