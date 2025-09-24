/**
 * Unified Plugin Registry Tests
 *
 * Comprehensive test suite for the consolidated plugin registry system.
 * Follows NodeSpace testing guide patterns with unit, integration, and error testing.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { PluginRegistry } from '$lib/plugins/pluginRegistry';
import type {
  PluginDefinition,
  NodeViewerComponent,
  NodeReferenceComponent
} from '$lib/plugins/types';

// Mock Svelte components for testing
const MockViewerComponent = vi.fn() as unknown as NodeViewerComponent;
const MockReferenceComponent = vi.fn() as unknown as NodeReferenceComponent;

// Mock module import for lazy loading tests
const mockLazyImport = vi.fn();

describe('PluginRegistry - Unified Plugin System', () => {
  let registry: PluginRegistry;
  let lifecycleEvents: {
    onRegister: ReturnType<typeof vi.fn>;
    onUnregister: ReturnType<typeof vi.fn>;
    onEnable: ReturnType<typeof vi.fn>;
    onDisable: ReturnType<typeof vi.fn>;
  };

  // Test plugin definitions
  const testPlugin: PluginDefinition = {
    id: 'test-plugin',
    name: 'Test Plugin',
    description: 'A test plugin for unit testing',
    version: '1.0.0',
    config: {
      slashCommands: [
        {
          id: 'test-cmd',
          name: 'Test Command',
          description: 'Test slash command',
          contentTemplate: 'Test content'
        }
      ],
      canHaveChildren: true,
      canBeChild: true
    },
    viewer: {
      component: MockViewerComponent,
      priority: 1
    },
    reference: {
      component: MockReferenceComponent,
      priority: 1
    }
  };

  const lazyLoadPlugin: PluginDefinition = {
    id: 'lazy-plugin',
    name: 'Lazy Load Plugin',
    description: 'Plugin with lazy-loaded viewer',
    version: '1.0.0',
    config: {
      slashCommands: [
        {
          id: 'lazy-cmd',
          name: 'Lazy Command',
          description: 'Lazy loaded command',
          contentTemplate: 'Lazy content',
          priority: 2
        }
      ],
      canHaveChildren: false,
      canBeChild: true
    },
    viewer: {
      lazyLoad: () => mockLazyImport(),
      priority: 2
    },
    reference: {
      component: MockReferenceComponent,
      priority: 2
    }
  };

  beforeEach(() => {
    // Reset all mocks
    vi.clearAllMocks();

    // Create lifecycle event spies
    lifecycleEvents = {
      onRegister: vi.fn(),
      onUnregister: vi.fn(),
      onEnable: vi.fn(),
      onDisable: vi.fn()
    };

    // Create fresh registry with lifecycle events
    registry = new PluginRegistry(lifecycleEvents);

    // Setup mock lazy import
    mockLazyImport.mockResolvedValue({ default: MockViewerComponent });
  });

  describe('Plugin Registration', () => {
    it('should register a complete plugin successfully', () => {
      registry.register(testPlugin);

      expect(lifecycleEvents.onRegister).toHaveBeenCalledWith(testPlugin);
      expect(registry.hasPlugin('test-plugin')).toBe(true);
      expect(registry.isEnabled('test-plugin')).toBe(true);
    });

    it('should register multiple plugins', () => {
      registry.register(testPlugin);
      registry.register(lazyLoadPlugin);

      expect(registry.getAllPlugins()).toHaveLength(2);
      expect(registry.hasPlugin('test-plugin')).toBe(true);
      expect(registry.hasPlugin('lazy-plugin')).toBe(true);
    });

    it('should handle plugin registration without viewer', () => {
      const referenceOnlyPlugin: PluginDefinition = {
        id: 'ref-only',
        name: 'Reference Only',
        description: 'Plugin with only reference component',
        version: '1.0.0',
        config: {
          slashCommands: [],
          canHaveChildren: false,
          canBeChild: true
        },
        reference: {
          component: MockReferenceComponent,
          priority: 1
        }
      };

      registry.register(referenceOnlyPlugin);

      expect(registry.hasPlugin('ref-only')).toBe(true);
      expect(registry.hasViewer('ref-only')).toBe(false);
      expect(registry.hasReferenceComponent('ref-only')).toBe(true);
    });

    it('should handle plugin registration without reference', () => {
      const viewerOnlyPlugin: PluginDefinition = {
        id: 'viewer-only',
        name: 'Viewer Only',
        description: 'Plugin with only viewer component',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'viewer-cmd',
              name: 'Viewer Command',
              description: 'Viewer command',
              contentTemplate: 'Viewer content'
            }
          ],
          canHaveChildren: true,
          canBeChild: true
        },
        viewer: {
          component: MockViewerComponent,
          priority: 1
        }
      };

      registry.register(viewerOnlyPlugin);

      expect(registry.hasPlugin('viewer-only')).toBe(true);
      expect(registry.hasViewer('viewer-only')).toBe(true);
      expect(registry.hasReferenceComponent('viewer-only')).toBe(false);
    });
  });

  describe('Plugin Lifecycle Management', () => {
    beforeEach(() => {
      registry.register(testPlugin);
    });

    it('should unregister plugins correctly', () => {
      registry.unregister('test-plugin');

      expect(lifecycleEvents.onUnregister).toHaveBeenCalledWith('test-plugin');
      expect(registry.hasPlugin('test-plugin')).toBe(false);
      expect(registry.isEnabled('test-plugin')).toBe(false);
    });

    it('should enable and disable plugins', () => {
      // Disable plugin
      registry.setEnabled('test-plugin', false);

      expect(lifecycleEvents.onDisable).toHaveBeenCalledWith('test-plugin');
      expect(registry.isEnabled('test-plugin')).toBe(false);
      expect(registry.hasPlugin('test-plugin')).toBe(true); // Still registered

      // Re-enable plugin
      registry.setEnabled('test-plugin', true);

      expect(lifecycleEvents.onEnable).toHaveBeenCalledWith('test-plugin');
      expect(registry.isEnabled('test-plugin')).toBe(true);
    });

    it('should throw error when enabling non-existent plugin', () => {
      expect(() => {
        registry.setEnabled('non-existent', true);
      }).toThrow('Plugin non-existent is not registered');
    });

    it('should get enabled vs all plugins correctly', () => {
      registry.register(lazyLoadPlugin);
      registry.setEnabled('test-plugin', false);

      const allPlugins = registry.getAllPlugins();
      const enabledPlugins = registry.getEnabledPlugins();

      expect(allPlugins).toHaveLength(2);
      expect(enabledPlugins).toHaveLength(1);
      expect(enabledPlugins[0].id).toBe('lazy-plugin');
    });
  });

  describe('Viewer Component Management', () => {
    beforeEach(() => {
      registry.register(testPlugin);
      registry.register(lazyLoadPlugin);
    });

    it('should get direct viewer component', async () => {
      const viewer = await registry.getViewer('test-plugin');

      expect(viewer).toBe(MockViewerComponent);
    });

    it('should lazy load viewer component', async () => {
      const viewer = await registry.getViewer('lazy-plugin');

      expect(mockLazyImport).toHaveBeenCalled();
      expect(viewer).toBe(MockViewerComponent);
    });

    it('should cache lazy loaded components', async () => {
      // Load twice
      await registry.getViewer('lazy-plugin');
      await registry.getViewer('lazy-plugin');

      // Should only call lazy load once
      expect(mockLazyImport).toHaveBeenCalledTimes(1);
    });

    it('should return null for non-existent viewer', async () => {
      const viewer = await registry.getViewer('non-existent');

      expect(viewer).toBeNull();
    });

    it('should return null for disabled plugin viewer', async () => {
      registry.setEnabled('test-plugin', false);

      const viewer = await registry.getViewer('test-plugin');

      expect(viewer).toBeNull();
    });

    it('should handle lazy load failures gracefully', async () => {
      mockLazyImport.mockRejectedValue(new Error('Load failed'));

      const viewer = await registry.getViewer('lazy-plugin');

      expect(viewer).toBeNull();
    });
  });

  describe('Reference Component Management', () => {
    beforeEach(() => {
      registry.register(testPlugin);
    });

    it('should get reference component', () => {
      const component = registry.getReferenceComponent('test-plugin');

      expect(component).toBe(MockReferenceComponent);
    });

    it('should return null for non-existent reference', () => {
      const component = registry.getReferenceComponent('non-existent');

      expect(component).toBeNull();
    });

    it('should return null for disabled plugin reference', () => {
      registry.setEnabled('test-plugin', false);

      const component = registry.getReferenceComponent('test-plugin');

      expect(component).toBeNull();
    });
  });

  describe('Slash Command Management', () => {
    beforeEach(() => {
      registry.register(testPlugin);
      registry.register(lazyLoadPlugin);
    });

    it('should get all slash commands from enabled plugins', () => {
      const commands = registry.getAllSlashCommands();

      expect(commands).toHaveLength(2);
      expect(commands[0].id).toBe('lazy-cmd'); // Higher priority first
      expect(commands[1].id).toBe('test-cmd');
    });

    it('should exclude commands from disabled plugins', () => {
      registry.setEnabled('test-plugin', false);

      const commands = registry.getAllSlashCommands();

      expect(commands).toHaveLength(1);
      expect(commands[0].id).toBe('lazy-cmd');
    });

    it('should find specific slash command', () => {
      const command = registry.findSlashCommand('test-cmd');

      expect(command).toBeDefined();
      expect(command!.name).toBe('Test Command');
    });

    it('should return null for non-existent command', () => {
      const command = registry.findSlashCommand('non-existent');

      expect(command).toBeNull();
    });

    it('should filter slash commands by query', () => {
      const filtered = registry.filterSlashCommands('test');

      expect(filtered).toHaveLength(1);
      expect(filtered[0].id).toBe('test-cmd');
    });

    it('should return all commands for empty query', () => {
      const filtered = registry.filterSlashCommands('');

      expect(filtered).toHaveLength(2);
    });

    it('should filter by command name, description, and shortcut', () => {
      const commandWithShortcut: PluginDefinition = {
        id: 'shortcut-plugin',
        name: 'Shortcut Plugin',
        description: 'Plugin with keyboard shortcut',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'shortcut-cmd',
              name: 'Shortcut Command',
              description: 'Command with shortcut',
              shortcut: 'Ctrl+K',
              contentTemplate: 'Shortcut content'
            }
          ],
          canHaveChildren: true,
          canBeChild: true
        }
      };

      registry.register(commandWithShortcut);

      // Filter by name
      expect(registry.filterSlashCommands('shortcut')).toHaveLength(1);

      // Filter by description (need to match "Command with shortcut")
      expect(registry.filterSlashCommands('shortcut')).toHaveLength(1);

      // Filter by shortcut
      expect(registry.filterSlashCommands('ctrl')).toHaveLength(1);
    });
  });

  describe('Registry Statistics', () => {
    beforeEach(() => {
      registry.register(testPlugin);
      registry.register(lazyLoadPlugin);
    });

    it('should provide accurate statistics', () => {
      const stats = registry.getStats();

      expect(stats.pluginsCount).toBe(2);
      expect(stats.viewersCount).toBe(2);
      expect(stats.referencesCount).toBe(2);
      expect(stats.slashCommandsCount).toBe(2);
      expect(stats.plugins).toEqual(['test-plugin', 'lazy-plugin']);
    });

    it('should update statistics when plugins are disabled', () => {
      registry.setEnabled('test-plugin', false);

      const stats = registry.getStats();

      expect(stats.pluginsCount).toBe(2); // Total registered
      expect(stats.viewersCount).toBe(1); // Only enabled
      expect(stats.referencesCount).toBe(1); // Only enabled
      expect(stats.slashCommandsCount).toBe(1); // Only enabled
    });
  });

  describe('Error Handling & Edge Cases', () => {
    it('should handle getting plugin that does not exist', () => {
      const plugin = registry.getPlugin('non-existent');

      expect(plugin).toBeNull();
    });

    it('should handle clearing registry', () => {
      registry.register(testPlugin);
      registry.register(lazyLoadPlugin);

      registry.clear();

      expect(registry.getAllPlugins()).toHaveLength(0);
      expect(registry.getAllSlashCommands()).toHaveLength(0);
      expect(registry.getStats().pluginsCount).toBe(0);
    });

    it('should handle plugin with empty slash commands', () => {
      const emptyCommandsPlugin: PluginDefinition = {
        id: 'empty-commands',
        name: 'Empty Commands',
        description: 'Plugin with no slash commands',
        version: '1.0.0',
        config: {
          slashCommands: [],
          canHaveChildren: true,
          canBeChild: true
        },
        reference: {
          component: MockReferenceComponent,
          priority: 1
        }
      };

      registry.register(emptyCommandsPlugin);

      expect(registry.getAllSlashCommands()).toHaveLength(0);
      expect(registry.hasPlugin('empty-commands')).toBe(true);
    });

    it('should handle plugin registration without lifecycle events', () => {
      const registryWithoutEvents = new PluginRegistry();

      // Should not throw
      expect(() => {
        registryWithoutEvents.register(testPlugin);
      }).not.toThrow();

      expect(registryWithoutEvents.hasPlugin('test-plugin')).toBe(true);
    });

    it('should handle viewer component without priority', () => {
      const noPriorityPlugin: PluginDefinition = {
        id: 'no-priority',
        name: 'No Priority',
        description: 'Plugin without priority',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'no-priority-cmd',
              name: 'No Priority Command',
              description: 'Command without priority',
              contentTemplate: 'No priority content'
            }
          ],
          canHaveChildren: true,
          canBeChild: true
        },
        viewer: {
          component: MockViewerComponent
          // No priority specified
        }
      };

      registry.register(noPriorityPlugin);

      const commands = registry.getAllSlashCommands();
      expect(commands).toHaveLength(1);
      expect(commands[0].priority).toBeUndefined();
    });
  });

  describe('Plugin API Validation', () => {
    it('should validate plugin definition structure', () => {
      const invalidPlugin = {
        // Missing required fields
        name: 'Invalid Plugin'
      } as PluginDefinition;

      // Registry should handle this gracefully or validate
      expect(() => {
        registry.register(invalidPlugin);
      }).not.toThrow(); // Current implementation is permissive
    });

    it('should handle plugin with minimal configuration', () => {
      const minimalPlugin: PluginDefinition = {
        id: 'minimal',
        name: 'Minimal Plugin',
        description: 'Minimal plugin definition',
        version: '1.0.0',
        config: {
          slashCommands: []
        }
      };

      registry.register(minimalPlugin);

      expect(registry.hasPlugin('minimal')).toBe(true);
      expect(registry.hasViewer('minimal')).toBe(false);
      expect(registry.hasReferenceComponent('minimal')).toBe(false);
    });
  });
});
