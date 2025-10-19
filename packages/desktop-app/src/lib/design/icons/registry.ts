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

import type { Component, ComponentType } from 'svelte';
import CircleIcon from './components/circle-icon.svelte';
import TaskIcon from './components/task-icon.svelte';
import AIIcon from './components/ai-icon.svelte';
import CodeBlockIcon from './components/code-block-icon.svelte';
import QuoteBlockIcon from './components/quote-block-icon.svelte';
import OrderedListIcon from './components/ordered-list-icon.svelte';

// Dynamic node types - can be extended by plugins
export type NodeType = string;

export type NodeState = 'pending' | 'inProgress' | 'completed' | 'default';

// Svelte 5 Component type (modern) or Svelte 4 ComponentType (legacy)
// This allows the registry to support both Svelte 4 and Svelte 5 components
export type IconComponentType = ComponentType | Component<Record<string, unknown>>;

export interface IconConfig {
  /** Svelte component reference for the icon (supports both Svelte 4 and 5) */
  component: IconComponentType;
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
 * Dynamic Icon Registry: Maps node types to their configuration
 *
 * This eliminates the complex conditional logic in the old Icon component
 * and provides a clean, scalable way to manage icon semantics.
 * External plugins can register their own icon configurations.
 */
class IconRegistry {
  private registry = new Map<string, IconConfig>();

  constructor() {
    // Register core icon configs
    this.registerCoreConfigs();
  }

  private registerCoreConfigs(): void {
    // Text nodes - default node type with ring effect for children
    this.register('text', {
      component: CircleIcon,
      semanticClass: 'node-icon',
      colorVar: 'currentColor',
      hasState: false,
      hasRingEffect: true
    });

    // Header nodes - similar to text with ring effect for children
    this.register('header', {
      component: CircleIcon,
      semanticClass: 'node-icon',
      colorVar: 'currentColor',
      hasState: false,
      hasRingEffect: true
    });

    // Document nodes - similar to text but potentially different semantics
    this.register('document', {
      component: CircleIcon,
      semanticClass: 'node-icon',
      colorVar: 'currentColor',
      hasState: false,
      hasRingEffect: true
    });

    // Task nodes - with state support (pending/inProgress/completed)
    this.register('task', {
      component: TaskIcon,
      semanticClass: 'task-icon',
      colorVar: 'hsl(var(--node-task, 200 40% 45%))',
      hasState: true,
      hasRingEffect: true // Tasks show ring effects when they have children
    });

    // AI Chat nodes - square design with "AI" text
    this.register('ai-chat', {
      component: AIIcon,
      semanticClass: 'ai-icon',
      colorVar: 'hsl(var(--node-ai-chat, 200 40% 45%))',
      hasState: false,
      hasRingEffect: false
    });

    // AI Chat nodes (alternative naming convention)
    this.register('ai_chat', {
      component: AIIcon,
      semanticClass: 'ai-icon',
      colorVar: 'hsl(var(--node-ai-chat, 200 40% 45%))',
      hasState: false,
      hasRingEffect: false
    });

    // User nodes - for user-related content
    this.register('user', {
      component: CircleIcon,
      semanticClass: 'node-icon',
      colorVar: 'hsl(var(--node-text, 200 40% 45%))', // Blue-gray (Scheme 3)
      hasState: false,
      hasRingEffect: true
    });

    // Entity nodes - for entities and relationships
    this.register('entity', {
      component: CircleIcon,
      semanticClass: 'node-icon',
      colorVar: 'hsl(var(--node-entity, 200 40% 45%))',
      hasState: false,
      hasRingEffect: true
    });

    // Query nodes - for search queries and filters
    this.register('query', {
      component: CircleIcon,
      semanticClass: 'node-icon',
      colorVar: 'hsl(var(--node-query, 200 40% 45%))',
      hasState: false,
      hasRingEffect: true
    });

    // Code block nodes - for code snippets with language selection
    this.register('code-block', {
      component: CodeBlockIcon,
      semanticClass: 'node-icon',
      colorVar: 'hsl(var(--node-text, 200 40% 45%))',
      hasState: false,
      hasRingEffect: false // Code blocks are leaf nodes (no children)
    });

    // Quote block nodes - for block quotes with markdown styling
    this.register('quote-block', {
      component: QuoteBlockIcon,
      semanticClass: 'node-icon',
      colorVar: 'hsl(var(--node-text, 200 40% 45%))',
      hasState: false,
      hasRingEffect: true // Quote blocks can have children
    });

    // Ordered list nodes - for auto-numbered ordered list items
    this.register('ordered-list', {
      component: OrderedListIcon,
      semanticClass: 'node-icon',
      colorVar: 'hsl(var(--node-text, 200 40% 45%))',
      hasState: false,
      hasRingEffect: false // Ordered lists are leaf nodes (no children)
    });
  }

  /**
   * Register an icon configuration for a node type
   * External plugins can use this to register their own icons
   */
  public register(nodeType: string, config: IconConfig): void {
    this.registry.set(nodeType, config);
  }

  /**
   * Get icon configuration for a specific node type
   */
  public getConfig(nodeType: string): IconConfig {
    const config = this.registry.get(nodeType);
    if (!config) {
      // Fallback to text node for unrecognized types
      return this.registry.get('text')!;
    }
    return config;
  }

  /**
   * Check if a node type has a registered icon config
   */
  public hasConfig(nodeType: string): boolean {
    return this.registry.has(nodeType);
  }

  /**
   * Get all registered node types
   */
  public getAllNodeTypes(): string[] {
    return Array.from(this.registry.keys());
  }
}

// Global icon registry instance
export const iconRegistry = new IconRegistry();

/**
 * Get icon configuration for a specific node type
 *
 * @param nodeType - The type of node
 * @returns The icon configuration for the node type
 */
export function getIconConfig(nodeType: NodeType): IconConfig {
  return iconRegistry.getConfig(nodeType);
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

  // For task nodes, read state from metadata
  if (nodeType === 'task' && additionalProps) {
    const taskState = additionalProps.taskState as string;
    if (taskState && ['pending', 'inProgress', 'completed'].includes(taskState)) {
      return taskState as NodeState;
    }
    return 'pending'; // Default to pending if no valid state
  }

  // For other node types, use default state
  return 'default';
}
