/**
 * BaseNode Decoration System - Core Architecture (Phase 2.2)
 *
 * Implements the core decorateReference() pattern for creating component-based decorations
 * for node references in the universal node reference system.
 *
 * Key Features:
 * - Base decoration system with component-based rendering
 * - Node type configurations for styling and metadata
 * - Integration with NodeReferenceService and component system
 * - Extensible architecture for future node type implementations
 */

import type { NodeReferenceService } from './NodeReferenceService';
import type { ComponentDecoration } from '../types/ComponentDecoration';
import { getNodeReferenceComponent } from '../components/references';

// ============================================================================
// Core Types and Interfaces
// ============================================================================

export interface DecorationContext {
  nodeId: string;
  nodeType: string;
  title: string;
  content: string;
  uri: string;
  metadata: Record<string, unknown>;
  targetElement: HTMLElement;
  displayContext: 'inline' | 'popup' | 'preview';
}

export interface NodeTypeConfig {
  icon: string;
  label: string;
  color: string;
  defaultDecoration: (context: DecorationContext) => ComponentDecoration;
}

// ============================================================================
// Base Node Decorator
// ============================================================================

export abstract class BaseNodeDecorator {
  protected nodeReferenceService: NodeReferenceService;
  protected nodeType: string;

  constructor(nodeReferenceService: NodeReferenceService, nodeType: string) {
    this.nodeReferenceService = nodeReferenceService;
    this.nodeType = nodeType;
  }

  /**
   * Creates a base component decoration for any node type
   */
  protected getBaseComponentDecoration(context: DecorationContext): ComponentDecoration {
    const config = NODE_TYPE_CONFIGS[context.nodeType] || NODE_TYPE_CONFIGS.default;

    // Get the appropriate component for this node type
    const Component = getNodeReferenceComponent(context.nodeType);

    return {
      component: Component,
      props: {
        nodeId: context.nodeId,
        nodeType: context.nodeType,
        title: context.title,
        content: context.content,
        uri: context.uri,
        icon: config.icon,
        color: config.color,
        ariaLabel: `${config.label}: ${context.title}`,
        metadata: context.metadata,
        displayContext: context.displayContext
      }
    };
  }

  /**
   * Abstract method for creating component decorations
   * Implementations should return ComponentDecoration objects
   */
  public abstract decorateReference(context: DecorationContext): ComponentDecoration;
}

// ============================================================================
// Default Decorator Implementation
// ============================================================================

/**
 * Default decorator that provides basic component-based decoration for all node types
 */
class DefaultNodeDecorator extends BaseNodeDecorator {
  constructor(nodeReferenceService: NodeReferenceService) {
    super(nodeReferenceService, 'default');
  }

  public decorateReference(context: DecorationContext): ComponentDecoration {
    return this.getBaseComponentDecoration(context);
  }
}

// ============================================================================
// Node Type Configurations
// ============================================================================

export const NODE_TYPE_CONFIGS: Record<string, NodeTypeConfig> = {
  default: {
    icon: '📝',
    label: 'Node',
    color: 'var(--node-default)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  text: {
    icon: '📝',
    label: 'Text',
    color: 'var(--node-text)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  task: {
    icon: '☐',
    label: 'Task',
    color: 'var(--node-task)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  user: {
    icon: '👤',
    label: 'User',
    color: 'var(--node-user)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  date: {
    icon: '📅',
    label: 'Date',
    color: 'var(--node-date)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  document: {
    icon: '📄',
    label: 'Document',
    color: 'var(--node-document)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  },
  ai_chat: {
    icon: '🤖',
    label: 'AI Chat',
    color: 'var(--node-ai-chat)',
    defaultDecoration: (context) => {
      const decorator = new DefaultNodeDecorator(null!);
      return decorator.decorateReference(context);
    }
  }
};

// ============================================================================
// Node Decorator Factory
// ============================================================================

export class NodeDecoratorFactory {
  private nodeReferenceService: NodeReferenceService;
  private decorators: Map<string, BaseNodeDecorator> = new Map();

  constructor(nodeReferenceService: NodeReferenceService) {
    this.nodeReferenceService = nodeReferenceService;

    // Initialize default decorator for all node types
    const defaultDecorator = new DefaultNodeDecorator(this.nodeReferenceService);
    this.decorators.set('default', defaultDecorator);
    this.decorators.set('text', defaultDecorator);
    this.decorators.set('task', defaultDecorator);
    this.decorators.set('user', defaultDecorator);
    this.decorators.set('date', defaultDecorator);
    this.decorators.set('document', defaultDecorator);
    this.decorators.set('ai_chat', defaultDecorator);
  }

  /**
   * Creates a component decoration for the specified node type
   */
  public decorateReference(context: DecorationContext): ComponentDecoration {
    const decorator = this.decorators.get(context.nodeType) || this.decorators.get('default')!;
    return decorator.decorateReference(context);
  }

  /**
   * Gets the configuration for a node type
   */
  public getNodeTypeConfig(nodeType: string): NodeTypeConfig {
    return NODE_TYPE_CONFIGS[nodeType] || NODE_TYPE_CONFIGS.default;
  }
}

// ============================================================================
// Exports
// ============================================================================

export { DefaultNodeDecorator };
