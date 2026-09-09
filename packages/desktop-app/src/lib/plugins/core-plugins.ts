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
import { parseDateString, formatDateTitle } from '$lib/utils/date-formatting';
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
    lazyLoad: () => import('../design/components/text-node.svelte'),
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
  // Plugin-owned pattern behavior
  pattern: {
    detect: /^(#{1,6})\s/,
    canRevert: true,
    revert: /^#{1,6}$/, // "# " → "#" should revert to text
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
  // No editor pattern — tasks are created via /task slash command only.
  // Typed "- [ ]" syntax creates a checkbox node (checkboxNodePlugin pattern).
  config: {
    slashCommands: [
      {
        id: 'task',
        name: 'Task',
        description: 'Create a task with checkbox',
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
  // TaskNodeViewer for task-specific UI
  viewer: {
    lazyLoad: () => import('../components/viewers/task-node-viewer.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  // Type-specific metadata extraction
  // Backend returns TaskNode with status at TOP LEVEL (flat type-specific fields)
  // Also supports generic Node where status is in properties (for SSE events)
  extractMetadata: (node: {
    nodeType: string;
    status?: string;
    priority?: string | number;
    properties?: Record<string, unknown>;
  }) => {
    const properties = node.properties || {};
    // Check top-level status first (TaskNode format), fall back to properties.status
    // TaskNode has status at node.status, generic Node has it at node.properties.status
    const status = node.status ?? properties.status;
    const priority = node.priority ?? properties.priority;

    // Map task status to NodeState expected by TaskNode component
    let taskState: 'pending' | 'inProgress' | 'completed' = 'pending';
    if (status === 'IN_PROGRESS' || status === 'in_progress') {
      taskState = 'inProgress';
    } else if (status === 'DONE' || status === 'done') {
      taskState = 'completed';
    } else if (status === 'OPEN' || status === 'open') {
      taskState = 'pending';
    } else if (status === 'CANCELLED' || status === 'cancelled') {
      taskState = 'completed';
    }

    // Spread properties first, then override with resolved top-level values
    // This ensures top-level type-specific fields take precedence over properties
    return { ...properties, taskState, status, priority };
  },
  // Type-specific state mapping
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

  // Type-specific updater for task node properties
  // Routes to updateTaskNode() instead of generic updateNode()
  updater: {
    update: async (id: string, version: number, changes: Record<string, unknown>) => {
      // Convert changes to TaskNodeUpdate format
      // The caller provides type-safe changes, we map to the backend format
      const update: TaskNodeUpdate = {};
      if ('status' in changes && changes.status !== undefined)
        update.status = changes.status as TaskNodeUpdate['status'];
      if ('priority' in changes) update.priority = changes.priority as TaskNodeUpdate['priority'];
      if ('dueDate' in changes) update.dueDate = changes.dueDate as TaskNodeUpdate['dueDate'];
      if ('startedAt' in changes)
        update.startedAt = changes.startedAt as TaskNodeUpdate['startedAt'];
      if ('completedAt' in changes)
        update.completedAt = changes.completedAt as TaskNodeUpdate['completedAt'];
      if ('content' in changes && changes.content !== undefined)
        update.content = changes.content as string;

      // A caller (e.g. Kanban grouped by a field this updater doesn't map —
      // see `resolveFieldWrite`'s doc comment on that being out of scope)
      // can hand this a `changes` object with no field this updater
      // recognizes. Sending an empty `TaskNodeUpdate` to the backend would
      // still be rejected ("TaskNodeUpdate contains no changes"), but only
      // as an opaque, unhelpful `[ERROR] updateTaskNode – Object` once it
      // reaches the daemon. Failing loudly here, with the fields nobody
      // recognized, is diagnosable — the previous version silently reduced
      // to `{}` and let a generic backend rejection stand in for it.
      if (Object.keys(update).length === 0) {
        throw new Error(
          `Task update has no fields this updater can map (received: ${Object.keys(changes).join(', ') || '(none)'})`
        );
      }

      // Returns TaskNode which has node fields but not properties (flat structure)
      // Cast to Node for interface compatibility - sharedNodeStore will handle appropriately
      const result = await backendAdapter.updateTaskNode(id, version, update);
      return result as unknown as import('../types').Node;
    }
  },

  // Type-specific schema form for task node properties
  schemaForm: {
    lazyLoad: () => import('../components/property-forms/task-schema-form.svelte')
  }
};

// Checkbox node - pure content node, state encoded in content string
// No task management semantics; toggling updates the content in place
export const checkboxNodePlugin: PluginDefinition = {
  id: 'checkbox',
  name: 'Checkbox Node',
  description: 'A simple checkbox item — markdown annotation, not a managed task',
  version: '1.0.0',
  // Pattern: "- [ ] " or "- [x] " typed in the editor creates a checkbox node
  pattern: {
    detect: /^- \[[ xX]\] /,
    canRevert: true, // prefix preserved in content; breaking "- [ ] " reverts node to text
    revert: /^-[ \[]?$|^- \[[ xX]?\]?$/, // partially-deleted prefix (e.g. "- [" or "- [ ]") → revert to text
    onEnter: 'inherit',
    prefixToInherit: '- [ ] ',
    splittingStrategy: 'simple-split',
    cursorPlacement: 'start',
    extractMetadata: () => ({})
  },
  config: {
    slashCommands: [
      {
        id: 'checkbox',
        name: 'Checkbox',
        description: 'Create a simple checkbox item',
        shortcut: '- [ ] ',
        contentTemplate: '- [ ] ',
        nodeType: 'checkbox',
        desiredCursorPosition: 6 // Position cursor after "- [ ] "
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/checkbox-node.svelte'),
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
  viewer: {
    lazyLoad: () => import('../components/viewers/date-node-viewer.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  // Date node ids ARE dates ("2026-07-07") — the display title ("Today", "Tomorrow",
  // or the raw date string) is computed from the id, not from node.content.
  getTitle: (node) => {
    const date = parseDateString(node.id);
    return date ? formatDateTitle(date) : undefined;
  }
};

export const codeBlockNodePlugin: PluginDefinition = {
  id: 'code-block',
  name: 'Code Block Node',
  description: 'Code snippet with language selection and syntax',
  version: '1.0.0',
  // Plugin-owned pattern behavior
  pattern: {
    detect: /^```\n/, // Matches ``` followed immediately by newline (language set via dropdown only)
    canRevert: true,
    revert: /^```$/, // "```" alone should revert to text
    onEnter: 'none', // Code blocks don't inherit on Enter
    splittingStrategy: 'simple-split',
    cursorPlacement: 'start',
    extractMetadata: () => ({
      language: 'plaintext' // Default language; user selects via dropdown
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
  // Structured content - cannot accept arbitrary merges
  acceptsContentMerge: false
};

export const quoteBlockNodePlugin: PluginDefinition = {
  id: 'quote-block',
  name: 'Quote Block Node',
  description: 'Block quote with markdown styling conventions',
  version: '1.0.0',
  // Plugin-owned pattern behavior
  pattern: {
    detect: /^>\s/,
    canRevert: true,
    revert: /^>$/, // "> " → ">" should revert to text
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
  // Structured content - cannot accept arbitrary merges
  acceptsContentMerge: false
};

export const orderedListNodePlugin: PluginDefinition = {
  id: 'ordered-list',
  name: 'Ordered List Node',
  description: 'Auto-numbered ordered list items',
  version: '1.0.0',
  // Plugin-owned pattern behavior
  pattern: {
    detect: /^1\.\s/,
    canRevert: true,
    revert: /^1\.$/, // "1. " → "1." should revert to text
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

export const horizontalLineNodePlugin: PluginDefinition = {
  id: 'horizontal-line',
  name: 'Horizontal Line',
  description: 'Horizontal rule / thematic break',
  version: '1.0.0',
  pattern: {
    detect: /^[-*_]{3,}$/,
    canRevert: true,
    onEnter: 'none',
    splittingStrategy: 'simple-split',
    cursorPlacement: 'start',
    extractMetadata: () => ({})
  },
  config: {
    slashCommands: [
      {
        id: 'hr',
        name: 'Horizontal Line',
        description: 'Insert a horizontal rule',
        shortcut: '---',
        contentTemplate: '---',
        nodeType: 'horizontal-line'
      }
    ],
    canHaveChildren: false,
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/horizontal-line-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  acceptsContentMerge: false
};

export const tableNodePlugin: PluginDefinition = {
  id: 'table',
  name: 'Table',
  description: 'GFM markdown table with alignment support',
  version: '1.0.0',
  pattern: {
    detect: /^\|\s/,
    canRevert: true,
    revert: /^\|$/,
    onEnter: 'none',
    splittingStrategy: 'simple-split',
    cursorPlacement: 'start',
    extractMetadata: () => ({})
  },
  config: {
    slashCommands: [
      {
        id: 'table',
        name: 'Table',
        description: 'Create a markdown table',
        shortcut: '|',
        contentTemplate: '| Column 1 | Column 2 |\n| --- | --- |\n| | |',
        nodeType: 'table',
        desiredCursorPosition: 2
      }
    ],
    canHaveChildren: false,
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/table-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  acceptsContentMerge: false
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
    // No manual `/query` slash command: it created definition-less
    // query nodes (no targetType/filters) that cannot render. Query nodes are now
    // materialized from a type's default view, or created by AI/MCP. The node,
    // viewer, and reference registrations below are intentionally retained so
    // existing and AI/MCP-created query nodes are unaffected.
    slashCommands: [],
    canHaveChildren: false, // Query nodes are leaf nodes
    canBeChild: true
  },
  node: {
    lazyLoad: () => import('../design/components/query-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  viewer: {
    lazyLoad: () => import('../components/viewers/query-node-viewer.svelte'),
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
  // CollectionNodeViewer for collection-specific UI
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

/**
 * AI Chat Node Plugin
 *
 * AI chat conversations stored as first-class knowledge graph nodes.
 * Messages are nested properties (ADR-028), enabling semantic search,
 * Stamped ACL permissions, and cloud sync.
 */
export const aiChatNodePlugin: PluginDefinition = {
  id: 'ai-chat',
  name: 'AI Chat',
  description: 'AI conversation node',
  version: '1.0.0',
  // The conversation lives in properties.messages, not .content — a start-of-node
  // Backspace must never silently delete or merge it away (enforced in
  // handleDeleteNode / handleCombineWithPrevious in base-node-viewer.svelte).
  deletableViaBackspace: false,
  // ai-chat's .content is not a normal editable text field, so (like code-block /
  // quote-block) it must not absorb a Backspace-merge from the node below it.
  acceptsContentMerge: false,
  config: {
    // No slash command — entity types are not slash-creatable. AI chats are created
    // via the sidebar's dedicated "AI Chats" section ("+ New chat").
    slashCommands: [],
    canHaveChildren: false,
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

// Export all core plugins
// These are the foundation plugins - external developers can create additional plugins
// like WhiteBoardNode, ImageNode, etc. in separate packages
export const personNodePlugin: PluginDefinition = {
  id: 'person',
  name: 'Person',
  description: 'Create a person node — contact, collaborator, or stakeholder',
  version: '1.0.0',
  config: {
    // No slash command — entity types are not slash-creatable. Person nodes are
    // created via the sidenav's type view (SIDENAV_CORE_TYPES → create instance).
    slashCommands: [],
    canHaveChildren: false,
    canBeChild: true
  },
  // person is a static core plugin (not schema-plugin-loader-driven), so its
  // title_template-derived flags are declared here rather than read off the
  // stored schema — must match core_schemas.rs's person title_template.
  hasTitleTemplate: true,
  titleTemplate: '{first_name} {last_name}',
  // "Person" is a real entity noun, not a registry label like "Text Node" — even though
  // person also has an inline node component below (unlike other entity-noun types, which
  // render as read-only entity rows). See PluginDefinition.entityNoun's doc comment.
  entityNoun: true,
  node: {
    lazyLoad: () => import('../design/components/person-node.svelte'),
    priority: 1
  },
  reference: {
    component: BaseNodeReference as NodeReferenceComponent,
    priority: 1
  },
  schemaForm: {
    lazyLoad: () => import('../components/property-forms/person-schema-form.svelte')
  }
};

/**
 * Which types get a slash command
 *
 * Slash commands are for **content primitives** — the building blocks you type while
 * outlining: text, headers, checkboxes, code, quotes, ordered lists, rules, tables.
 * `task` is the one deliberate exception, because capturing a task mid-thought is core to
 * outlining.
 *
 * Entity types should **not** be slash-creatable — neither core entity types nor
 * user-defined schema types. Projects, collections, documents and users are containers and
 * records set up deliberately — by the agent, or through the schema/entity surfaces — not
 * typed in ad hoc mid-outline. They stay reachable: most core entity types are listed in
 * the sidenav as user-queryable types and open to schema-driven properties forms
 * (`SIDENAV_CORE_TYPES`), and the rest reach creation through a surface of their own —
 * `person` and every user-defined type through the sidenav's type view, `ai-chat` through
 * its own dedicated sidebar section.
 *
 * `project` therefore has no plugin here at all. If you are here because `/project` seems
 * to be "missing", it is absent by decision, not by oversight. Note that `project` renders
 * as a read-only entity row regardless — that follows from having no inline node component
 * (see `rendersAsEntityRow`), independent of registration.
 */
export const corePlugins = [
  textNodePlugin,
  headerNodePlugin,
  taskNodePlugin,
  checkboxNodePlugin,
  dateNodePlugin,
  codeBlockNodePlugin,
  quoteBlockNodePlugin,
  orderedListNodePlugin,
  horizontalLineNodePlugin,
  tableNodePlugin,
  queryNodePlugin,
  collectionNodePlugin,
  aiChatNodePlugin,
  personNodePlugin
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

    // Register pattern with PatternRegistry if present
    // Convert PluginPattern to PatternTemplate for PatternRegistry compatibility
    if (plugin.pattern) {
      const patternTemplate: PatternTemplate = {
        regex: plugin.pattern.detect,
        nodeType: plugin.id,
        priority: 10, // Default priority
        splittingStrategy: plugin.pattern.splittingStrategy,
        // For headers with function-based prefix, PatternRegistry will extract from regex
        prefixToInherit:
          typeof plugin.pattern.prefixToInherit === 'string'
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
