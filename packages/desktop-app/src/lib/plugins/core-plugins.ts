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
import BaseNodeReference from '../components/base-node-reference.svelte';

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
  viewer: {
    lazyLoad: () => import('../components/viewers/text-node-viewer.svelte'),
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
  viewer: {
    lazyLoad: () => import('../components/viewers/task-node-viewer.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  }
};

export const aiChatNodePlugin: PluginDefinition = {
  id: 'ai-chat',
  name: 'AI Chat Node',
  description: 'Start an AI conversation',
  version: '1.0.0',
  config: {
    slashCommands: [
      {
        id: 'ai-chat',
        name: 'AI Chat',
        description: 'Start an AI conversation',
        shortcut: '⌘ + k',
        contentTemplate: ''
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  },
  viewer: {
    lazyLoad: () => import('../components/viewers/ai-chat-node-viewer.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
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

// Export all core plugins
// These are the foundation plugins - external developers can create additional plugins
// like WhiteBoardNode, CodeNode, ImageNode, etc. in separate packages
export const corePlugins = [
  textNodePlugin,
  headerNodePlugin,
  taskNodePlugin,
  aiChatNodePlugin,
  dateNodePlugin,
  userNodePlugin,
  documentNodePlugin
];

/**
 * Register all core plugins with the unified registry
 * Replaces the old BasicNodeTypeRegistry initialization
 */
export function registerCorePlugins(registry: import('./plugin-registry').PluginRegistry): void {
  // Check if plugins are already registered in this specific registry instance
  if (registry.hasPlugin('text')) {
    return; // Already registered in this registry
  }

  for (const plugin of corePlugins) {
    registry.register(plugin);
  }

  // Log registration statistics
  const stats = registry.getStats();
  console.log('[UnifiedPluginRegistry] Core plugins registered:', {
    plugins: stats.pluginsCount,
    slashCommands: stats.slashCommandsCount,
    viewers: stats.viewersCount,
    references: stats.referencesCount
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
  console.log(`[UnifiedPluginRegistry] External plugin registered: ${plugin.name} (${plugin.id})`);
}
