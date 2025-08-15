<!--
  BaseNodeViewer - Container that manages a collection of nodes
  Handles node creation, deletion, and organization
-->

<script lang="ts">
  import TextNode from '$lib/components/TextNode.svelte';
  import Icon from '$lib/design/icons';
  import { htmlToMarkdown } from '$lib/utils/markdown.js';
  import { v4 as uuidv4 } from 'uuid';

  // Comprehensive test data for collapse/expand with formatting and deep nesting
  let nodes = $state([
    { 
      id: uuidv4(), 
      type: 'text', 
      autoFocus: false, 
      content: '# Main Project Overview', 
      inheritHeaderLevel: 1, 
      children: [
        { 
          id: uuidv4(), 
          type: 'text', 
          autoFocus: false, 
          content: '## Features with **bold** and *italic* text', 
          inheritHeaderLevel: 2, 
          children: [
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'User authentication with __underlined__ security', inheritHeaderLevel: 0, children: [], expanded: true },
            { 
              id: uuidv4(), 
              type: 'text', 
              autoFocus: false, 
              content: 'Database operations', 
              inheritHeaderLevel: 0, 
              children: [
                { id: uuidv4(), type: 'text', autoFocus: false, content: 'CRUD operations with **optimized** queries', inheritHeaderLevel: 0, children: [], expanded: true },
                { id: uuidv4(), type: 'text', autoFocus: false, content: 'Data validation and *error handling*', inheritHeaderLevel: 0, children: [], expanded: true },
                { 
                  id: uuidv4(), 
                  type: 'text', 
                  autoFocus: false, 
                  content: 'Advanced features', 
                  inheritHeaderLevel: 0, 
                  children: [
                    { id: uuidv4(), type: 'text', autoFocus: false, content: 'Real-time updates via __WebSocket__', inheritHeaderLevel: 0, children: [], expanded: true },
                    { id: uuidv4(), type: 'text', autoFocus: false, content: 'Caching with **Redis** integration', inheritHeaderLevel: 0, children: [], expanded: true }
                  ],
                  expanded: true 
                }
              ],
              expanded: true 
            },
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'API endpoints with *RESTful* design', inheritHeaderLevel: 0, children: [], expanded: true }
          ],
          expanded: true 
        },
        { 
          id: uuidv4(), 
          type: 'text', 
          autoFocus: false, 
          content: '## Testing Strategy (collapsed)', 
          inheritHeaderLevel: 2, 
          children: [
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'Unit tests with **Jest** framework', inheritHeaderLevel: 0, children: [], expanded: true },
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'Integration tests for *API endpoints*', inheritHeaderLevel: 0, children: [], expanded: true },
            { 
              id: uuidv4(), 
              type: 'text', 
              autoFocus: false, 
              content: 'E2E testing', 
              inheritHeaderLevel: 0, 
              children: [
                { id: uuidv4(), type: 'text', autoFocus: false, content: 'Playwright for __browser automation__', inheritHeaderLevel: 0, children: [], expanded: true },
                { id: uuidv4(), type: 'text', autoFocus: false, content: 'Visual regression with **Chromatic**', inheritHeaderLevel: 0, children: [], expanded: true }
              ],
              expanded: true 
            }
          ], 
          expanded: false 
        }
      ], 
      expanded: true 
    },
    { 
      id: uuidv4(), 
      type: 'text', 
      autoFocus: false, 
      content: '### Development Notes', 
      inheritHeaderLevel: 3, 
      children: [
        { id: uuidv4(), type: 'text', autoFocus: false, content: 'Use **TypeScript** for type safety', inheritHeaderLevel: 0, children: [], expanded: true },
        { id: uuidv4(), type: 'text', autoFocus: false, content: 'Follow *ESLint* and __Prettier__ conventions', inheritHeaderLevel: 0, children: [], expanded: true },
        { 
          id: uuidv4(), 
          type: 'text', 
          autoFocus: false, 
          content: 'Git workflow', 
          inheritHeaderLevel: 0, 
          children: [
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'Feature branches with **descriptive** names', inheritHeaderLevel: 0, children: [], expanded: true },
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'Pull requests with *code review*', inheritHeaderLevel: 0, children: [], expanded: true },
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'Automated CI/CD with __GitHub Actions__', inheritHeaderLevel: 0, children: [], expanded: true }
          ],
          expanded: true 
        }
      ], 
      expanded: true 
    },
    { 
      id: uuidv4(), 
      type: 'text', 
      autoFocus: false, 
      content: 'Deployment (collapsed branch)', 
      inheritHeaderLevel: 0, 
      children: [
        { id: uuidv4(), type: 'text', autoFocus: false, content: '**Production** environment setup', inheritHeaderLevel: 0, children: [], expanded: true },
        { 
          id: uuidv4(), 
          type: 'text', 
          autoFocus: false, 
          content: 'Staging environment', 
          inheritHeaderLevel: 0, 
          children: [
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'Mirror of *production* with test data', inheritHeaderLevel: 0, children: [], expanded: true },
            { id: uuidv4(), type: 'text', autoFocus: false, content: 'Automated deployment from __main branch__', inheritHeaderLevel: 0, children: [], expanded: true }
          ],
          expanded: true 
        },
        { id: uuidv4(), type: 'text', autoFocus: false, content: 'Monitoring and *alerting* systems', inheritHeaderLevel: 0, children: [], expanded: true }
      ], 
      expanded: false 
    },
    { id: uuidv4(), type: 'text', autoFocus: false, content: 'Simple leaf node with **formatting**', inheritHeaderLevel: 0, children: [], expanded: true },
    { id: uuidv4(), type: 'text', autoFocus: false, content: 'Another leaf with *italics* and __underline__', inheritHeaderLevel: 0, children: [], expanded: true }
  ]);

  // Handle creating new nodes when Enter is pressed
  function handleCreateNewNode(event: CustomEvent<{ 
    afterNodeId: string; 
    nodeType: string; 
    currentContent?: string; 
    newContent?: string;
    inheritHeaderLevel?: number; 
  }>) {
    const { afterNodeId, nodeType, currentContent, newContent, inheritHeaderLevel } = event.detail;
    
    // Find the node and its parent recursively
    function findNodeLocation(nodesList: any[], targetId: string, parent: any = null, parentChildren: any[] = null): 
      { node: any, index: number, parent: any, parentChildren: any[] } | null {
      
      for (let i = 0; i < nodesList.length; i++) {
        const node = nodesList[i];
        if (node.id === targetId) {
          return { node, index: i, parent, parentChildren: parentChildren || nodesList };
        }
        
        if (node.children && node.children.length > 0) {
          const result = findNodeLocation(node.children, targetId, node, node.children);
          if (result) return result;
        }
      }
      return null;
    }
    
    const nodeLocation = findNodeLocation(nodes, afterNodeId);
    
    if (nodeLocation) {
      const { node, index, parentChildren } = nodeLocation;
      
      // First, set all existing nodes to not auto-focus recursively
      function clearAutoFocus(nodesList: any[]) {
        nodesList.forEach(n => {
          n.autoFocus = false;
          if (n.children) clearAutoFocus(n.children);
        });
      }
      clearAutoFocus(nodes);
      
      // Update current node content if split content is provided
      if (currentContent !== undefined) {
        node.content = currentContent;
      }
      
      // Create new node with UUID, auto-focus, and header inheritance
      const newNode = {
        id: uuidv4(),
        type: nodeType,
        autoFocus: true,
        content: newContent || '', // Use split content or empty string
        inheritHeaderLevel: inheritHeaderLevel || 0, // Inherit header level from parent
        children: [], // Initialize with empty children array
        expanded: true // New nodes are expanded by default
      };
      
      // Insert new node after the current one in the appropriate array
      parentChildren.splice(index + 1, 0, newNode);
      nodes = [...nodes]; // Trigger reactivity
      
      // If new content contains HTML formatting, convert to markdown after a tick
      if (newContent && newContent.includes('<span class="markdown-')) {
        setTimeout(() => {
          // Convert HTML content to markdown for storage
          const markdownContent = htmlToMarkdown(newContent);
          const nodeIndex = nodes.findIndex(n => n.id === newNode.id);
          if (nodeIndex !== -1) {
            nodes[nodeIndex].content = markdownContent;
            nodes = [...nodes]; // Trigger reactivity
          }
        }, 100); // Small delay to let the node initialize with HTML first
      }
      
      // Node created successfully with header inheritance if applicable
    }
  }

  // Handle indenting nodes (Tab key)
  function handleIndentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;
    
    // Store cursor position before DOM changes
    const cursorPosition = saveCursorPosition(nodeId);
    
    // Find the node and its context recursively
    function findNodeForIndent(nodesList: any[], targetId: string, parent: any = null, parentChildren: any[] = null): 
      { node: any, index: number, parent: any, parentChildren: any[], previousSibling: any } | null {
      
      for (let i = 0; i < nodesList.length; i++) {
        const node = nodesList[i];
        if (node.id === targetId && i > 0) {
          // Found the target node and it has a previous sibling
          return { 
            node, 
            index: i, 
            parent, 
            parentChildren: parentChildren || nodesList,
            previousSibling: nodesList[i - 1]
          };
        }
        
        if (node.children && node.children.length > 0) {
          const result = findNodeForIndent(node.children, targetId, node, node.children);
          if (result) return result;
        }
      }
      return null;
    }
    
    const nodeInfo = findNodeForIndent(nodes, nodeId);
    
    if (nodeInfo) {
      const { node, index, parentChildren, previousSibling } = nodeInfo;
      
      // Remove node from current position
      parentChildren.splice(index, 1);
      
      // Add node to previous sibling's children
      if (!previousSibling.children) {
        previousSibling.children = [];
      }
      previousSibling.children.push(node);
      
      // Trigger reactivity
      nodes = [...nodes];
      
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
    const walker = document.createTreeWalker(
      element,
      NodeFilter.SHOW_TEXT,
      null
    );
    
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
    
    // Find the node and move it up one level recursively
    function findAndOutdentNode(nodesList: any[], targetId: string, grandparent: any = null, grandparentChildren: any[] = null): boolean {
      for (let i = 0; i < nodesList.length; i++) {
        const parent = nodesList[i];
        if (parent.children && parent.children.length > 0) {
          const childIndex = parent.children.findIndex((child: any) => child.id === targetId);
          if (childIndex !== -1) {
            const node = parent.children[childIndex];
            
            // Remove from parent's children
            parent.children.splice(childIndex, 1);
            
            if (grandparent && grandparentChildren) {
              // Move to grandparent's children (one level up)
              const parentIndex = grandparentChildren.findIndex(n => n.id === parent.id);
              if (parentIndex !== -1) {
                grandparentChildren.splice(parentIndex + 1, 0, node);
                return true;
              }
            } else {
              // Move to root level
              const parentIndex = nodes.findIndex(n => n.id === parent.id);
              if (parentIndex !== -1) {
                nodes.splice(parentIndex + 1, 0, node);
                return true;
              }
            }
          }
          
          // Recurse into deeper levels
          if (findAndOutdentNode(parent.children, targetId, parent, parent.children)) {
            return true;
          }
        }
      }
      return false;
    }

    if (findAndOutdentNode(nodes, nodeId)) {
      nodes = [...nodes];
      
      // Restore cursor position after DOM update
      setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
    }
  }

  // Handle chevron click to toggle expand/collapse
  function handleToggleExpanded(nodeId: string) {
    // Find node recursively and toggle its expanded state
    function toggleNodeExpanded(nodesList: any[], targetId: string): boolean {
      for (let i = 0; i < nodesList.length; i++) {
        if (nodesList[i].id === targetId) {
          nodesList[i].expanded = !nodesList[i].expanded;
          return true;
        }
        if (nodesList[i].children && nodesList[i].children.length > 0) {
          if (toggleNodeExpanded(nodesList[i].children, targetId)) {
            return true;
          }
        }
      }
      return false;
    }

    if (toggleNodeExpanded(nodes, nodeId)) {
      nodes = [...nodes]; // Trigger reactivity
    }
  }

  // Get node type color from design system
  function getNodeColor(nodeType: string): string {
    return `hsl(var(--node-${nodeType}, var(--node-text)))`;
  }

  // Handle arrow key navigation between nodes using entry/exit methods
  function handleArrowNavigation(event: CustomEvent<{ 
    nodeId: string; 
    direction: 'up' | 'down'; 
    columnHint: number;
  }>) {
    const { nodeId, direction, columnHint } = event.detail;
    
    // Find navigable nodes (respecting node autonomy)
    function findNavigableNodes(): any[] {
      const result: any[] = [];
      
      function collectNodes(nodeList: any[]) {
        for (const node of nodeList) {
          result.push(node);
          if (node.children && node.children.length > 0 && node.expanded) {
            collectNodes(node.children);
          }
        }
      }
      
      collectNodes(nodes);
      return result;
    }
    
    const navigableNodes = findNavigableNodes();
    const currentIndex = navigableNodes.findIndex(n => n.id === nodeId);
    
    if (currentIndex === -1) return;
    
    // Find next navigable node that accepts navigation
    let targetIndex = direction === 'up' ? currentIndex - 1 : currentIndex + 1;
    
    while (targetIndex >= 0 && targetIndex < navigableNodes.length) {
      const candidateNode = navigableNodes[targetIndex];
      
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
  function enterNodeAtPosition(targetNodeId: string, direction: 'up' | 'down', columnHint: number): boolean {
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
</script>

<div class="node-viewer">
  {#each nodes as node (node.id)}
    {#snippet renderNode(node)}
      <div class="node-container" data-has-children={node.children?.length > 0}>
        <div class="node-content-wrapper">
          <!-- Chevron for parent nodes (only visible on hover) -->
          {#if node.children && node.children.length > 0}
            <button 
              class="node-chevron" 
              class:expanded={node.expanded}
              onclick={() => handleToggleExpanded(node.id)}
              aria-label={node.expanded ? 'Collapse' : 'Expand'}
            >
              <Icon name="chevron-right" size={16} color={getNodeColor(node.type)} />
            </button>
          {:else}
            <!-- Spacer for nodes without children to maintain alignment -->
            <div class="node-chevron-spacer"></div>
          {/if}

          {#if node.type === 'text'}
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
                // Find node recursively and update content
                function updateNodeContent(nodesList, targetId, newContent) {
                  for (let i = 0; i < nodesList.length; i++) {
                    if (nodesList[i].id === targetId) {
                      nodesList[i].content = newContent;
                      return true;
                    }
                    if (nodesList[i].children.length > 0) {
                      if (updateNodeContent(nodesList[i].children, targetId, newContent)) {
                        return true;
                      }
                    }
                  }
                  return false;
                }
                
                updateNodeContent(nodes, node.id, e.detail.content);
                nodes = [...nodes]; // Trigger reactivity
              }}
            />
          {/if}
        </div>
        
        <!-- Render children with indentation (only if expanded) -->
        {#if node.children && node.children.length > 0 && node.expanded}
          <div class="node-children">
            {#each node.children as childNode (childNode.id)}
              {@render renderNode(childNode)}
            {/each}
          </div>
        {/if}
      </div>
    {/snippet}
    
    {@render renderNode(node)}
  {/each}
</div>

<style>
  .node-viewer {
    /* Container for nodes - document-like spacing */
    display: flex;
    flex-direction: column;
    gap: 0; /* 0px gap - all spacing from node padding for 8px total */
    
    /* NodeSpace Extension Colors - Node type colors per design system */
    --node-text: 142 71% 45%;          /* Green for text nodes */
    --node-task: 25 95% 53%;           /* Orange for task nodes */  
    --node-ai-chat: 221 83% 53%;       /* Blue for AI chat nodes */
    --node-entity: 271 81% 56%;        /* Purple for entity nodes */
    --node-query: 330 81% 60%;         /* Pink for query nodes */
  }

  .node-container {
    /* Individual node wrapper - no additional spacing */
  }

  .node-content-wrapper {
    /* Wrapper for chevron + content */
    display: flex;
    align-items: flex-start;
    gap: 0.25rem; /* 4px gap between chevron/spacer and text content */
  }

  /* Chevron styling following design system */
  .node-chevron {
    opacity: 0;
    background: none;
    border: none;
    padding: 0.125rem; /* 2px padding for clickable area */
    margin-top: 0.4rem; /* Align with text like circle indicators - adjusted positioning */
    cursor: pointer;
    border-radius: 0.125rem; /* 2px border radius */
    /* No transition - instant appearance/rotation */
    transform-origin: center;
    flex-shrink: 0;
    width: 20px; /* Explicit width: 16px icon + 4px padding */
    height: 20px; /* Match height for consistent alignment */
    display: flex;
    align-items: center;
    justify-content: center;
    position: relative;
    right: -0.35rem; /* 5.6px - move chevron right to center it between parent and child circle lines */
  }


  .node-chevron:focus {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
  }

  /* Show chevron only when hovering over the circle indicator itself */
  .node-content-wrapper:hover .node-chevron {
    opacity: 1;
  }

  /* Expanded state: rotate 90 degrees to point down */
  .node-chevron.expanded {
    transform: rotate(90deg);
  }

  /* Spacer for alignment when no chevron */
  .node-chevron-spacer {
    width: 20px; /* Match chevron button width: 16px icon + 4px padding */
    height: 20px; /* Match chevron button height */
    margin-top: 0.28rem; /* Match chevron margin-top for perfect alignment */
    flex-shrink: 0;
  }

  .node-children {
    /* Indent child nodes visually - increased to account for chevron space */
    margin-left: 2.5rem; /* 40px indentation: 16px circle + 24px gap (matches parent spacing) */
    transition: height 150ms ease-in-out, opacity 150ms ease-in-out;
  }

  /* Smooth transitions for collapse/expand */
  .node-children {
    overflow: hidden;
  }
</style>