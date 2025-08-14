<!--
  BaseNodeViewer - Container that manages a collection of nodes
  Handles node creation, deletion, and organization
-->

<script lang="ts">
  import TextNode from '$lib/components/TextNode.svelte';
  import Icon from '$lib/design/icons';

  // Type aliases for DOM APIs to satisfy linter
  type DOMElement = globalThis.Element;
  type DOMText = globalThis.Text;

  // HTML to Markdown conversion (duplicate of MinimalBaseNode function)
  function htmlToMarkdown(htmlContent: string): string {
    let markdown = htmlContent;

    // Convert header tags to markdown syntax (check first to preserve header level)
    markdown = markdown.replace(/<h1[^>]*>(.*?)<\/h1>/g, '# $1');
    markdown = markdown.replace(/<h2[^>]*>(.*?)<\/h2>/g, '## $1');
    markdown = markdown.replace(/<h3[^>]*>(.*?)<\/h3>/g, '### $1');
    markdown = markdown.replace(/<h4[^>]*>(.*?)<\/h4>/g, '#### $1');
    markdown = markdown.replace(/<h5[^>]*>(.*?)<\/h5>/g, '##### $1');
    markdown = markdown.replace(/<h6[^>]*>(.*?)<\/h6>/g, '###### $1');

    // Convert HTML spans to markdown syntax
    markdown = markdown.replace(/<span class="markdown-bold">(.*?)<\/span>/g, '**$1**');
    markdown = markdown.replace(/<span class="markdown-italic">(.*?)<\/span>/g, '*$1*');
    markdown = markdown.replace(/<span class="markdown-underline">(.*?)<\/span>/g, '__$1__');

    // Clean up any remaining HTML tags
    markdown = markdown.replace(/<[^>]*>/g, '');

    return markdown;
  }

  // Markdown to HTML conversion (duplicate of MinimalBaseNode function)
  function markdownToHtml(markdownContent: string): string {
    let html = markdownContent;

    // Convert inline formatting only
    html = html.replace(/\*\*(.*?)\*\*/g, '<span class="markdown-bold">$1</span>');
    html = html.replace(/(?<!\*)\*([^*]+)\*(?!\*)/g, '<span class="markdown-italic">$1</span>');
    html = html.replace(/__(.*?)__/g, '<span class="markdown-underline">$1</span>');

    return html;
  }

  // Comprehensive test data with mixed node types including AI-chat nodes (canHaveChildren=false)
  let nodes = $state([
    {
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: '# Main Project Overview',
      inheritHeaderLevel: 1,
      children: [
        {
          id: globalThis.crypto.randomUUID(),
          type: 'text',
          autoFocus: false,
          content: '## Features with **bold** and *italic* text',
          inheritHeaderLevel: 2,
          children: [
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'User authentication with __underlined__ security',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'ai-chat',
              autoFocus: false,
              content:
                'AI Assistant: I can help explain authentication patterns. What specific approach are you considering?',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'Database operations',
              inheritHeaderLevel: 0,
              children: [
                {
                  id: globalThis.crypto.randomUUID(),
                  type: 'text',
                  autoFocus: false,
                  content: 'CRUD operations with **optimized** queries',
                  inheritHeaderLevel: 0,
                  children: [],
                  expanded: true
                },
                {
                  id: globalThis.crypto.randomUUID(),
                  type: 'ai-chat',
                  autoFocus: false,
                  content:
                    'AI Assistant: Consider using database connection pooling and prepared statements for better performance.',
                  inheritHeaderLevel: 0,
                  children: [],
                  expanded: true
                },
                {
                  id: globalThis.crypto.randomUUID(),
                  type: 'text',
                  autoFocus: false,
                  content: 'Data validation and *error handling*',
                  inheritHeaderLevel: 0,
                  children: [],
                  expanded: true
                },
                {
                  id: globalThis.crypto.randomUUID(),
                  type: 'text',
                  autoFocus: false,
                  content: 'Advanced features',
                  inheritHeaderLevel: 0,
                  children: [
                    {
                      id: globalThis.crypto.randomUUID(),
                      type: 'text',
                      autoFocus: false,
                      content: 'Real-time updates via __WebSocket__',
                      inheritHeaderLevel: 0,
                      children: [],
                      expanded: true
                    },
                    {
                      id: globalThis.crypto.randomUUID(),
                      type: 'text',
                      autoFocus: false,
                      content: 'Caching with **Redis** integration',
                      inheritHeaderLevel: 0,
                      children: [],
                      expanded: true
                    }
                  ],
                  expanded: true
                }
              ],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'API endpoints with *RESTful* design',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'ai-chat',
              autoFocus: false,
              content:
                'AI Assistant: RESTful APIs should follow HTTP verb conventions. Would you like me to suggest endpoint patterns?',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            }
          ],
          expanded: true
        },
        {
          id: globalThis.crypto.randomUUID(),
          type: 'text',
          autoFocus: false,
          content: '## Testing Strategy (collapsed)',
          inheritHeaderLevel: 2,
          children: [
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'Unit tests with **Jest** framework',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'ai-chat',
              autoFocus: false,
              content:
                'AI Assistant: Jest is excellent for unit testing. Consider adding coverage thresholds to maintain quality.',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'Integration tests for *API endpoints*',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'E2E testing',
              inheritHeaderLevel: 0,
              children: [
                {
                  id: globalThis.crypto.randomUUID(),
                  type: 'text',
                  autoFocus: false,
                  content: 'Playwright for __browser automation__',
                  inheritHeaderLevel: 0,
                  children: [],
                  expanded: true
                },
                {
                  id: globalThis.crypto.randomUUID(),
                  type: 'text',
                  autoFocus: false,
                  content: 'Visual regression with **Chromatic**',
                  inheritHeaderLevel: 0,
                  children: [],
                  expanded: true
                }
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
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: '### Development Notes',
      inheritHeaderLevel: 3,
      children: [
        {
          id: globalThis.crypto.randomUUID(),
          type: 'text',
          autoFocus: false,
          content: 'Use **TypeScript** for type safety',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: globalThis.crypto.randomUUID(),
          type: 'ai-chat',
          autoFocus: false,
          content:
            'AI Assistant: TypeScript catches errors at compile time. Try enabling strict mode for maximum safety.',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: globalThis.crypto.randomUUID(),
          type: 'text',
          autoFocus: false,
          content: 'Follow *ESLint* and __Prettier__ conventions',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: globalThis.crypto.randomUUID(),
          type: 'text',
          autoFocus: false,
          content: 'Git workflow',
          inheritHeaderLevel: 0,
          children: [
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'Feature branches with **descriptive** names',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'Pull requests with *code review*',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'ai-chat',
              autoFocus: false,
              content:
                'AI Assistant: Code reviews improve quality and knowledge sharing. Consider using review checklists.',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'Automated CI/CD with __GitHub Actions__',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            }
          ],
          expanded: true
        }
      ],
      expanded: true
    },
    {
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: 'Deployment (collapsed branch)',
      inheritHeaderLevel: 0,
      children: [
        {
          id: globalThis.crypto.randomUUID(),
          type: 'text',
          autoFocus: false,
          content: '**Production** environment setup',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: globalThis.crypto.randomUUID(),
          type: 'ai-chat',
          autoFocus: false,
          content:
            'AI Assistant: Production deployments should include health checks, monitoring, and rollback strategies.',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: globalThis.crypto.randomUUID(),
          type: 'text',
          autoFocus: false,
          content: 'Staging environment',
          inheritHeaderLevel: 0,
          children: [
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'Mirror of *production* with test data',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: globalThis.crypto.randomUUID(),
              type: 'text',
              autoFocus: false,
              content: 'Automated deployment from __main branch__',
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            }
          ],
          expanded: true
        },
        {
          id: globalThis.crypto.randomUUID(),
          type: 'text',
          autoFocus: false,
          content: 'Monitoring and *alerting* systems',
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        }
      ],
      expanded: false
    },
    {
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: 'Simple leaf node with **formatting**',
      inheritHeaderLevel: 0,
      children: [],
      expanded: true
    },
    {
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: 'Another leaf with *italics* and __underline__',
      inheritHeaderLevel: 0,
      children: [],
      expanded: true
    },
    {
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: 'Final node - can be indented into previous text node',
      inheritHeaderLevel: 0,
      children: [],
      expanded: true
    },
    {
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: '## Header Node - Cannot Be Joined',
      inheritHeaderLevel: 0,
      children: [],
      expanded: true
    },
    {
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: 'Regular text after header - can be joined with other regular text',
      inheritHeaderLevel: 0,
      children: [],
      expanded: true
    },
    {
      id: globalThis.crypto.randomUUID(),
      type: 'text',
      autoFocus: false,
      content: 'Another regular text node - try backspace at start to join with above',
      inheritHeaderLevel: 0,
      children: [],
      expanded: true
    },
    {
      id: globalThis.crypto.randomUUID(),
      type: 'ai-chat',
      autoFocus: false,
      content:
        'AI Assistant: This demonstrates mixed node types. Headers cannot be joined (try backspace after header), but regular text nodes can be joined together. AI-chat nodes also block joining.',
      inheritHeaderLevel: 0,
      children: [],
      expanded: true
    }
  ]);

  // Handle creating new nodes when Enter is pressed with sophisticated children transfer
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

    // Find the node and its parent recursively
    function findNodeLocation(
      nodesList: any[],
      targetId: string,
      parent: any = null,
      parentChildren: any[] | null = null
    ): { node: any; index: number; parent: any; parentChildren: any[] } | null {
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
        nodesList.forEach((n) => {
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
        id: globalThis.crypto.randomUUID(),
        type: nodeType,
        autoFocus: true,
        content: newContent || '', // Use split content or empty string
        inheritHeaderLevel: inheritHeaderLevel || 0, // Inherit header level from parent
        children: [], // Initialize with empty children array
        expanded: true // New nodes are expanded by default
      };

      // Enhanced Enter key behavior with children transfer based on collapsed state
      if (node.children && node.children.length > 0) {
        const isCollapsed = !node.expanded; // Check if the node is collapsed

        if (isCollapsed) {
          // When collapsed, children stay with the original (left) node
          // No children transfer needed - preserve existing structure
        } else {
          // When expanded, children move to the new (right) node using sophisticated transfer
          const collapsedNodes = new Set<string>();
          transferChildrenWithDepthPreservation(node, newNode, collapsedNodes);
        }
      }

      // Insert new node after the current one in the appropriate array
      parentChildren.splice(index + 1, 0, newNode);
      nodes = [...nodes]; // Trigger reactivity

      // If new content contains HTML formatting, convert to markdown after a tick
      if (newContent && newContent.includes('<span class="markdown-')) {
        setTimeout(() => {
          // Convert HTML content to markdown for storage
          const markdownContent = htmlToMarkdown(newContent);
          // Find the new node recursively in the updated tree
          function findAndUpdateNode(nodesList: any[], targetId: string): boolean {
            for (let i = 0; i < nodesList.length; i++) {
              if (nodesList[i].id === targetId) {
                nodesList[i].content = markdownContent;
                return true;
              }
              if (nodesList[i].children && nodesList[i].children.length > 0) {
                if (findAndUpdateNode(nodesList[i].children, targetId)) {
                  return true;
                }
              }
            }
            return false;
          }

          if (findAndUpdateNode(nodes, newNode.id)) {
            nodes = [...nodes]; // Trigger reactivity
          }
        }, 100); // Small delay to let the node initialize with HTML first
      }

      // Node created successfully with header inheritance and children transfer if applicable
    }
  }

  // Enhanced indent validation
  function canIndent(node: any, siblings: any[]): boolean {
    const nodeIndex = siblings.findIndex((s) => s.id === node.id);
    if (nodeIndex <= 0) return false; // No previous sibling

    const previousSibling = siblings[nodeIndex - 1];
    // Check if previous sibling can have children based on node type
    return previousSibling.type !== 'ai-chat'; // AI-chat nodes cannot have children
  }

  // Handle indenting nodes (Tab key) with enhanced validation
  function handleIndentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;

    // Store cursor position before DOM changes
    const cursorPosition = saveCursorPosition(nodeId);

    // Find the node and its context recursively
    function findNodeForIndent(
      nodesList: any[],
      targetId: string,
      parent: any = null,
      parentChildren: any[] | null = null
    ): {
      node: any;
      index: number;
      parent: any;
      parentChildren: any[];
      previousSibling: any;
    } | null {
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

      // Enhanced validation: Check if we can indent into the previous sibling
      if (!canIndent(node, parentChildren)) {
        // Cannot indent - previous sibling doesn't support children
        return;
      }

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

  function getTextNodes(element: DOMElement): DOMText[] {
    const textNodes: DOMText[] = [];
    const walker = document.createTreeWalker(element, globalThis.NodeFilter.SHOW_TEXT, null);

    let node = walker.nextNode();
    while (node) {
      textNodes.push(node as DOMText);
      node = walker.nextNode();
    }

    return textNodes;
  }

  // Enhanced outdent with sibling transfer (Shift+Tab key)
  function handleOutdentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;

    // Store cursor position before DOM changes
    const cursorPosition = saveCursorPosition(nodeId);

    // Find the node and move it up one level recursively with sibling transfer
    function findAndOutdentNode(
      nodesList: any[],
      targetId: string,
      grandparent: any = null,
      grandparentChildren: any[] | null = null
    ): boolean {
      for (let i = 0; i < nodesList.length; i++) {
        const parent = nodesList[i];
        if (parent.children && parent.children.length > 0) {
          const childIndex = parent.children.findIndex((child: any) => child.id === targetId);
          if (childIndex !== -1) {
            const node = parent.children[childIndex];

            // Sophisticated outdent with sibling transfer:
            // The next immediate sibling becomes a child of the outdented node
            const nextSiblingIndex = childIndex + 1;
            let nextSibling = null;
            if (nextSiblingIndex < parent.children.length) {
              nextSibling = parent.children[nextSiblingIndex];
            }

            // Remove node from parent's children
            parent.children.splice(childIndex, 1);

            // If there's a next sibling, transfer it to become child of outdented node
            if (nextSibling) {
              // Remove the next sibling from its current position
              const updatedNextSiblingIndex = parent.children.findIndex(
                (child: any) => child.id === nextSibling.id
              );
              if (updatedNextSiblingIndex !== -1) {
                parent.children.splice(updatedNextSiblingIndex, 1);
              }

              // Add next sibling as child of outdented node
              if (!node.children) {
                node.children = [];
              }
              node.children.push(nextSibling);

              // Auto-expand the outdented node since it received a new child
              node.expanded = true;
            }

            if (grandparent && grandparentChildren) {
              // Move to grandparent's children (one level up)
              const parentIndex = grandparentChildren.findIndex((n) => n.id === parent.id);
              if (parentIndex !== -1) {
                grandparentChildren.splice(parentIndex + 1, 0, node);
                return true;
              }
            } else {
              // Move to root level
              const parentIndex = nodes.findIndex((n) => n.id === parent.id);
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

  // Handle node joining requests from backspace
  function handleNodeJoining(event: CustomEvent<{
    currentNodeId: string;
    currentContent: string;
    currentNodeType: string;
    canBeCombined: boolean;
  }>) {
    const { currentNodeId, currentContent, canBeCombined } = event.detail;
    
    // Find the current node and get visible nodes
    const visibleNodes = getAllVisibleNodes(nodes);
    const currentIndex = visibleNodes.findIndex(n => n.id === currentNodeId);
    
    if (currentIndex <= 0) return; // No previous node or is first node
    
    const prevNode = visibleNodes[currentIndex - 1];
    const currentNode = visibleNodes[currentIndex];
    
    // Check if both nodes can be combined
    const prevCanBeCombined = canNodeBeCombined(prevNode);
    
    if (!canBeCombined || !prevCanBeCombined) {
      // If nodes can't be combined, handle empty node removal only
      if (currentContent.trim() === '') {
        handleEmptyNodeRemoval(currentNode, prevNode);
      }
      return;
    }
    
    // Perform content merge
    handleContentMerge(currentNode, prevNode);
  }

  // Check if a node can be combined based on its type and content
  function canNodeBeCombined(node: any): boolean {
    // For text nodes: check if it has header formatting
    if (node.type === 'text') {
      const hasHeaderFormatting = node.content.match(/^#{1,6}\s/);
      return !hasHeaderFormatting;
    }
    
    // AI-chat nodes cannot be combined
    if (node.type === 'ai-chat') {
      return false;
    }
    
    // Default: no combination allowed
    return false;
  }

  // Handle removal of empty nodes
  function handleEmptyNodeRemoval(currentNode: any, prevNode: any) {
    // Transfer children before removing node
    if (currentNode.children && currentNode.children.length > 0) {
      transferChildrenWithDepthPreservation(currentNode, prevNode, new Set());
    }
    
    // Remove current node from tree
    removeNodeFromTree(currentNode);
    
    // Update focus to previous node
    if (prevNode) {
      prevNode.autoFocus = true;
    }
    
    nodes = [...nodes]; // Trigger reactivity
  }

  // Handle content merging between nodes - simplified approach
  function handleContentMerge(currentNode: any, prevNode: any) {
    // Calculate junction position BEFORE updating content
    const junctionPosition = prevNode.content.length;
    
    // Simple content merge: prevNode.content + currentNode.content
    prevNode.content = prevNode.content + currentNode.content;
    
    // Transfer children to the target node
    if (currentNode.children && currentNode.children.length > 0) {
      transferChildrenWithDepthPreservation(currentNode, prevNode, new Set());
    }
    
    // Remove the current node from the tree
    removeNodeFromTree(currentNode);
    
    // Trigger Svelte reactivity to update the DOM
    nodes = [...nodes];
    
    // Position cursor at the junction point - account for HTML formatting
    setTimeout(() => {
      const targetElement = document.getElementById(`contenteditable-${prevNode.id}`);
      if (targetElement) {
        targetElement.focus();
        
        const selection = window.getSelection();
        if (selection) {
          // Calculate the actual DOM position by walking text nodes
          const textContent = targetElement.textContent || '';
          const markdownContent = prevNode.content.substring(0, junctionPosition);
          
          // Convert markdown to HTML to find actual DOM position
          const htmlContent = markdownToHtml(markdownContent);
          const tempDiv = document.createElement('div');
          tempDiv.innerHTML = htmlContent;
          const htmlTextLength = tempDiv.textContent?.length || 0;
          
          // Find the correct text node and position
          let currentOffset = 0;
          let targetTextNode = null;
          let targetOffset = htmlTextLength;
          
          const walker = document.createTreeWalker(
            targetElement,
            NodeFilter.SHOW_TEXT,
            null
          );
          
          let node = walker.nextNode();
          while (node && currentOffset < htmlTextLength) {
            const nodeLength = node.textContent?.length || 0;
            if (currentOffset + nodeLength >= htmlTextLength) {
              targetTextNode = node;
              targetOffset = htmlTextLength - currentOffset;
              break;
            }
            currentOffset += nodeLength;
            node = walker.nextNode();
          }
          
          // Set cursor position
          if (targetTextNode) {
            const range = document.createRange();
            range.setStart(targetTextNode, targetOffset);
            range.collapse(true);
            selection.removeAllRanges();
            selection.addRange(range);
          }
        }
      }
    }, 50);
  }

  // Remove a node from the tree structure
  function removeNodeFromTree(nodeToRemove: any): void {
    function removeFromParent(nodesList: any[], targetId: string): boolean {
      for (let i = 0; i < nodesList.length; i++) {
        if (nodesList[i].id === targetId) {
          nodesList.splice(i, 1);
          return true;
        }
        if (nodesList[i].children && nodesList[i].children.length > 0) {
          if (removeFromParent(nodesList[i].children, targetId)) {
            return true;
          }
        }
      }
      return false;
    }
    
    removeFromParent(nodes, nodeToRemove.id);
  }

  // Get all visible nodes in order (flattened tree)
  function getAllVisibleNodes(nodesList: any[]): any[] {
    const visible: any[] = [];
    
    function collectVisible(nodes: any[]) {
      for (const node of nodes) {
        visible.push(node);
        if (node.children && node.children.length > 0 && node.expanded) {
          collectVisible(node.children);
        }
      }
    }
    
    collectVisible(nodesList);
    return visible;
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

  // Sophisticated child transfer with depth-aware logic
  // Used for advanced delete operations and child management
  function transferChildrenWithDepthPreservation(
    sourceNode: any,
    targetNode: any,
    collapsedNodes: Set<string> = new Set()
  ): void {
    if (!sourceNode.children || sourceNode.children.length === 0) return;

    // Get the depth of the source node
    const sourceDepth = getNodeDepth(sourceNode);

    // Determine the appropriate parent for the transferred children
    let newParent: any;
    let insertAtBeginning = false;

    if (sourceDepth === 0) {
      // Source is a root node - children should go to target root ancestor
      newParent = findRootAncestor(targetNode);
      if (!newParent) return;
      insertAtBeginning = collapsedNodes.has(newParent.id);
    } else {
      // Source is non-root - children go directly to target
      newParent = targetNode;
      insertAtBeginning = collapsedNodes.has(targetNode.id);
    }

    // Transfer children with position awareness
    if (insertAtBeginning) {
      // Target was collapsed - insert children at the beginning
      const existingChildren = [...newParent.children];
      newParent.children = [...sourceNode.children, ...existingChildren];
    } else {
      // Target was expanded - append children at the end
      newParent.children.push(...sourceNode.children);
    }

    // Update parent references for all transferred children
    sourceNode.children.forEach((child: any) => {
      child.parent = newParent;
    });

    // Auto-expand the target node since it received new children
    if (collapsedNodes.has(newParent.id)) {
      collapsedNodes.delete(newParent.id);
      newParent.expanded = true;
    }

    // Clear the source node's children since they've been moved
    sourceNode.children = [];
  }

  // Helper function to get node depth in hierarchy
  function getNodeDepth(node: any): number {
    let depth = 0;
    let current = node.parent;
    while (current) {
      depth++;
      current = current.parent;
    }
    return depth;
  }

  // Helper function to find root ancestor
  function findRootAncestor(node: any): any | null {
    // Find the node in the nodes array (root level)
    function findInNodes(nodesList: any[], targetId: string): any | null {
      for (let i = 0; i < nodesList.length; i++) {
        if (nodesList[i].id === targetId) {
          return nodesList[i];
        }
        if (nodesList[i].children && nodesList[i].children.length > 0) {
          const result = findInNodes(nodesList[i].children, targetId);
          if (result) {
            // This node is not at root level, return the root ancestor
            return nodesList[i];
          }
        }
      }
      return null;
    }

    // If the node itself is already at root level
    if (nodes.some((n) => n.id === node.id)) {
      return node;
    }

    // Find the root ancestor
    return findInNodes(nodes, node.id);
  }
</script>

<div class="node-viewer">
  {#each nodes as node (node.id)}
    {#snippet renderNode(node: any)}
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
              nodeType={node.type}
              autoFocus={node.autoFocus}
              content={node.content}
              inheritHeaderLevel={node.inheritHeaderLevel}
              children={node.children}
              on:createNewNode={handleCreateNewNode}
              on:indentNode={handleIndentNode}
              on:outdentNode={handleOutdentNode}
              on:requestNodeJoining={handleNodeJoining}
              on:contentChanged={(e) => {
                // Find node recursively and update content directly (no reactivity trigger for typing)
                function updateNodeContent(nodesList: any[], targetId: string, newContent: string) {
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

                // Update content directly without triggering reactivity for normal typing
                updateNodeContent(nodes, node.id, e.detail.content);
                // DON'T trigger reactivity with nodes = [...nodes] for content changes
                // This prevents component recreation during typing
              }}
            />
          {:else if node.type === 'ai-chat'}
            <TextNode
              nodeId={node.id}
              nodeType={node.type}
              autoFocus={node.autoFocus}
              content={node.content}
              inheritHeaderLevel={node.inheritHeaderLevel}
              children={node.children}
              on:createNewNode={handleCreateNewNode}
              on:indentNode={handleIndentNode}
              on:outdentNode={handleOutdentNode}
              on:requestNodeJoining={handleNodeJoining}
              on:contentChanged={(e) => {
                // Find node recursively and update content directly (no reactivity trigger for typing)
                function updateNodeContent(nodesList: any[], targetId: string, newContent: string) {
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

                // Update content directly without triggering reactivity for normal typing
                updateNodeContent(nodes, node.id, e.detail.content);
                // DON'T trigger reactivity with nodes = [...nodes] for content changes
                // This prevents component recreation during typing
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
    --node-text: 142 71% 45%; /* Green for text nodes */
    --node-task: 25 95% 53%; /* Orange for task nodes */
    --node-ai-chat: 221 83% 53%; /* Blue for AI chat nodes */
    --node-entity: 271 81% 56%; /* Purple for entity nodes */
    --node-query: 330 81% 60%; /* Pink for query nodes */
  }

  .node-container {
    position: relative; /* Enable positioning context for child elements */
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
    margin-top: 0.425rem; /* Align with text like circle indicators - moved down slightly */
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
    transition:
      height 150ms ease-in-out,
      opacity 150ms ease-in-out;
  }

  /* Smooth transitions for collapse/expand */
  .node-children {
    overflow: hidden;
  }
</style>
