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
import { PatternRegistry } from '../patterns/registry';
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
    // New unified pattern template for h1-h6 auto-conversion
    patternTemplate: {
      regex: /^(#{1,6})\s/,
      nodeType: 'header',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: undefined, // Will be extracted from regex
      cursorPlacement: 'after-prefix',
      extractMetadata: (match: RegExpMatchArray) => ({
        headerLevel: match[1].length // Capture group 1 is the hashtags
      })
    } as PatternTemplate,
    // Keep legacy pattern detection for backward compatibility
    patternDetection: [
      {
        pattern: /^(#{1,6})\s/,
        targetNodeType: 'header',
        cleanContent: false, // Keep "# " syntax in content for editing
        extractMetadata: (match: RegExpMatchArray) => ({
          headerLevel: match[1].length // Capture group 1 is the hashtags
        }),
        priority: 10
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
    // New unified pattern template for task checkbox auto-conversion
    // Supports: [ ], [x], [X], - [ ], - [x], * [ ], * [x], + [ ], + [x]
    patternTemplate: {
      regex: /^[-*+]?\s*\[\s*[xX\s]\s*\]\s/,
      nodeType: 'task',
      priority: 10,
      splittingStrategy: 'simple-split',
      cursorPlacement: 'start',
      extractMetadata: (match: RegExpMatchArray) => {
        // Check if checkbox is marked (contains 'x' or 'X')
        const isCompleted = /[xX]/.test(match[0]);
        return {
          taskState: isCompleted ? 'completed' : 'pending'
        };
      }
    } as PatternTemplate,
    // Keep legacy pattern detection for backward compatibility
    patternDetection: [
      {
        pattern: /^[-*+]?\s*\[\s*[xX\s]\s*\]\s/,
        targetNodeType: 'task',
        cleanContent: true, // Remove "[ ]" from content, task state shown in icon
        extractMetadata: (match: RegExpMatchArray) => {
          // Check if checkbox is marked (contains 'x' or 'X')
          const isCompleted = /[xX]/.test(match[0]);
          return {
            taskState: isCompleted ? 'completed' : 'pending'
          };
        },
        priority: 10
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/task-node.svelte'),
    priority: 1
  },
  // No viewer - task nodes use BaseNodeViewer (default) for page-level display
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
  // No viewer - ai-chat nodes use BaseNodeViewer (default) for page-level display
  // No node component yet - needs to be created
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
    // New unified pattern template for ``` auto-conversion
    // Requires newline: user types ```, then presses Shift+Enter to trigger
    patternTemplate: {
      regex: /^```(\w+)?\n/,
      nodeType: 'code-block',
      priority: 10,
      splittingStrategy: 'simple-split',
      cursorPlacement: 'start',
      contentTemplate: '```\n\n```', // Auto-complete with closing fence
      extractMetadata: (match: RegExpMatchArray) => ({
        language: match[1]?.toLowerCase() || 'plaintext'
      })
    } as PatternTemplate,
    // Keep legacy pattern detection for backward compatibility
    patternDetection: [
      {
        pattern: /^```(\w+)?\n/,
        targetNodeType: 'code-block',
        cleanContent: false, // Keep ``` in content for language parsing
        contentTemplate: '```\n\n```', // Auto-complete with closing fence
        extractMetadata: (match: RegExpMatchArray) => ({
          language: match[1]?.toLowerCase() || 'plaintext'
        }),
        priority: 10,
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
  }
};

export const quoteBlockNodePlugin: PluginDefinition = {
  id: 'quote-block',
  name: 'Quote Block Node',
  description: 'Block quote with markdown styling conventions',
  version: '1.0.0',
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
    // New unified pattern template for > auto-conversion
    patternTemplate: {
      regex: /^>\s/,
      nodeType: 'quote-block',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '> ',
      cursorPlacement: 'after-prefix',
      extractMetadata: () => ({})
    } as PatternTemplate,
    // Keep legacy pattern detection for backward compatibility
    patternDetection: [
      {
        pattern: /^>\s/,
        targetNodeType: 'quote-block',
        /**
         * cleanContent: false - CRITICAL: Quote block content MUST retain "> " prefix
         *
         * Unlike headers (which strip "# "), quote blocks store the prefix in database.
         * This allows multiline quotes where each line has "> " prefix.
         *
         * Why this matters:
         * - Backend validation requires "> " prefix to identify quote blocks
         * - Multiline quotes need per-line prefix tracking ("> Line1\n> Line2")
         * - Conversion back to text requires knowing which lines were quoted
         *
         * Removing this flag would break quote-block persistence entirely.
         */
        cleanContent: false,
        extractMetadata: () => ({}),
        priority: 10,
        desiredCursorPosition: 2 // Place cursor after "> "
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
  }
};

export const orderedListNodePlugin: PluginDefinition = {
  id: 'ordered-list',
  name: 'Ordered List Node',
  description: 'Auto-numbered ordered list items',
  version: '1.0.0',
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
    // New unified pattern template for "1. " auto-conversion
    patternTemplate: {
      regex: /^1\.\s/,
      nodeType: 'ordered-list',
      priority: 10,
      splittingStrategy: 'prefix-inheritance',
      prefixToInherit: '1. ',
      cursorPlacement: 'after-prefix',
      extractMetadata: () => ({})
    } as PatternTemplate,
    // Keep legacy pattern detection for backward compatibility
    patternDetection: [
      {
        pattern: /^1\.\s/,
        targetNodeType: 'ordered-list',
        /**
         * cleanContent: false - CRITICAL: Content MUST retain "1. " prefix
         *
         * Similar to quote-block ("> "), ordered lists store prefix in database.
         * The "1. " prefix is:
         * - Stored in database for round-trip consistency
         * - Stripped in view mode for auto-numbering display
         * - Shown in edit mode for user visibility
         *
         * Auto-numbering (1, 2, 3...) happens via CSS counters during rendering.
         */
        cleanContent: false,
        extractMetadata: () => ({}),
        priority: 10,
        desiredCursorPosition: 3 // Place cursor after "1. "
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
// like WhiteBoardNode, ImageNode, etc. in separate packages
export const corePlugins = [
  textNodePlugin,
  headerNodePlugin,
  taskNodePlugin,
  aiChatNodePlugin,
  dateNodePlugin,
  codeBlockNodePlugin,
  quoteBlockNodePlugin,
  orderedListNodePlugin
  // Note: userNodePlugin and documentNodePlugin reserved for future use
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

    // Register pattern template with PatternRegistry if present
    if (plugin.config.patternTemplate) {
      patternRegistry.register(plugin.config.patternTemplate);
    }
  }

  // Log registration statistics
  const stats = registry.getStats();
  const patternStats = patternRegistry.getStats();
  console.log('[UnifiedPluginRegistry] Core plugins registered:', {
    plugins: stats.pluginsCount,
    slashCommands: stats.slashCommandsCount,
    viewers: stats.viewersCount,
    references: stats.referencesCount
  });
  console.log('[PatternRegistry] Patterns registered:', {
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
  console.log(`[UnifiedPluginRegistry] External plugin registered: ${plugin.name} (${plugin.id})`);
}
