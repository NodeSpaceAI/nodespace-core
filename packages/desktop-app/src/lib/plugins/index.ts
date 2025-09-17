/**
 * Unified Plugin System
 *
 * Export all plugin-related functionality from a single entry point.
 * This replaces the old fragmented registry system.
 */

export { PluginRegistry, pluginRegistry } from './pluginRegistry';
export type {
  PluginDefinition,
  NodeViewerComponent,
  NodeReferenceComponent,
  ViewerRegistration,
  ReferenceRegistration,
  SlashCommandDefinition,
  NodeTypeConfig,
  RegistryStats,
  PluginLifecycleEvents
} from './types';
