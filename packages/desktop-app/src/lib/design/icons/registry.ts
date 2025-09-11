/**
 * Component-Based Icon Registry
 * 
 * This registry maps node types to their icon configurations using a component-based approach
 * that eliminates the architectural inconsistency between CSS-based design system and SVG-based components.
 * 
 * Key Features:
 * - Node semantic thinking instead of individual icon management
 * - Component references for type safety and performance
 * - Semantic class mapping from design system
 * - Scalable architecture for future node types
 */

import type { ComponentType } from 'svelte';
import CircleIcon from './components/CircleIcon.svelte';
import TaskIcon from './components/TaskIcon.svelte';
import AIIcon from './components/AIIcon.svelte';

export type NodeType = 
  | 'text' 
  | 'document'
  | 'task' 
  | 'ai-chat' 
  | 'ai_chat'
  | 'user'
  | 'entity'
  | 'query';

export type NodeState = 
  | 'pending'
  | 'inProgress' 
  | 'completed'
  | 'default';

export interface IconConfig {
  /** Svelte component reference for the icon */
  component: ComponentType;
  /** Semantic CSS class from design system */
  semanticClass: 'node-icon' | 'task-icon' | 'ai-icon';
  /** Default color theme from design system */
  colorVar: string;
  /** Whether this node type supports state variations */
  hasState: boolean;
  /** Whether this node type supports parent/child ring effects */
  hasRingEffect: boolean;
}

export interface NodeIconProps {
  nodeType: NodeType;
  state?: NodeState;
  hasChildren?: boolean;
  size?: number;
  className?: string;
  color?: string;
}

/**
 * Icon Registry: Maps node types to their configuration
 * 
 * This eliminates the complex conditional logic in the old Icon component
 * and provides a clean, scalable way to manage icon semantics.
 */
export const iconRegistry: Record<NodeType, IconConfig> = {
  // Text nodes - default node type with ring effect for children
  text: {
    component: CircleIcon,
    semanticClass: 'node-icon',
    colorVar: 'currentColor',
    hasState: false,
    hasRingEffect: true
  },

  // Document nodes - similar to text but potentially different semantics
  document: {
    component: CircleIcon,
    semanticClass: 'node-icon', 
    colorVar: 'currentColor',
    hasState: false,
    hasRingEffect: true
  },

  // Task nodes - with state support (pending/inProgress/completed)
  task: {
    component: TaskIcon,
    semanticClass: 'task-icon',
    colorVar: 'hsl(var(--node-task, 25 95% 53%))',
    hasState: true,
    hasRingEffect: false // Tasks use state-specific icons instead of ring effects
  },

  // AI Chat nodes - square design with "AI" text
  'ai-chat': {
    component: AIIcon,
    semanticClass: 'ai-icon',
    colorVar: 'hsl(var(--node-ai-chat, 221 83% 53%))',
    hasState: false,
    hasRingEffect: false
  },

  // AI Chat nodes (alternative naming convention)
  ai_chat: {
    component: AIIcon,
    semanticClass: 'ai-icon',
    colorVar: 'hsl(var(--node-ai-chat, 221 83% 53%))',
    hasState: false,
    hasRingEffect: false
  },

  // User nodes - for user-related content
  user: {
    component: CircleIcon,
    semanticClass: 'node-icon',
    colorVar: 'hsl(var(--node-text, 142 71% 45%))', // Use text color for now, can be customized
    hasState: false,
    hasRingEffect: true
  },

  // Entity nodes - for entities and relationships
  entity: {
    component: CircleIcon,
    semanticClass: 'node-icon',
    colorVar: 'hsl(var(--node-entity, 271 81% 56%))',
    hasState: false,
    hasRingEffect: true
  },

  // Query nodes - for search queries and filters
  query: {
    component: CircleIcon,
    semanticClass: 'node-icon',
    colorVar: 'hsl(var(--node-query, 330 81% 60%))',
    hasState: false,
    hasRingEffect: true
  }
};

/**
 * Get icon configuration for a specific node type
 * 
 * @param nodeType - The type of node
 * @returns The icon configuration for the node type
 */
export function getIconConfig(nodeType: NodeType): IconConfig {
  const config = iconRegistry[nodeType];
  if (!config) {
    // Fallback to text node for unrecognized types
    return iconRegistry.text;
  }
  return config;
}

/**
 * Determine the appropriate state for a task node
 * This can be expanded to include more sophisticated state logic
 * 
 * @param nodeType - The type of node
 * @param explicitState - Explicitly provided state
 * @param additionalProps - Additional properties that might influence state
 * @returns The resolved node state
 */
export function resolveNodeState(
  nodeType: NodeType, 
  explicitState?: NodeState,
  additionalProps?: Record<string, unknown>
): NodeState {
  // If explicit state is provided, use it
  if (explicitState) {
    return explicitState;
  }
  
  // For task nodes, default to pending if no state specified
  if (nodeType === 'task') {
    return 'pending';
  }
  
  // For other node types, use default state
  return 'default';
}