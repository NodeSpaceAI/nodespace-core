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
  description: 'Create a text node with optional header formatting',
  version: '1.0.0',
  config: {
    slashCommands: [
      {
        id: 'text',
        name: 'Text',
        description: 'Create a text node',
        contentTemplate: ''
      },
      {
        id: 'header1',
        name: 'Header 1',
        description: 'Create a large header',
        shortcut: '#',
        contentTemplate: '# '
      },
      {
        id: 'header2',
        name: 'Header 2',
        description: 'Create a medium header',
        shortcut: '##',
        contentTemplate: '## '
      },
      {
        id: 'header3',
        name: 'Header 3',
        description: 'Create a small header',
        shortcut: '###',
        contentTemplate: '### '
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
        contentTemplate: '- [ ] '
      }
    ],
    canHaveChildren: true,
    canBeChild: true
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

// Additional core node types for reference system
export const dateNodePlugin: PluginDefinition = {
  id: 'date',
  name: 'Date Node',
  description: 'Date and time node',
  version: '1.0.0',
  config: {
    slashCommands: [
      {
        id: 'date',
        name: 'Date',
        description: 'Insert current date',
        contentTemplate: new Date().toISOString().split('T')[0]
      }
    ],
    canHaveChildren: false,
    canBeChild: true
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
export function registerCorePlugins(registry: import('./pluginRegistry').PluginRegistry): void {
  for (const plugin of corePlugins) {
    registry.register(plugin);
  }

  console.log('[UnifiedPluginRegistry] Core plugins registered:', {
    plugins: corePlugins.length,
    slashCommands: registry.getAllSlashCommands().length,
    viewers: corePlugins.filter((p) => p.viewer).length,
    references: corePlugins.filter((p) => p.reference).length
  });
}

/**
 * Future: External plugin registration function
 * External developers will use this to register plugins like WhiteBoardNode
 */
export function registerExternalPlugin(
  registry: import('./pluginRegistry').PluginRegistry,
  plugin: PluginDefinition
): void {
  registry.register(plugin);
  console.log(`[UnifiedPluginRegistry] External plugin registered: ${plugin.name} (${plugin.id})`);
}
