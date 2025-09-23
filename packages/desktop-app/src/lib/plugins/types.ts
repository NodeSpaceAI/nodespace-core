/**
 * Unified Plugin Registry Types
 *
 * Consolidates all plugin-related interfaces into a single, comprehensive system.
 * Replaces ViewerRegistry, NODE_REFERENCE_COMPONENTS, and BasicNodeTypeRegistry.
 */

import type { SvelteComponent, Component } from 'svelte';
import type { NodeViewerProps } from '$lib/types/nodeViewers';

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
  node?: NodeRegistration;      // Individual node component (TaskNode, TextNode, etc.)
  viewer?: ViewerRegistration;  // Rich viewer component (TaskNodeViewer, DatePageViewer, etc.)
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
