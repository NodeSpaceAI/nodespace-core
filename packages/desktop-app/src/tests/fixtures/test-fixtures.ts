/**
 * Unified Test Fixtures for NodeSpace
 *
 * Central location for all mock data used across tests.
 * Eliminates duplication of mock nodes, autocomplete results,
 * slash commands, and other test data.
 *
 * Philosophy:
 * - Single source of truth for test data
 * - Consistent structure across all tests
 * - Easy to maintain and update
 *
 * Usage:
 * ```typescript
 * import { MOCK_TEXT_NODE, MOCK_AUTOCOMPLETE_RESULTS } from '@tests/fixtures';
 * ```
 */

import type { Node } from '$lib/types/node';
import type { NodeType } from '$lib/design/icons';
import { createTestNode } from '../helpers/test-helpers';

// ============================================================================
// Standard Mock Nodes
// ============================================================================

/**
 * Standard text node for testing
 */
export const MOCK_TEXT_NODE: Node = createTestNode({
  id: 'mock-text-1',
  nodeType: 'text',
  content: 'This is a sample text node for testing.',
  createdAt: '2024-01-01T10:00:00Z',
  modifiedAt: '2024-01-01T10:30:00Z'
});

/**
 * Standard task node for testing
 */
export const MOCK_TASK_NODE: Node = createTestNode({
  id: 'mock-task-1',
  nodeType: 'task',
  content: 'Complete the testing setup',
  createdAt: '2024-01-01T11:00:00Z',
  modifiedAt: '2024-01-01T11:15:00Z',
  properties: { completed: false }
});

/**
 * Standard document node for testing
 */
export const MOCK_DOCUMENT_NODE: Node = createTestNode({
  id: 'mock-doc-1',
  nodeType: 'document',
  content: 'Project documentation',
  createdAt: '2024-01-01T12:00:00Z',
  modifiedAt: '2024-01-01T12:05:00Z'
});

/**
 * Standard AI chat node for testing
 */
export const MOCK_AI_CHAT_NODE: Node = createTestNode({
  id: 'mock-chat-1',
  nodeType: 'ai-chat',
  content: 'Help me understand this concept',
  createdAt: '2024-01-01T13:00:00Z',
  modifiedAt: '2024-01-01T13:10:00Z'
});

/**
 * Standard date node for testing
 */
export const MOCK_DATE_NODE: Node = createTestNode({
  id: '2024-10-06',
  nodeType: 'date',
  content: 'Daily notes for October 6, 2024',
  parentId: null,
  containerNodeId: null,
  createdAt: '2024-10-06T00:00:00Z',
  modifiedAt: '2024-10-06T00:00:00Z'
});

/**
 * Collection of standard nodes for testing
 */
export const MOCK_NODES = [
  MOCK_TEXT_NODE,
  MOCK_TASK_NODE,
  MOCK_DOCUMENT_NODE,
  MOCK_AI_CHAT_NODE,
  MOCK_DATE_NODE
] as const;

// ============================================================================
// Mock Node Hierarchies
// ============================================================================

/**
 * Simple parent-child hierarchy
 */
export const MOCK_PARENT_NODE = createTestNode({
  id: 'parent-1',
  content: 'Parent node',
  parentId: null,
  containerNodeId: null
});

export const MOCK_CHILD_NODE_1 = createTestNode({
  id: 'child-1',
  content: 'First child',
  parentId: 'parent-1',
  containerNodeId: 'parent-1'
});

export const MOCK_CHILD_NODE_2 = createTestNode({
  id: 'child-2',
  content: 'Second child',
  parentId: 'parent-1',
  containerNodeId: 'parent-1'
});

export const MOCK_GRANDCHILD_NODE = createTestNode({
  id: 'grandchild-1',
  content: 'Grandchild node',
  parentId: 'child-1',
  containerNodeId: 'parent-1'
});

/**
 * Complete node hierarchy for testing
 */
export const MOCK_NODE_HIERARCHY = [
  MOCK_PARENT_NODE,
  MOCK_CHILD_NODE_1,
  MOCK_CHILD_NODE_2,
  MOCK_GRANDCHILD_NODE
] as const;

// ============================================================================
// Mock Autocomplete Results
// ============================================================================

/**
 * Node result type for autocomplete testing
 */
export interface NodeResult {
  id: string;
  title: string;
  type: NodeType;
  subtitle?: string;
  metadata?: string;
}

/**
 * Standard autocomplete results for testing
 */
export const MOCK_AUTOCOMPLETE_RESULTS: NodeResult[] = [
  {
    id: 'node-1',
    title: 'First Node',
    type: 'text',
    subtitle: 'Sample',
    metadata: '1 day ago'
  },
  {
    id: 'node-2',
    title: 'Second Node',
    type: 'document',
    subtitle: 'Docs',
    metadata: '2 days ago'
  },
  {
    id: 'node-3',
    title: 'Third Node',
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

/**
 * Filtered autocomplete results (subset for testing filtering)
 */
export const MOCK_FILTERED_RESULTS: NodeResult[] = [
  MOCK_AUTOCOMPLETE_RESULTS[0],
  MOCK_AUTOCOMPLETE_RESULTS[3]
];

/**
 * Empty autocomplete results for testing empty states
 */
export const MOCK_EMPTY_RESULTS: NodeResult[] = [];

// ============================================================================
// Mock Slash Commands
// ============================================================================

/**
 * Slash command type for testing
 */
export interface SlashCommand {
  id: string;
  name: string;
  description: string;
  nodeType: string;
  icon: string;
  shortcut: string;
  headerLevel?: number;
}

/**
 * Standard slash commands for testing
 */
export const MOCK_SLASH_COMMANDS: SlashCommand[] = [
  {
    id: 'header1',
    name: 'Heading 1',
    description: 'Large heading',
    nodeType: 'text',
    icon: 'text',
    shortcut: '/h1',
    headerLevel: 1
  },
  {
    id: 'header2',
    name: 'Heading 2',
    description: 'Medium heading',
    nodeType: 'text',
    icon: 'text',
    shortcut: '/h2',
    headerLevel: 2
  },
  {
    id: 'header3',
    name: 'Heading 3',
    description: 'Small heading',
    nodeType: 'text',
    icon: 'text',
    shortcut: '/h3',
    headerLevel: 3
  },
  {
    id: 'task',
    name: 'Task',
    description: 'Create a task',
    nodeType: 'task',
    icon: 'task',
    shortcut: '/task'
  },
  {
    id: 'doc',
    name: 'Document',
    description: 'Create a document',
    nodeType: 'document',
    icon: 'document',
    shortcut: '/doc'
  }
];

/**
 * Header-specific slash commands for testing
 */
export const MOCK_HEADER_COMMANDS: SlashCommand[] = [
  MOCK_SLASH_COMMANDS[0], // h1
  MOCK_SLASH_COMMANDS[1], // h2
  MOCK_SLASH_COMMANDS[2] // h3
];

/**
 * Non-header slash commands for testing
 */
export const MOCK_NON_HEADER_COMMANDS: SlashCommand[] = [
  MOCK_SLASH_COMMANDS[3], // task
  MOCK_SLASH_COMMANDS[4] // doc
];

// ============================================================================
// Mock Event Data
// ============================================================================

/**
 * Standard position for dropdown/overlay testing
 */
export const MOCK_DEFAULT_POSITION = { x: 100, y: 100 };

/**
 * Mock cursor positions for testing
 */
export const MOCK_CURSOR_POSITIONS = {
  topLeft: { x: 0, y: 0 },
  center: { x: 400, y: 300 },
  bottomRight: { x: 800, y: 600 }
} as const;

// ============================================================================
// Mock Properties
// ============================================================================

/**
 * Standard task properties
 */
export const MOCK_TASK_PROPERTIES = {
  completed: false,
  dueDate: '2024-12-31',
  priority: 'high'
} as const;

/**
 * Standard document properties
 */
export const MOCK_DOCUMENT_PROPERTIES = {
  template: 'standard',
  version: 1,
  lastReviewed: '2024-10-01'
} as const;

// ============================================================================
// Mock Timestamps
// ============================================================================

/**
 * Standard timestamps for testing
 */
export const MOCK_TIMESTAMPS = {
  now: '2024-10-06T12:00:00Z',
  yesterday: '2024-10-05T12:00:00Z',
  lastWeek: '2024-09-29T12:00:00Z',
  lastMonth: '2024-09-06T12:00:00Z'
} as const;

// ============================================================================
// Helper Functions for Fixtures
// ============================================================================

/**
 * Create a copy of a fixture with overrides
 *
 * Useful for creating variations without modifying the original.
 *
 * @param fixture - Original fixture
 * @param overrides - Properties to override
 * @returns New fixture with overrides applied
 *
 * @example
 * ```typescript
 * const customNode = withOverrides(MOCK_TEXT_NODE, {
 *   content: 'Custom content'
 * });
 * ```
 */
export function withOverrides<T extends Node>(fixture: T, overrides: Partial<Node>): Node {
  return { ...fixture, ...overrides };
}

/**
 * Create multiple nodes from a template
 *
 * @param template - Node template
 * @param count - Number of nodes to create
 * @param transform - Optional transform function for each node
 * @returns Array of nodes
 *
 * @example
 * ```typescript
 * const nodes = createNodesFromTemplate(MOCK_TEXT_NODE, 5, (node, i) => ({
 *   ...node,
 *   id: `node-${i}`,
 *   content: `Node ${i}`
 * }));
 * ```
 */
export function createNodesFromTemplate(
  template: Node,
  count: number,
  transform?: (node: Node, index: number) => Partial<Node>
): Node[] {
  const nodes: Node[] = [];
  for (let i = 0; i < count; i++) {
    const overrides = transform?.(template, i) || {};
    nodes.push({ ...template, ...overrides });
  }
  return nodes;
}
