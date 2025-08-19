/**
 * Navigation utilities for arrow key navigation between nodes
 * Based on patterns from nodespace-core-ui
 */

export interface NavigableNode {
  id: string;
  children: NavigableNode[];
  expanded?: boolean;
}

export interface NavigationState {
  preferredColumn: number | null;
  resetCounter: number;
}

/**
 * Get all nodes in navigational order (flattened hierarchy)
 * Only includes visible/expanded nodes
 */
export function getVisibleNodes(nodes: NavigableNode[]): NavigableNode[] {
  const visible: NavigableNode[] = [];

  function visitRecursive(nodeList: NavigableNode[]) {
    for (const node of nodeList) {
      visible.push(node);

      // Only recurse into children if node is expanded (or expanded is undefined)
      if (node.children.length > 0 && (node.expanded === undefined || node.expanded === true)) {
        visitRecursive(node.children);
      }
    }
  }

  visitRecursive(nodes);
  return visible;
}

/**
 * Find a node by ID in the hierarchy
 */
export function findNodeById(nodes: NavigableNode[], nodeId: string): NavigableNode | null {
  for (const node of nodes) {
    if (node.id === nodeId) {
      return node;
    }

    if (node.children.length > 0) {
      const found = findNodeById(node.children, nodeId);
      if (found) return found;
    }
  }

  return null;
}

/**
 * Get the depth of a node in the hierarchy
 */
export function getNodeDepth(nodes: NavigableNode[], nodeId: string): number {
  function searchWithDepth(nodeList: NavigableNode[], currentDepth: number): number | null {
    for (const node of nodeList) {
      if (node.id === nodeId) {
        return currentDepth;
      }

      if (node.children.length > 0) {
        const foundDepth = searchWithDepth(node.children, currentDepth + 1);
        if (foundDepth !== null) return foundDepth;
      }
    }
    return null;
  }

  return searchWithDepth(nodes, 0) || 0;
}

/**
 * Calculate cursor position with simple indentation adjustment
 */
export function calculateVisualCursorPosition(
  content: string,
  isUpArrow: boolean,
  columnInCurrentLine: number,
  currentDepth: number,
  targetDepth: number
): number {
  const lines = content.split('\n');

  // Simple adjustment: each depth level = 2 spaces visual difference
  const depthDiff = targetDepth - currentDepth;
  let adjustedColumn = Math.max(0, columnInCurrentLine - depthDiff * 2);

  if (isUpArrow) {
    // Up: position in last line
    const lastLine = lines[lines.length - 1];
    const lastLineStart = content.length - lastLine.length;
    return lastLineStart + Math.min(adjustedColumn, lastLine.length);
  } else {
    // Down: position in first line
    const firstLine = lines[0] || '';
    return Math.min(adjustedColumn, firstLine.length);
  }
}

/**
 * Legacy function for backward compatibility
 */
export function calculateCursorPosition(
  content: string,
  isUpArrow: boolean,
  columnInCurrentLine: number,
  currentDepth: number,
  targetDepth: number
): number {
  const result = calculateVisualCursorPosition(
    content,
    isUpArrow,
    columnInCurrentLine,
    currentDepth,
    targetDepth
  );
  return result.position;
}

/**
 * Check if cursor is at a node boundary for navigation
 */
export function isAtNodeBoundary(
  content: string,
  cursorPosition: number,
  isUpArrow: boolean
): { isAtBoundary: boolean; currentLineIndex: number; columnInCurrentLine: number } {
  const lines = content.split('\n');
  const textBeforeCursor = content.substring(0, cursorPosition);
  const currentLineIndex = textBeforeCursor.split('\n').length - 1;
  const currentLineStartIndex = textBeforeCursor.lastIndexOf('\n') + 1;
  const columnInCurrentLine = cursorPosition - currentLineStartIndex;

  let isAtBoundary = false;

  if (isUpArrow) {
    // For up arrow: at boundary if cursor is in first line
    isAtBoundary = currentLineIndex === 0;
  } else {
    // For down arrow: at boundary if cursor is in last line
    isAtBoundary = currentLineIndex === lines.length - 1;
  }

  return {
    isAtBoundary,
    currentLineIndex,
    columnInCurrentLine
  };
}

/**
 * Get the next or previous navigable node with hierarchy awareness
 */
export function getTargetNode(
  nodes: NavigableNode[],
  currentNodeId: string,
  direction: 'up' | 'down'
): NavigableNode | null {
  const visibleNodes = getVisibleNodes(nodes);
  const currentIndex = visibleNodes.findIndex((n) => n.id === currentNodeId);

  if (currentIndex === -1) return null;

  const targetIndex = direction === 'up' ? currentIndex - 1 : currentIndex + 1;

  if (targetIndex >= 0 && targetIndex < visibleNodes.length) {
    return visibleNodes[targetIndex];
  }

  return null;
}

/**
 * Get contextual navigation info including parent/child relationships
 */
export function getNavigationContext(
  nodes: NavigableNode[],
  currentNodeId: string
): {
  currentNode: NavigableNode | null;
  parent: NavigableNode | null;
  depth: number;
  isFirstChild: boolean;
  isLastChild: boolean;
  nextSibling: NavigableNode | null;
  prevSibling: NavigableNode | null;
} {
  function searchWithContext(
    nodeList: NavigableNode[],
    parent: NavigableNode | null = null,
    depth: number = 0
  ): typeof result | null {
    for (let i = 0; i < nodeList.length; i++) {
      const node = nodeList[i];

      if (node.id === currentNodeId) {
        return {
          currentNode: node,
          parent,
          depth,
          isFirstChild: i === 0,
          isLastChild: i === nodeList.length - 1,
          nextSibling: i < nodeList.length - 1 ? nodeList[i + 1] : null,
          prevSibling: i > 0 ? nodeList[i - 1] : null
        };
      }

      if (node.children.length > 0) {
        const found = searchWithContext(node.children, node, depth + 1);
        if (found) return found;
      }
    }
    return null;
  }

  const result = {
    currentNode: null as NavigableNode | null,
    parent: null as NavigableNode | null,
    depth: 0,
    isFirstChild: false,
    isLastChild: false,
    nextSibling: null as NavigableNode | null,
    prevSibling: null as NavigableNode | null
  };

  return searchWithContext(nodes) || result;
}

/**
 * Reset navigation state (used when user explicitly moves cursor)
 */
export function resetNavigationState(navigationState: NavigationState): void {
  navigationState.preferredColumn = null;
  navigationState.resetCounter += 1;
}
