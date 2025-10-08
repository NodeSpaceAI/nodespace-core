/**
 * Mock ReactiveNodeService for testing
 * Provides the same API without Svelte 5 runes dependency
 */

import { v4 as uuidv4 } from 'uuid';
import { ContentProcessor } from '../../lib/services/content-processor';

export interface Node {
  id: string;
  content: string;
  nodeType: string;
  depth: number;
  parentId?: string;
  children: string[];
  expanded: boolean;
  autoFocus: boolean;
  inheritHeaderLevel: number;
  metadata: Record<string, unknown>;
  mentions?: string[];
  beforeSiblingId?: string;
  isPlaceholder?: boolean;
}

export interface NodeManagerEvents {
  focusRequested: (nodeId: string, position?: number) => void;
  hierarchyChanged: () => void;
  nodeCreated: (nodeId: string) => void;
  nodeDeleted: (nodeId: string) => void;
}

export function createMockReactiveNodeService(events: NodeManagerEvents) {
  // Plain objects without Svelte runes
  const _nodes: Record<string, Node> = {};
  let _rootNodeIds: string[] = [];
  const _collapsedNodes = new Set<string>();
  let _activeNodeId: string | undefined = undefined;
  let _updateTrigger = 0;

  // ContentProcessor instance for content processing methods
  const contentProcessor = ContentProcessor.getInstance();

  function getVisibleNodesRecursive(nodeIds: string[]): Node[] {
    const result: Node[] = [];

    for (const nodeId of nodeIds) {
      const node = _nodes[nodeId];
      if (node) {
        result.push(node);

        // Add children if node is expanded
        if (node.expanded && !_collapsedNodes.has(nodeId) && node.children.length > 0) {
          result.push(...getVisibleNodesRecursive(node.children));
        }
      }
    }

    return result;
  }

  function findNode(nodeId: string): Node | null {
    const node = _nodes[nodeId] || null;
    return node;
  }

  function clearAllAutoFocus(): void {
    for (const [nodeId, node] of Object.entries(_nodes)) {
      if (node.autoFocus) {
        const updatedNode = { ...node, autoFocus: false };
        _nodes[nodeId] = updatedNode;
      }
    }
  }

  /**
   * Create a node via the mock service (internal service API, not a test helper)
   *
   * This is part of the mock ReactiveNodeService interface.
   * For creating test data, use createTestNode() from @tests/helpers instead.
   */
  function createNode(
    afterNodeId: string,
    content: string = '',
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning?: boolean
  ): string {
    const afterNode = findNode(afterNodeId);
    if (!afterNode) {
      return '';
    }

    // Clear autoFocus from all existing nodes
    clearAllAutoFocus();

    const nodeId = uuidv4();

    // Determine depth and parent based on insertion strategy
    let newDepth: number;
    let newParentId: string | undefined;

    if (insertAtBeginning) {
      // Node goes above afterNode with same parent and depth
      newDepth = afterNode.depth;
      newParentId = afterNode.parentId;
    } else {
      // Node goes after afterNode - always create as sibling
      newDepth = afterNode.depth;
      newParentId = afterNode.parentId;
    }

    const newNode: Node = {
      id: nodeId,
      content,
      nodeType,
      depth: newDepth,
      parentId: newParentId,
      children: [],
      expanded: true,
      autoFocus: true,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterNode.inheritHeaderLevel,
      metadata: {},
      isPlaceholder: content.trim() === '' || /^#{1,6}\s*$/.test(content.trim())
    };

    _nodes[nodeId] = newNode;

    events.nodeCreated(nodeId);

    return nodeId;
  }

  function updateNodeContent(nodeId: string, content: string): void {
    const node = _nodes[nodeId];
    if (!node) {
      return;
    }

    // Update local state
    const headerLevel = contentProcessor.parseHeaderLevel(content);
    _nodes[nodeId] = {
      ...node,
      content,
      isPlaceholder: content.trim() === '',
      inheritHeaderLevel: headerLevel
    };
  }

  return {
    // Reactive getters
    get nodes() {
      return new Map(Object.entries(_nodes));
    },
    get rootNodeIds() {
      return _rootNodeIds;
    },
    get activeNodeId() {
      return _activeNodeId;
    },
    get visibleNodes() {
      return getVisibleNodesRecursive(_rootNodeIds);
    },
    get _updateTrigger() {
      return _updateTrigger;
    },

    // Node operations
    findNode,
    createNode,
    updateNodeContent,

    // Content processing methods for integration tests
    parseNodeContent(nodeId: string) {
      const node = findNode(nodeId);
      if (!node) return null;
      return contentProcessor.parseMarkdown(node.content);
    },

    async renderNodeAsHTML(nodeId: string): Promise<string> {
      const node = findNode(nodeId);
      if (!node) return '';
      const result = await contentProcessor.markdownToDisplay(node.content);
      return result || '';
    },

    getNodeHeaderLevel(nodeId: string): number {
      const node = findNode(nodeId);
      if (!node) return 0;
      const headerMatch = node.content.match(/^(#{1,6})\s+/);
      return headerMatch ? headerMatch[1].length : 0;
    },

    getNodeDisplayText(nodeId: string): string {
      const node = findNode(nodeId);
      if (!node) return '';
      return contentProcessor
        .displayToMarkdown(node.content)
        .replace(/[#*`[\]()]/g, '')
        .trim();
    },

    updateNodeContentWithProcessing(nodeId: string, content: string): boolean {
      const node = findNode(nodeId);
      if (!node) return false;
      updateNodeContent(nodeId, content);
      return true;
    },

    // Initialize nodes with UI state options
    initializeNodes(
      nodes: Array<import('$lib/types/node').Node>,
      options?: {
        expanded?: boolean;
        autoFocus?: boolean;
        inheritHeaderLevel?: number;
      }
    ): void {
      // Clear existing data
      Object.keys(_nodes).forEach((id) => delete _nodes[id]);
      _rootNodeIds = [];

      const defaultOptions = {
        expanded: options?.expanded ?? true,
        autoFocus: options?.autoFocus ?? false,
        inheritHeaderLevel: options?.inheritHeaderLevel ?? 0
      };

      // Convert unified format to internal format
      for (const unifiedNode of nodes) {
        const node: Node = {
          id: unifiedNode.id,
          content: unifiedNode.content,
          nodeType: unifiedNode.nodeType,
          depth: 0, // Will be calculated based on hierarchy
          parentId: unifiedNode.parentId || undefined,
          children: [], // Will be computed from parent_id relationships
          expanded: defaultOptions.expanded,
          autoFocus: defaultOptions.autoFocus,
          inheritHeaderLevel: defaultOptions.inheritHeaderLevel,
          metadata: unifiedNode.properties,
          mentions: unifiedNode.mentions,
          beforeSiblingId: unifiedNode.beforeSiblingId || undefined
        };
        _nodes[unifiedNode.id] = node;
      }

      // Compute children arrays from parent_id relationships
      for (const node of Object.values(_nodes)) {
        node.children = [];
      }
      for (const node of Object.values(_nodes)) {
        if (node.parentId && _nodes[node.parentId]) {
          _nodes[node.parentId].children.push(node.id);
        }
      }

      // Set root nodes (nodes without parents)
      _rootNodeIds = nodes.filter((n) => !n.parentId).map((n) => n.id);

      // Calculate depths based on parent hierarchy
      const calculateDepth = (nodeId: string, depth: number = 0): void => {
        const node = _nodes[nodeId];
        if (node) {
          node.depth = depth;
          for (const childId of node.children) {
            calculateDepth(childId, depth + 1);
          }
        }
      };
      for (const rootId of _rootNodeIds) {
        calculateDepth(rootId, 0);
      }

      // Trigger reactivity
      _updateTrigger++;
    }
  };
}

export type MockReactiveNodeService = ReturnType<typeof createMockReactiveNodeService>;
