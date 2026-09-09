/**
 * Unified Plugin Registry Types
 *
 * Consolidates all plugin-related interfaces into a single, comprehensive system.
 * Replaces ViewerRegistry, NODE_REFERENCE_COMPONENTS, and BasicNodeTypeRegistry.
 */

import type { SvelteComponent, Component } from 'svelte';
import type { NodeViewerProps, NodeComponentProps } from '../types/node-viewers';
import type { PatternTemplate } from '../patterns/types';
import type { Node } from '../types';

// Base component types - match existing nodeViewers.ts definitions
export type NodeViewerComponent = Component<NodeViewerProps>; // Page-level viewers (DateNodeViewer, BaseNodeViewer)
export type NodeComponent = Component<NodeComponentProps>; // Individual node components (TaskNode, TextNode, etc.)
export type NodeReferenceComponent = new (...args: unknown[]) => SvelteComponent;

// Plugin component registration types
export interface NodeRegistration {
  component?: NodeComponent;
  lazyLoad?: () => Promise<{ default: NodeComponent }>;
  priority?: number;
}

export interface ViewerRegistration {
  component?: NodeViewerComponent;
  lazyLoad?: () => Promise<{ default: NodeViewerComponent }>;
  priority?: number;
}

export interface ReferenceRegistration {
  component: NodeReferenceComponent;
  priority?: number;
}

/**
 * Schema form component type
 * Used for type-specific property editing forms (TaskSchemaForm, PersonSchemaForm, etc.)
 */
export type SchemaFormComponent = Component<{ nodeId: string }>;

/**
 * Schema form registration for type-specific property forms
 * Allows lazy loading of schema form components
 */
export interface SchemaFormRegistration {
  component?: SchemaFormComponent;
  lazyLoad?: () => Promise<{ default: SchemaFormComponent }>;
  priority?: number;
}

/**
 * Type-specific node updater interface
 * Provides type-safe update operations for nodes with type-specific properties
 *
 * The changes parameter accepts Record<string, unknown> at the interface level,
 * but implementations can use more specific types internally.
 * Type safety is enforced at the call site (e.g., TaskSchemaForm uses TaskNodeUpdate).
 */
export interface NodeUpdater {
  /**
   * Update a node with type-specific changes
   *
   * @param id - Node ID
   * @param version - Expected version for OCC
   * @param changes - Changes to apply (type-specific fields like status, priority, etc.)
   * @returns Updated node
   */
  update: (id: string, version: number, changes: Record<string, unknown>) => Promise<Node>;
}

/**
 * Plugin-owned pattern behavior definition
 *
 * Consolidates all pattern-related behavior into the plugin system,
 * making it trivial to add new node types with pattern detection.
 * Each plugin fully owns its pattern detection, reversion, and inheritance behavior.
 *
 * Plugin-Owned Pattern Definitions for Extensibility
 */
export interface PluginPattern {
  /** Regular expression to detect in content */
  detect: RegExp;

  /** Whether this pattern can revert to text when deleted */
  canRevert: boolean;

  /** Pattern to test for reversion (e.g., "# " → "#" should revert to text) */
  revert?: RegExp;

  /** Behavior when Enter key is pressed */
  onEnter: 'inherit' | 'text' | 'none';

  /** Prefix to inherit on new line (for 'inherit' mode) */
  prefixToInherit?: string | ((content: string) => string | undefined);

  /** Strategy for splitting content when Enter is pressed */
  splittingStrategy: 'prefix-inheritance' | 'simple-split';

  /** Where to place cursor in new node after split */
  cursorPlacement: 'start' | 'after-prefix' | 'end';

  /** Optional function to extract metadata from pattern matches */
  extractMetadata?: (match: RegExpMatchArray) => Record<string, unknown>;
}

// Pattern detection configuration for auto-converting node types
export interface PatternDetectionConfig {
  /**
   * Regular expression pattern to detect in content
   * Can be a RegExp object or a string that will be converted to RegExp
   * Example: /^(#{1,6})\s/ for headers, /^\[\s*[x\s]\s*\]/i for tasks
   */
  pattern: RegExp | string;

  /**
   * Target node type when pattern is detected
   * Example: 'header' for # patterns, 'task' for [ ] patterns
   */
  targetNodeType: string;

  /**
   * Whether to remove the pattern syntax from content after detection
   * true: "# Hello" → "Hello" (clean content)
   * false: "# Hello" → "# Hello" (keep syntax in content)
   * Default: false (headers keep syntax for editing)
   */
  cleanContent?: boolean;

  /**
   * Optional function to extract metadata from pattern matches
   * Example: Extract header level from capture groups
   * @param match - The RegExp match result
   * @returns Metadata object to pass to the node component
   */
  extractMetadata?: (match: RegExpMatchArray) => Record<string, unknown>;

  /**
   * Priority for pattern matching (higher = checked first)
   * Useful when patterns might overlap
   */
  priority?: number;

  /**
   * Desired cursor position after pattern detection and conversion
   * If specified, cursor will be placed at this position after conversion
   * Example: For "> " pattern, set to 2 to place cursor after "> "
   */
  desiredCursorPosition?: number;

  /**
   * Optional content template to apply when pattern is detected
   * If specified, this template will replace the matched content
   * Useful for completing structures like code blocks: "```\n" → "```\n\n```"
   * Example: For code-block pattern, set to '```\n\n```' to auto-complete closing fence
   */
  contentTemplate?: string;
}

// Slash command definition
export interface SlashCommandDefinition {
  id: string;
  name: string;
  description: string;
  shortcut?: string;
  contentTemplate: string;
  nodeType?: string; // Target node type when this command is selected
  priority?: number;
  /**
   * Desired cursor position after slash command insertion
   * If specified, cursor will be placed at this position after insertion
   * Example: For code-block with '```plaintext\n\n```', set to 13 to place cursor after first fence line
   */
  desiredCursorPosition?: number;
}

// Node type configuration
export interface NodeTypeConfig {
  slashCommands: SlashCommandDefinition[];
  /**
   * Pattern detection configurations for auto-converting node types
   * Example: Header plugin defines /^(#{1,6})\s/ to auto-convert text → header
   * @deprecated Use patternTemplate instead - will be removed in next major version
   */
  patternDetection?: PatternDetectionConfig[];
  /**
   * Pattern template for unified pattern system (new)
   * Used by PatternRegistry and PatternSplitter for consistent pattern handling
   */
  patternTemplate?: PatternTemplate;
  canHaveChildren?: boolean;
  canBeChild?: boolean;
  defaultContent?: string;
}

// Complete plugin definition
export interface PluginDefinition {
  id: string;
  name: string;
  description: string;
  version: string;
  config: NodeTypeConfig;
  /** Plugin-owned pattern behavior */
  pattern?: PluginPattern;
  node?: NodeRegistration; // Individual node component (TaskNode, TextNode, etc.)
  viewer?: ViewerRegistration; // Rich viewer component (TaskNodeViewer, DateNodeViewer, etc.)
  reference?: ReferenceRegistration;

  /**
   * Type-specific schema form for editing type-specific properties
   *
   * Core node types with type-specific properties and bespoke UI/business logic (task,
   * person) should provide hardcoded schema forms for full TypeScript type safety.
   *
   * Everything else — user-defined types, and core types with a schema but no hardcoded
   * form (e.g. project) — falls back to the generic GenericSchemaForm.
   *
   * @example
   * schemaForm: {
   *   lazyLoad: () => import('../components/property-forms/task-schema-form.svelte')
   * }
   */
  schemaForm?: SchemaFormRegistration;

  /**
   * Type-specific updater for node properties
   *
   * Provides type-safe update operations that route to the correct
   * backend method (e.g., updateTaskNode instead of generic updateNode).
   *
   * When defined, sharedNodeStore.updateNode() will use this updater
   * instead of the generic properties-based update.
   *
   * @example
   * updater: {
   *   update: async (id, version, changes) => backendAdapter.updateTaskNode(id, version, changes)
   * }
   */
  updater?: NodeUpdater;

  /**
   * Extract and transform node properties into component-compatible metadata
   * Used to handle type-specific property transformations without hardcoding in BaseNodeViewer
   *
   * Backend returns typed nodes (e.g., TaskNode) with type-specific fields at top level.
   * The function receives node with optional top-level type-specific fields AND properties.
   *
   * @param node - Node with optional top-level type-specific fields and properties from database
   * @returns Metadata object compatible with node component expectations
   * @example
   * // TaskNode format (status at top level):
   * extractMetadata({ nodeType: 'task', status: 'in_progress', properties: {} })
   * // Returns: { taskState: 'inProgress', status: 'in_progress' }
   *
   * // Generic Node format (status in properties):
   * extractMetadata({ nodeType: 'task', properties: { status: 'in_progress' } })
   * // Returns: { taskState: 'inProgress', status: 'in_progress' }
   */
  extractMetadata?: (node: {
    nodeType: string;
    status?: string;
    priority?: string | number;
    properties?: Record<string, unknown>;
  }) => Record<string, unknown>;

  /**
   * Maps UI state to schema property value
   * Used for type-specific state transformations (e.g., task state cycling)
   *
   * Example: Task nodes map 'pending' → 'open', 'inProgress' → 'in_progress', 'completed' → 'done'
   *
   * @param state - UI state value
   * @param fieldName - Schema field name
   * @returns Schema-compatible property value
   */
  mapStateToSchema?: (state: string, fieldName: string) => unknown;

  /**
   * Whether this node type accepts content merges from adjacent nodes
   * Used to protect structured content nodes (e.g., code-block, quote-block)
   *
   * Default: true (most nodes accept merges)
   * Set to false for nodes that must maintain specific formatting
   */
  acceptsContentMerge?: boolean;

  /**
   * Whether this node type may be deleted or merged away by a Backspace at the
   * start of the node (the node acting as the *current* node, not the target).
   * Protects node types whose data lives outside `.content` — e.g. ai-chat,
   * whose conversation is stored in `properties.messages` — from silent
   * whole-node loss.
   *
   * Default: true (most nodes can be deleted via Backspace)
   * Set to false for nodes that must not be destroyed by a start-of-node Backspace
   */
  deletableViaBackspace?: boolean;

  /**
   * Compute this node's tab title. Only needed when the title isn't derivable from the
   * node's own stored fields (node.title / node.content) — e.g. date nodes, whose display
   * title ("Today", "Tomorrow", a date string) is computed from parsing the node's id, not
   * from stored content. Most node types omit this and fall back to node.title || node.content.
   */
  getTitle?: (node: Node) => string | undefined;

  /**
   * True when this type's schema is title_template-driven — `title` is a computed property
   * value, genuinely distinct from `content`, rather than a copy of it. Row-rendering surfaces
   * (query list/kanban/table views, the collections sidebar, the outline's BaseNode fallback)
   * use this — via `PluginRegistry.hasTitleTemplate()` — to decide whether to prefer `title`
   * over `content`. A per-type property, not tied to slash-command availability: user-defined
   * schema types carry it here even though they have no slash command.
   */
  hasTitleTemplate?: boolean;

  /** The raw titleTemplate string (e.g. '{first_name} {last_name}') for placeholder display
   * while the template hasn't resolved to any value yet. Only meaningful when
   * `hasTitleTemplate` is true. */
  titleTemplate?: string;

  /**
   * True when `name` is a word a user would recognize as naming the thing itself — an entity
   * noun ("Person") — rather than an internal label chosen for the plugin registry /
   * slash-command menu ("Text Node", "Header Node").
   *
   * Every type with no inline node component (`rendersAsEntityRow` true) already qualifies by
   * construction — nothing that renders only as a read-only entity row names itself for a
   * slash-command menu, so its `name` is always the noun; those types need not set this.
   * It exists for the one combination `rendersAsEntityRow` cannot see on its own: a type that
   * BOTH has an inline node component AND has an entity-noun `name` — `person` being the first
   * ("Person" is what a user calls the thing, independent of it also being inline-editable).
   *
   * Drives the "Untitled <name>" fallback for an untitled node's tab title
   * (`node-type-predicates.ts`'s `hasEntityNounName`, consumed by `navigation-service.ts`).
   * Default `false`/unset: opt in explicitly, never infer from the inline/entity-row split,
   * which answers a different question (how the type is edited, not what its name means).
   */
  entityNoun?: boolean;
}

// Registry statistics for debugging
export interface RegistryStats {
  pluginsCount: number;
  viewersCount: number;
  referencesCount: number;
  slashCommandsCount: number;
  plugins: string[];
}

/**
 * Result from pattern detection in content
 * Returned by PluginRegistry.detectPatternInContent()
 */
export interface PatternDetectionResult {
  /** Plugin that owns the matched pattern */
  plugin: PluginDefinition;
  /** Backward-compatible pattern config derived from plugin.pattern */
  config: PatternDetectionConfig;
  /** RegExp match result */
  match: RegExpMatchArray;
  /** Extracted metadata from pattern (e.g., header level) */
  metadata: Record<string, unknown>;
}

// Plugin lifecycle events
export interface PluginLifecycleEvents {
  onRegister?: (plugin: PluginDefinition) => void;
  onUnregister?: (pluginId: string) => void;
  onEnable?: (pluginId: string) => void;
  onDisable?: (pluginId: string) => void;
}
