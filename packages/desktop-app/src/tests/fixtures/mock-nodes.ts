/**
 * Simple mock node data for testing
 */
import type { MockNodeData } from '../utils/test-utils';

/**
 * Helper to create unified Node objects for integration tests
 *
 * Creates a complete node object with all required fields for testing
 * with ReactiveNodeService and EventBus integration tests.
 *
 * @param id - Unique node identifier
 * @param content - Node content text
 * @param nodeType - Type of node (default: 'text')
 * @param parentId - Parent node ID (default: null for root nodes)
 * @param properties - Additional node properties
 * @param originNodeId - Origin node ID (defaults to parentId if not specified)
 * @returns Complete node object ready for initializeNodes()
 */
export function createNode(
  id: string,
  content: string,
  nodeType: string = 'text',
  parentId: string | null = null,
  properties: Record<string, unknown> = {},
  originNodeId?: string | null
) {
  const now = new Date().toISOString();
  return {
    id,
    nodeType,
    content,
    parentId,
    originNodeId: originNodeId ?? parentId,
    beforeSiblingId: null,
    children: [],
    createdAt: now,
    modifiedAt: now,
    properties
  };
}

export const sampleTextNode: MockNodeData = {
  id: 'sample-text-1',
  type: 'text',
  content: 'This is a sample text node for testing.',
  createdAt: new Date('2024-01-01T10:00:00Z'),
  updatedAt: new Date('2024-01-01T10:30:00Z')
};

export const sampleTaskNode: MockNodeData = {
  id: 'sample-task-1',
  type: 'task',
  content: 'Complete the testing setup',
  createdAt: new Date('2024-01-01T11:00:00Z'),
  updatedAt: new Date('2024-01-01T11:15:00Z')
};

export const sampleAIChatNode: MockNodeData = {
  id: 'sample-chat-1',
  type: 'ai-chat',
  content: 'Help me understand this concept',
  createdAt: new Date('2024-01-01T12:00:00Z'),
  updatedAt: new Date('2024-01-01T12:05:00Z')
};

export const sampleNodes: MockNodeData[] = [sampleTextNode, sampleTaskNode, sampleAIChatNode];

/**
 * Node result type for autocomplete testing
 */
export interface NodeResult {
  id: string;
  title: string;
  type: string;
  subtitle: string;
  metadata: string;
}

/**
 * Mock nodes for autocomplete testing
 * Used across component and integration tests for consistency
 */
export const MOCK_AUTOCOMPLETE_NODES: NodeResult[] = [
  {
    id: 'node-1',
    title: 'Test Node',
    type: 'text',
    subtitle: 'Sample',
    metadata: '1 day ago'
  },
  {
    id: 'node-2',
    title: 'Testing Guide',
    type: 'document',
    subtitle: 'Docs',
    metadata: '2 days ago'
  },
  {
    id: 'node-3',
    title: 'Task Example',
    type: 'task',
    subtitle: 'Todo',
    metadata: 'Today'
  },
  {
    id: 'node-4',
    title: 'Another Test',
    type: 'text',
    subtitle: 'Sample',
    metadata: '3 days ago'
  },
  {
    id: 'node-5',
    title: 'Project Documentation',
    type: 'document',
    subtitle: 'Docs',
    metadata: '1 week ago'
  }
];
