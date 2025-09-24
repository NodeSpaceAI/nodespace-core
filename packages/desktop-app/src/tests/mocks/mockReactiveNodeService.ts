/**
 * Mock ReactiveNodeService for testing
 * Provides the same API without Svelte 5 runes dependency
 */

import { v4 as uuidv4 } from 'uuid';
import { ContentProcessor } from '../../lib/services/contentProcessor';
import { eventBus } from '../../lib/services/eventBus';

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
  before_sibling_id?: string;
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

    // Initialize from legacy data
    initializeFromLegacyData(
      legacyData: Array<{
        id: string;
        type?: string;
        nodeType?: string;
        content: string;
        inheritHeaderLevel: number;
        children: string[];
        expanded: boolean;
        autoFocus: boolean;
        metadata?: Record<string, unknown>;
        parentId?: string;
      }>
    ): void {
      // Clear existing data
      Object.keys(_nodes).forEach((id) => delete _nodes[id]);
      _rootNodeIds = [];

      // Convert legacy data to new format
      for (const legacyNode of legacyData) {
        const node: Node = {
          id: legacyNode.id,
          content: legacyNode.content,
          nodeType: legacyNode.type || legacyNode.nodeType || 'text',
          depth: 0, // Will be calculated based on hierarchy
          parentId: undefined, // Will be set based on children relationships
          children: [...legacyNode.children],
          expanded: legacyNode.expanded,
          autoFocus: legacyNode.autoFocus,
          inheritHeaderLevel: legacyNode.inheritHeaderLevel,
          metadata: {}
        };
        _nodes[legacyNode.id] = node;
      }

      // Set root nodes (nodes not referenced as children)
      const allChildIds = new Set(legacyData.flatMap((n) => n.children));
      _rootNodeIds = legacyData.filter((n) => !allChildIds.has(n.id)).map((n) => n.id);

      // Update parent references and depths
      for (const node of Object.values(_nodes)) {
        for (const childId of node.children) {
          const child = _nodes[childId];
          if (child) {
            child.parentId = node.id;
            child.depth = node.depth + 1;
          }
        }
      }

      // Trigger reactivity
      _updateTrigger++;
    }
  };
}

export type MockReactiveNodeService = ReturnType<typeof createMockReactiveNodeService>;