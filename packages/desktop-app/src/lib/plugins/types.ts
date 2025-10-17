/**
 * Unified Plugin Registry Types
 *
 * Consolidates all plugin-related interfaces into a single, comprehensive system.
 * Replaces ViewerRegistry, NODE_REFERENCE_COMPONENTS, and BasicNodeTypeRegistry.
 */

import type { SvelteComponent, Component } from 'svelte';
import type { NodeViewerProps } from '../types/node-viewers';

// Base component types - match existing nodeViewers.ts definitions
export type NodeViewerComponent = Component<NodeViewerProps>;
export type NodeComponent = Component<NodeViewerProps>; // Individual node components (TaskNode, TextNode, etc.)
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
}

// Node type configuration
export interface NodeTypeConfig {
  slashCommands: SlashCommandDefinition[];
  /**
   * Pattern detection configurations for auto-converting node types
   * Example: Header plugin defines /^(#{1,6})\s/ to auto-convert text → header
   */
  patternDetection?: PatternDetectionConfig[];
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
  node?: NodeRegistration; // Individual node component (TaskNode, TextNode, etc.)
  viewer?: ViewerRegistration; // Rich viewer component (TaskNodeViewer, DateNodeViewer, etc.)
  reference?: ReferenceRegistration;
}

// Registry statistics for debugging
export interface RegistryStats {
  pluginsCount: number;
  viewersCount: number;
  referencesCount: number;
  slashCommandsCount: number;
  plugins: string[];
}

// Plugin lifecycle events
export interface PluginLifecycleEvents {
  onRegister?: (plugin: PluginDefinition) => void;
  onUnregister?: (pluginId: string) => void;
  onEnable?: (pluginId: string) => void;
  onDisable?: (pluginId: string) => void;
}
