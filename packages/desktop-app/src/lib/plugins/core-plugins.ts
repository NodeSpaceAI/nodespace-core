/**
 * Core Plugin Definitions
 *
 * Unified plugin system that consolidates:
 * - ViewerRegistry (viewer components)
 * - NODE_REFERENCE_COMPONENTS (reference components)
 * - BasicNodeTypeRegistry (node definitions + slash commands)
 *
 * Incorporates the excellent slash command work from the recent BasicNodeTypeRegistry.
 * Designed for future external plugin development (e.g., WhiteBoardNode).
 */

import type { PluginDefinition, NodeReferenceComponent } from './types';
import type { PatternTemplate } from '../patterns/types';
import type { CoreTaskStatus, TaskNodeUpdate } from '../types/task-node';
import { PatternRegistry } from '../patterns/registry';
import { backendAdapter } from '../services/backend-adapter';
import BaseNodeReference from '../components/base-node-reference.svelte';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('CorePlugins');

// Core plugins for built-in node types
// Incorporates slash command definitions from recent BasicNodeTypeRegistry work
export const textNodePlugin: PluginDefinition = {
  id: 'text',
  name: 'Text Node',
  description: 'Create a plain text node',
  version: '1.0.0',
  config: {
    slashCommands: [
      {
        id: 'text',
        name: 'Text',
        description: 'Create a text node',
        contentTemplate: '',
        nodeType: 'text' // Explicit nodeType for proper visual updates
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  },
  // No viewer - text nodes use BaseNodeViewer (default)
  node: {
    lazyLoad: () => import('../components/text-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  }
};

export const headerNodePlugin: PluginDefinition = {
  id: 'header',
  name: 'Header Node',
  description: 'Create a header with customizable level (1-6)',
  version: '1.0.0',
  // Plugin-owned pattern behavior (Issue #667)
  pattern: {
    detect: /^(#{1,6})\s/,
    canRevert: true,
    revert: /^#{1,6}$/,  // "# " → "#" should revert to text
    onEnter: 'inherit',
    prefixToInherit: (content: string) => content.match(/^(#{1,6})\s/)?.[0],
    splittingStrategy: 'prefix-inheritance',
    cursorPlacement: 'after-prefix',
    extractMetadata: (match: RegExpMatchArray) => ({
      headerLevel: match[1].length
    })
  },
  config: {
    slashCommands: [
      {
        id: 'header1',
        name: 'Header 1',
        description: 'Create a large header',
        shortcut: '#',
        contentTemplate: '# ',
        nodeType: 'header'
      },
      {
        id: 'header2',
        name: 'Header 2',
        description: 'Create a medium header',
        shortcut: '##',
        contentTemplate: '## ',
        nodeType: 'header'
      },
      {
        id: 'header3',
        name: 'Header 3',
        description: 'Create a small header',
        shortcut: '###',
        contentTemplate: '### ',
        nodeType: 'header'
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/header-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  }
};

export const taskNodePlugin: PluginDefinition = {
  id: 'task',
  name: 'Task Node',
  description: 'Create a task with checkbox and state management',
  version: '1.0.0',
  // Plugin-owned pattern behavior (Issue #667)
  pattern: {
    detect: /^[-*+]?\s*\[\s*[xX\s]\s*\]\s/,
    canRevert: false,  // cleanContent: true - checkbox syntax removed, cannot revert
    onEnter: 'inherit',
    prefixToInherit: '',  // No prefix inheritance for tasks
    splittingStrategy: 'simple-split',
    cursorPlacement: 'start',
    extractMetadata: (match: RegExpMatchArray) => {
      const isCompleted = /[xX]/.test(match[0]);
      return {
        taskState: isCompleted ? 'completed' : 'pending'
      };
    }
  },
  config: {
    slashCommands: [
      {
        id: 'task',
        name: 'Task',
        description: 'Create a task with checkbox',
        shortcut: '[ ]',
        contentTemplate: '', // Empty content - task icon shows the state instead
        nodeType: 'task' // Set node type to 'task' when selected
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/task-node.svelte'),
    priority: 1
  },
  // TaskNodeViewer for task-specific UI (Issue #715)
  viewer: {
    lazyLoad: () => import('../components/viewers/task-node-viewer.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  // Type-specific metadata extraction (Issue #698, #794)
  // Issue #794: Properties are now namespaced under properties[node_type]
  extractMetadata: (node: { nodeType: string; properties?: Record<string, unknown> }) => {
    const properties = node.properties || {};
    const taskProps = properties[node.nodeType] as Record<string, unknown> | undefined;
    const status = taskProps?.status;

    // Map task status to NodeState expected by TaskNode
    let taskState: 'pending' | 'inProgress' | 'completed' = 'pending';
    if (status === 'IN_PROGRESS' || status === 'in_progress') {
      taskState = 'inProgress';
    } else if (status === 'DONE' || status === 'done') {
      taskState = 'completed';
    } else if (status === 'OPEN' || status === 'open') {
      taskState = 'pending';
    }

    return { taskState, ...properties };
  },
  // Type-specific state mapping (Issue #698)
  mapStateToSchema: (state: string, _fieldName: string): CoreTaskStatus => {
    switch (state) {
      case 'pending':
        return 'open';
      case 'inProgress':
        return 'in_progress';
      case 'completed':
        return 'done';
      default:
        return 'open';
    }
  },

  // Type-specific updater for spoke table fields (Issue #709)
  // Routes to updateTaskNode() instead of generic updateNode()
  updater: {
    update: async (id: string, version: number, changes: Record<string, unknown>) => {
      // Convert changes to TaskNodeUpdate format
      // The caller provides type-safe changes, we map to the backend format
      const update: TaskNodeUpdate = {};
      if ('status' in changes && changes.status !== undefined) update.status = changes.status as TaskNodeUpdate['status'];
      if ('priority' in changes) update.priority = changes.priority as TaskNodeUpdate['priority'];
      if ('dueDate' in changes) update.dueDate = changes.dueDate as TaskNodeUpdate['dueDate'];
      if ('assignee' in changes) update.assignee = changes.assignee as TaskNodeUpdate['assignee'];
      if ('startedAt' in changes) update.startedAt = changes.startedAt as TaskNodeUpdate['startedAt'];
      if ('completedAt' in changes) update.completedAt = changes.completedAt as TaskNodeUpdate['completedAt'];
      if ('content' in changes && changes.content !== undefined) update.content = changes.content as string;

      // Returns TaskNode which has hub fields but not properties (flat structure)
      // Cast to Node for interface compatibility - sharedNodeStore will handle appropriately
      const result = await backendAdapter.updateTaskNode(id, version, update);
      return result as unknown as import('../types').Node;
    }
  },

  // Type-specific schema form for spoke fields (Issue #709)
  schemaForm: {
    lazyLoad: () => import('../components/property-forms/task-schema-form.svelte')
  }
};

// Date node - exists implicitly for all dates, cannot be created via slash commands
export const dateNodePlugin: PluginDefinition = {
  id: 'date',
  name: 'Date Node',
  description: 'Date and time node (not creatable - exists for all dates)',
  version: '1.0.0',
  config: {
    slashCommands: [], // No slash commands - date nodes exist implicitly
    canHaveChildren: true,
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/date-node.svelte'),
    priority: 1
  },
  viewer: {
    lazyLoad: () => import('../components/viewers/date-node-viewer.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  }
};

export const codeBlockNodePlugin: PluginDefinition = {
  id: 'code-block',
  name: 'Code Block Node',
  description: 'Code snippet with language selection and syntax',
  version: '1.0.0',
  // Plugin-owned pattern behavior (Issue #667)
  pattern: {
    detect: /^```(\w+)?\n/,  // Matches ``` or ```language followed by newline
    canRevert: true,
    revert: /^```$/,  // "```" alone should revert to text
    onEnter: 'none',  // Code blocks don't inherit on Enter
    splittingStrategy: 'simple-split',
    cursorPlacement: 'start',
    extractMetadata: (match: RegExpMatchArray) => ({
      language: match[1]?.toLowerCase() || 'plaintext'
    })
  },
  config: {
    slashCommands: [
      {
        id: 'code',
        name: 'Code Block',
        description: 'Create a code block with language selection',
        shortcut: '```',
        contentTemplate: '```\n\n```',
        nodeType: 'code-block',
        desiredCursorPosition: 4 // Position cursor after "```\n" (on the empty line)
      }
    ],
    canHaveChildren: false, // Code blocks are leaf nodes
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/code-block-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  // Structured content - cannot accept arbitrary merges (Issue #698)
  acceptsContentMerge: false
};

export const quoteBlockNodePlugin: PluginDefinition = {
  id: 'quote-block',
  name: 'Quote Block Node',
  description: 'Block quote with markdown styling conventions',
  version: '1.0.0',
  // Plugin-owned pattern behavior (Issue #667)
  pattern: {
    detect: /^>\s/,
    canRevert: true,
    revert: /^>$/,  // "> " → ">" should revert to text
    onEnter: 'inherit',
    prefixToInherit: '> ',
    splittingStrategy: 'prefix-inheritance',
    cursorPlacement: 'after-prefix',
    extractMetadata: () => ({})
  },
  config: {
    slashCommands: [
      {
        id: 'quote',
        name: 'Quote Block',
        description: 'Create a block quote with markdown styling',
        shortcut: '>',
        contentTemplate: '> ',
        nodeType: 'quote-block',
        desiredCursorPosition: 2 // Position cursor after "> " prefix
      }
    ],
    canHaveChildren: true, // Quote blocks can have children
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/quote-block-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  // Structured content - cannot accept arbitrary merges (Issue #698)
  acceptsContentMerge: false
};

export const orderedListNodePlugin: PluginDefinition = {
  id: 'ordered-list',
  name: 'Ordered List Node',
  description: 'Auto-numbered ordered list items',
  version: '1.0.0',
  // Plugin-owned pattern behavior (Issue #667)
  pattern: {
    detect: /^1\.\s/,
    canRevert: true,
    revert: /^1\.$/,  // "1. " → "1." should revert to text
    onEnter: 'inherit',
    prefixToInherit: '1. ',
    splittingStrategy: 'prefix-inheritance',
    cursorPlacement: 'after-prefix',
    extractMetadata: () => ({})
  },
  config: {
    slashCommands: [
      {
        id: 'ordered-list',
        name: 'Ordered List',
        description: 'Create an auto-numbered list item',
        shortcut: '1.',
        contentTemplate: '1. ',
        nodeType: 'ordered-list',
        desiredCursorPosition: 3 // Position cursor after "1. " prefix
      }
    ],
    canHaveChildren: false, // Simple flat lists only (no nesting)
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/ordered-list-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  }
};

// Additional node types for reference system (no viewers currently)
export const userNodePlugin: PluginDefinition = {
  id: 'user',
  name: 'User Reference',
  description: 'User reference node',
  version: '1.0.0',
  config: {
    slashCommands: [],
    canHaveChildren: false,
    canBeChild: true
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  }
};

export const queryNodePlugin: PluginDefinition = {
  id: 'query',
  name: 'Query Node',
  description: 'Saved query definition for filtering and searching nodes',
  version: '1.0.0',
  config: {
    slashCommands: [
      {
        id: 'query',
        name: 'Query',
        description: 'Create a saved query for filtering nodes',
        contentTemplate: '', // Empty - users will type query description
        nodeType: 'query'
      }
    ],
    canHaveChildren: false, // Query nodes are leaf nodes
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../components/query-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  }
  // viewer: QueryNodeViewer implemented in Issue #441
};

export const documentNodePlugin: PluginDefinition = {
  id: 'document',
  name: 'Document Reference',
  description: 'Document reference node',
  version: '1.0.0',
  config: {
    slashCommands: [],
    canHaveChildren: true,
    canBeChild: true
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  }
};

/**
 * Collection Node Plugin
 *
 * Collections provide flexible, hierarchical organization for nodes.
 * Unlike parent-child relationships, collections allow:
 * - Many-to-many membership (nodes can belong to multiple collections)
 * - DAG structure (directed acyclic graph)
 * - Path-based navigation (e.g., "hr:policy:vacation")
 *
 * Collections are created via MCP tools, not through slash commands.
 * The plugin provides reference and future viewer components.
 */
export const collectionNodePlugin: PluginDefinition = {
  id: 'collection',
  name: 'Collection Node',
  description: 'Organize nodes into flexible, hierarchical collections',
  version: '1.0.0',
  config: {
    // No slash commands - collections are created via MCP tools
    // This is intentional: collections are organizational metadata, not content
    slashCommands: [],
    canHaveChildren: true, // Collections can have sub-collections (DAG structure)
    canBeChild: true // Collections can be nested under other nodes
  },
  // CollectionNodeViewer for collection-specific UI (Issue #757)
  viewer: {
    lazyLoad: () => import('../components/viewers/collection-node-viewer.svelte'),
    priority: 1
  },
  // Collections use BaseNodeReference for inline references
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  // Type-specific metadata extraction for collection properties
  extractMetadata: (node: { nodeType: string; properties?: Record<string, unknown> }) => {
    const properties = node.properties || {};
    return {
      description: properties.description as string | undefined,
      icon: properties.icon as string | undefined,
      color: properties.color as string | undefined,
      ...properties
    };
  }
};

// Export all core plugins
// These are the foundation plugins - external developers can create additional plugins
// like WhiteBoardNode, ImageNode, etc. in separate packages
export const corePlugins = [
  textNodePlugin,
  headerNodePlugin,
  taskNodePlugin,
  dateNodePlugin,
  codeBlockNodePlugin,
  quoteBlockNodePlugin,
  orderedListNodePlugin,
  queryNodePlugin,
  collectionNodePlugin
  // Note: userNodePlugin and documentNodePlugin are defined but not registered
  // They will be added when user/document reference system is implemented
];

/**
 * Register all core plugins with the unified registry
 * Replaces the old BasicNodeTypeRegistry initialization
 * Also registers patterns with PatternRegistry for unified pattern handling
 */
export function registerCorePlugins(registry: import('./plugin-registry').PluginRegistry): void {
  // Check if plugins are already registered in this specific registry instance
  if (registry.hasPlugin('text')) {
    return; // Already registered in this registry
  }

  const patternRegistry = PatternRegistry.getInstance();

  for (const plugin of corePlugins) {
    registry.register(plugin);

    // Register pattern with PatternRegistry if present (Issue #667)
    // Convert PluginPattern to PatternTemplate for PatternRegistry compatibility
    if (plugin.pattern) {
      const patternTemplate: PatternTemplate = {
        regex: plugin.pattern.detect,
        nodeType: plugin.id,
        priority: 10, // Default priority
        splittingStrategy: plugin.pattern.splittingStrategy,
        // For headers with function-based prefix, PatternRegistry will extract from regex
        prefixToInherit: typeof plugin.pattern.prefixToInherit === 'string'
          ? plugin.pattern.prefixToInherit
          : undefined,
        cursorPlacement: plugin.pattern.cursorPlacement,
        cleanContent: false, // Not used anymore
        extractMetadata: plugin.pattern.extractMetadata
      };
      patternRegistry.register(patternTemplate);
    }
  }

  // Log registration statistics
  const stats = registry.getStats();
  const patternStats = patternRegistry.getStats();
  log.debug('Core plugins registered:', {
    plugins: stats.pluginsCount,
    slashCommands: stats.slashCommandsCount,
    viewers: stats.viewersCount,
    references: stats.referencesCount
  });
  log.debug('Patterns registered:', {
    patterns: patternStats.patternCount,
    registeredNodeTypes: patternStats.registeredNodeTypes
  });
}

/**
 * Future: External plugin registration function
 * External developers will use this to register plugins like WhiteBoardNode
 */
export function registerExternalPlugin(
  registry: import('./plugin-registry').PluginRegistry,
  plugin: PluginDefinition
): void {
  registry.register(plugin);
  log.debug(`External plugin registered: ${plugin.name} (${plugin.id})`);
}
