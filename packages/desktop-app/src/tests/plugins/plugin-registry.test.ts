/**
 * Unified Plugin Registry Tests
 *
 * Comprehensive test suite for the unified plugin system that consolidates
 * ViewerRegistry, NODE_REFERENCE_COMPONENTS, and BasicNodeTypeRegistry.
 *
 * Tests follow the official NodeSpace testing guide patterns.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { PluginRegistry } from '$lib/plugins/plugin-registry';
import type {
  PluginDefinition,
  NodeViewerComponent,
  NodeReferenceComponent,
  NodeUpdater
} from '$lib/plugins/types';

// Mock Svelte component for testing
const MockViewerComponent = vi.fn() as unknown as NodeViewerComponent;
const MockReferenceComponent = vi.fn() as unknown as NodeReferenceComponent;

// Mock node updater for testing
const MockNodeUpdater: NodeUpdater = {
  update: vi.fn().mockResolvedValue({ id: 'test', nodeType: 'task', content: 'updated' })
};

describe('PluginRegistry - Core Functionality', () => {
  let registry: PluginRegistry;
  let lifecycleEvents: {
    onRegister?: (plugin: PluginDefinition) => void;
    onUnregister?: (pluginId: string) => void;
    onEnable?: (pluginId: string) => void;
    onDisable?: (pluginId: string) => void;
  };

  beforeEach(() => {
    // Reset lifecycle events tracking
    lifecycleEvents = {
      onRegister: vi.fn(),
      onUnregister: vi.fn(),
      onEnable: vi.fn(),
      onDisable: vi.fn()
    };

    registry = new PluginRegistry(lifecycleEvents);
    vi.clearAllMocks();
  });

  afterEach(() => {
    registry.clear();
  });

  describe('Plugin Registration', () => {
    it('should register a complete plugin definition', () => {
      const plugin: PluginDefinition = {
        id: 'test-plugin',
        name: 'Test Plugin',
        description: 'A test plugin for unit testing',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'test-command',
              name: 'Test Command',
              description: 'A test slash command',
              contentTemplate: 'test content'
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

      registry.register(plugin);

      expect(registry.hasPlugin('test-plugin')).toBe(true);
      expect(registry.isEnabled('test-plugin')).toBe(true);
      expect(lifecycleEvents.onRegister).toHaveBeenCalledWith(plugin);
    });

    it('should register minimal plugin (no viewer/reference)', () => {
      const plugin: PluginDefinition = {
        id: 'minimal-plugin',
        name: 'Minimal Plugin',
        description: 'Plugin with only slash commands',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'minimal-command',
              name: 'Minimal',
              description: 'Minimal command',
              contentTemplate: ''
            }
          ]
        }
      };

      registry.register(plugin);

      expect(registry.hasPlugin('minimal-plugin')).toBe(true);
      expect(registry.getAllSlashCommands()).toHaveLength(1);
      expect(registry.hasViewer('minimal-plugin')).toBe(false);
      expect(registry.hasReferenceComponent('minimal-plugin')).toBe(false);
    });

    it('should handle plugin with lazy-loaded viewer', () => {
      const plugin: PluginDefinition = {
        id: 'lazy-plugin',
        name: 'Lazy Plugin',
        description: 'Plugin with lazy-loaded viewer',
        version: '1.0.0',
        config: {
          slashCommands: []
        },
        viewer: {
          lazyLoad: () => Promise.resolve({ default: MockViewerComponent }),
          priority: 2
        }
      };

      registry.register(plugin);

      expect(registry.hasViewer('lazy-plugin')).toBe(true);
      expect(lifecycleEvents.onRegister).toHaveBeenCalledWith(plugin);
    });
  });

  describe('Plugin Retrieval', () => {
    beforeEach(() => {
      const plugin: PluginDefinition = {
        id: 'retrieval-test',
        name: 'Retrieval Test',
        description: 'Test plugin retrieval',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'cmd1',
              name: 'Command 1',
              description: 'First command',
              contentTemplate: 'content1',
              priority: 1
            },
            {
              id: 'cmd2',
              name: 'Command 2',
              description: 'Second command',
              contentTemplate: 'content2',
              priority: 2
            }
          ]
        },
        viewer: {
          component: MockViewerComponent
        },
        reference: {
          component: MockReferenceComponent
        }
      };
      registry.register(plugin);
    });

    it('should retrieve plugin by ID', () => {
      const plugin = registry.getPlugin('retrieval-test');

      expect(plugin).toBeDefined();
      expect(plugin?.name).toBe('Retrieval Test');
      expect(plugin?.config.slashCommands).toHaveLength(2);
    });

    it('should return null for non-existent plugin', () => {
      const plugin = registry.getPlugin('non-existent');
      expect(plugin).toBeNull();
    });

    it('should get all plugins', () => {
      const plugins = registry.getAllPlugins();

      expect(plugins).toHaveLength(1);
      expect(plugins[0].id).toBe('retrieval-test');
    });

    it('should get enabled plugins only', () => {
      // Register another plugin and disable it
      const disabledPlugin: PluginDefinition = {
        id: 'disabled-plugin',
        name: 'Disabled Plugin',
        description: 'This will be disabled',
        version: '1.0.0',
        config: { slashCommands: [] }
      };
      registry.register(disabledPlugin);
      registry.setEnabled('disabled-plugin', false);

      const enabledPlugins = registry.getEnabledPlugins();

      expect(enabledPlugins).toHaveLength(1);
      expect(enabledPlugins[0].id).toBe('retrieval-test');
    });
  });

  describe('Plugin State Management', () => {
    let plugin: PluginDefinition;

    beforeEach(() => {
      plugin = {
        id: 'state-test',
        name: 'State Test',
        description: 'Test plugin state management',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'state-cmd',
              name: 'State Command',
              description: 'Test state',
              contentTemplate: 'state'
            }
          ]
        }
      };
      registry.register(plugin);
    });

    it('should enable and disable plugins', () => {
      expect(registry.isEnabled('state-test')).toBe(true);

      registry.setEnabled('state-test', false);
      expect(registry.isEnabled('state-test')).toBe(false);
      expect(lifecycleEvents.onDisable).toHaveBeenCalledWith('state-test');

      registry.setEnabled('state-test', true);
      expect(registry.isEnabled('state-test')).toBe(true);
      expect(lifecycleEvents.onEnable).toHaveBeenCalledWith('state-test');
    });

    it('should throw error when enabling non-existent plugin', () => {
      expect(() => {
        registry.setEnabled('non-existent', true);
      }).toThrow('Plugin non-existent is not registered');
    });

    it('should unregister plugins', () => {
      expect(registry.hasPlugin('state-test')).toBe(true);

      registry.unregister('state-test');

      expect(registry.hasPlugin('state-test')).toBe(false);
      expect(registry.isEnabled('state-test')).toBe(false);
      expect(lifecycleEvents.onUnregister).toHaveBeenCalledWith('state-test');
    });
  });

  describe('Viewer Component Management', () => {
    it('should resolve direct viewer component', async () => {
      const plugin: PluginDefinition = {
        id: 'direct-viewer',
        name: 'Direct Viewer',
        description: 'Plugin with direct viewer component',
        version: '1.0.0',
        config: { slashCommands: [] },
        viewer: {
          component: MockViewerComponent,
          priority: 1
        }
      };

      registry.register(plugin);
      const viewer = await registry.getViewer('direct-viewer');

      expect(viewer).toBe(MockViewerComponent);
    });

    it('should resolve lazy-loaded viewer component', async () => {
      const plugin: PluginDefinition = {
        id: 'lazy-viewer',
        name: 'Lazy Viewer',
        description: 'Plugin with lazy-loaded viewer',
        version: '1.0.0',
        config: { slashCommands: [] },
        viewer: {
          lazyLoad: () => Promise.resolve({ default: MockViewerComponent }),
          priority: 1
        }
      };

      registry.register(plugin);
      const viewer = await registry.getViewer('lazy-viewer');

      expect(viewer).toBe(MockViewerComponent);
    });

    it('should return null for missing viewer', async () => {
      const viewer = await registry.getViewer('non-existent');
      expect(viewer).toBeNull();
    });

    it('should return null for disabled plugin viewer', async () => {
      const plugin: PluginDefinition = {
        id: 'disabled-viewer',
        name: 'Disabled Viewer',
        description: 'Plugin that will be disabled',
        version: '1.0.0',
        config: { slashCommands: [] },
        viewer: {
          component: MockViewerComponent
        }
      };

      registry.register(plugin);
      registry.setEnabled('disabled-viewer', false);

      const viewer = await registry.getViewer('disabled-viewer');
      expect(viewer).toBeNull();
    });

    it('should handle lazy-load errors gracefully', async () => {
      const plugin: PluginDefinition = {
        id: 'error-viewer',
        name: 'Error Viewer',
        description: 'Plugin with failing lazy load',
        version: '1.0.0',
        config: { slashCommands: [] },
        viewer: {
          lazyLoad: () => Promise.reject(new Error('Failed to load')),
          priority: 1
        }
      };

      registry.register(plugin);

      const viewer = await registry.getViewer('error-viewer');

      // Verify the actual behavior: getViewer returns null on error
      expect(viewer).toBeNull();
      // Note: Logger warnings are silenced during tests by design (enabled: !isTest)
    });
  });

  describe('Reference Component Management', () => {
    it('should resolve reference component', () => {
      const plugin: PluginDefinition = {
        id: 'ref-test',
        name: 'Reference Test',
        description: 'Plugin with reference component',
        version: '1.0.0',
        config: { slashCommands: [] },
        reference: {
          component: MockReferenceComponent,
          priority: 1
        }
      };

      registry.register(plugin);
      const reference = registry.getReferenceComponent('ref-test');

      expect(reference).toBe(MockReferenceComponent);
    });

    it('should return null for missing reference component', () => {
      const reference = registry.getReferenceComponent('non-existent');
      expect(reference).toBeNull();
    });

    it('should return null for disabled plugin reference', () => {
      const plugin: PluginDefinition = {
        id: 'disabled-ref',
        name: 'Disabled Reference',
        description: 'Plugin that will be disabled',
        version: '1.0.0',
        config: { slashCommands: [] },
        reference: {
          component: MockReferenceComponent
        }
      };

      registry.register(plugin);
      registry.setEnabled('disabled-ref', false);

      const reference = registry.getReferenceComponent('disabled-ref');
      expect(reference).toBeNull();
    });

    it('should check if reference component exists', () => {
      const plugin: PluginDefinition = {
        id: 'ref-check',
        name: 'Reference Check',
        description: 'Plugin for checking reference existence',
        version: '1.0.0',
        config: { slashCommands: [] },
        reference: {
          component: MockReferenceComponent
        }
      };

      registry.register(plugin);

      expect(registry.hasReferenceComponent('ref-check')).toBe(true);
      expect(registry.hasReferenceComponent('non-existent')).toBe(false);
    });
  });

  describe('Slash Command Management', () => {
    beforeEach(() => {
      const plugin1: PluginDefinition = {
        id: 'cmd-plugin-1',
        name: 'Command Plugin 1',
        description: 'First command plugin',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'high-priority',
              name: 'High Priority',
              description: 'High priority command',
              contentTemplate: 'high',
              priority: 10
            },
            {
              id: 'medium-priority',
              name: 'Medium Priority',
              description: 'Medium priority command',
              contentTemplate: 'medium',
              priority: 5
            }
          ]
        }
      };

      const plugin2: PluginDefinition = {
        id: 'cmd-plugin-2',
        name: 'Command Plugin 2',
        description: 'Second command plugin',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'low-priority',
              name: 'Low Priority',
              description: 'Low priority command',
              contentTemplate: 'low',
              priority: 1
            }
          ]
        }
      };

      registry.register(plugin1);
      registry.register(plugin2);
    });

    it('should get all slash commands sorted by priority', () => {
      const commands = registry.getAllSlashCommands();

      expect(commands).toHaveLength(3);
      expect(commands[0].id).toBe('high-priority');
      expect(commands[1].id).toBe('medium-priority');
      expect(commands[2].id).toBe('low-priority');
    });

    it('should find slash command by ID', () => {
      const command = registry.findSlashCommand('medium-priority');

      expect(command).toBeDefined();
      expect(command?.name).toBe('Medium Priority');
      expect(command?.contentTemplate).toBe('medium');
    });

    it('should return null for non-existent command', () => {
      const command = registry.findSlashCommand('non-existent');
      expect(command).toBeNull();
    });

    it('should filter slash commands by query', () => {
      const filtered = registry.filterSlashCommands('priority');

      expect(filtered).toHaveLength(3);
      expect(filtered.every((cmd) => cmd.name.toLowerCase().includes('priority'))).toBe(true);
    });

    it('should filter commands case-insensitively', () => {
      const filtered = registry.filterSlashCommands('HIGH');

      expect(filtered).toHaveLength(1);
      expect(filtered[0].id).toBe('high-priority');
    });

    it('should return all commands for empty query', () => {
      const all = registry.getAllSlashCommands();
      const filtered = registry.filterSlashCommands('');

      expect(filtered).toEqual(all);
    });

    it('should exclude commands from disabled plugins', () => {
      registry.setEnabled('cmd-plugin-1', false);

      const commands = registry.getAllSlashCommands();

      expect(commands).toHaveLength(1);
      expect(commands[0].id).toBe('low-priority');
    });
  });

  describe('Registry Statistics', () => {
    it('should provide accurate statistics', () => {
      const plugin1: PluginDefinition = {
        id: 'stats-plugin-1',
        name: 'Stats Plugin 1',
        description: 'Plugin for stats testing',
        version: '1.0.0',
        config: {
          slashCommands: [
            { id: 'cmd1', name: 'Command 1', description: 'First', contentTemplate: 'c1' },
            { id: 'cmd2', name: 'Command 2', description: 'Second', contentTemplate: 'c2' }
          ]
        },
        viewer: { component: MockViewerComponent },
        reference: { component: MockReferenceComponent }
      };

      const plugin2: PluginDefinition = {
        id: 'stats-plugin-2',
        name: 'Stats Plugin 2',
        description: 'Second stats plugin',
        version: '1.0.0',
        config: {
          slashCommands: [
            { id: 'cmd3', name: 'Command 3', description: 'Third', contentTemplate: 'c3' }
          ]
        },
        reference: { component: MockReferenceComponent }
      };

      registry.register(plugin1);
      registry.register(plugin2);

      const stats = registry.getStats();

      expect(stats.pluginsCount).toBe(2);
      expect(stats.viewersCount).toBe(1);
      expect(stats.referencesCount).toBe(2);
      expect(stats.slashCommandsCount).toBe(3);
      expect(stats.plugins).toEqual(['stats-plugin-1', 'stats-plugin-2']);
    });

    it('should exclude disabled plugins from stats', () => {
      const plugin: PluginDefinition = {
        id: 'disabled-stats',
        name: 'Disabled Stats',
        description: 'Plugin that will be disabled',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'disabled-cmd',
              name: 'Disabled',
              description: 'Will not count',
              contentTemplate: 'disabled'
            }
          ]
        },
        viewer: { component: MockViewerComponent }
      };

      registry.register(plugin);
      registry.setEnabled('disabled-stats', false);

      const stats = registry.getStats();

      expect(stats.pluginsCount).toBe(1); // Total registered
      expect(stats.viewersCount).toBe(0); // But no enabled viewers
      expect(stats.slashCommandsCount).toBe(0); // No enabled commands
    });
  });

  describe('Registry Cleanup', () => {
    it('should clear all plugins and state', () => {
      const plugin: PluginDefinition = {
        id: 'clear-test',
        name: 'Clear Test',
        description: 'Plugin for clear testing',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'clear-cmd',
              name: 'Clear',
              description: 'Clear command',
              contentTemplate: 'clear'
            }
          ]
        },
        viewer: { component: MockViewerComponent },
        reference: { component: MockReferenceComponent }
      };

      registry.register(plugin);

      expect(registry.getAllPlugins()).toHaveLength(1);
      expect(registry.getAllSlashCommands()).toHaveLength(1);

      registry.clear();

      expect(registry.getAllPlugins()).toHaveLength(0);
      expect(registry.getAllSlashCommands()).toHaveLength(0);
      expect(registry.hasPlugin('clear-test')).toBe(false);
    });
  });

  describe('Edge Cases and Error Handling', () => {
    it('should handle plugin with no slash commands', () => {
      const plugin: PluginDefinition = {
        id: 'no-commands',
        name: 'No Commands',
        description: 'Plugin without slash commands',
        version: '1.0.0',
        config: {
          slashCommands: []
        },
        viewer: { component: MockViewerComponent }
      };

      registry.register(plugin);

      expect(registry.getAllSlashCommands()).toHaveLength(0);
      expect(registry.hasViewer('no-commands')).toBe(true);
    });

    it('should handle duplicate plugin registration', () => {
      const plugin: PluginDefinition = {
        id: 'duplicate',
        name: 'Duplicate',
        description: 'Duplicate plugin',
        version: '1.0.0',
        config: { slashCommands: [] }
      };

      registry.register(plugin);
      expect(registry.getAllPlugins()).toHaveLength(1);

      // Register again with different data
      const updatedPlugin: PluginDefinition = {
        ...plugin,
        name: 'Updated Duplicate',
        description: 'Updated duplicate plugin'
      };

      registry.register(updatedPlugin);

      // Should replace the previous registration
      expect(registry.getAllPlugins()).toHaveLength(1);
      expect(registry.getPlugin('duplicate')?.name).toBe('Updated Duplicate');
    });

    it('should handle missing viewer component gracefully', async () => {
      const plugin: PluginDefinition = {
        id: 'no-viewer',
        name: 'No Viewer',
        description: 'Plugin without viewer',
        version: '1.0.0',
        config: { slashCommands: [] }
      };

      registry.register(plugin);

      const viewer = await registry.getViewer('no-viewer');
      expect(viewer).toBeNull();
    });

    it('should handle malformed lazy load function', async () => {
      const plugin: PluginDefinition = {
        id: 'malformed-lazy',
        name: 'Malformed Lazy',
        description: 'Plugin with malformed lazy load',
        version: '1.0.0',
        config: { slashCommands: [] },
        viewer: {
          lazyLoad: () => Promise.resolve({} as { default: NodeViewerComponent }), // Missing 'default' property
          priority: 1
        }
      };

      registry.register(plugin);

      const viewer = await registry.getViewer('malformed-lazy');
      expect(viewer).toBeUndefined(); // Would be undefined due to missing default
    });
  });

  describe('canHaveChildren', () => {
    it('should return false for plugins with canHaveChildren: false', () => {
      const plugin: PluginDefinition = {
        id: 'code-block',
        name: 'Code Block',
        description: 'Code block node',
        version: '1.0.0',
        config: {
          slashCommands: [],
          canHaveChildren: false
        }
      };

      registry.register(plugin);

      expect(registry.canHaveChildren('code-block')).toBe(false);
    });

    it('should return true for plugins with canHaveChildren: true', () => {
      const plugin: PluginDefinition = {
        id: 'text',
        name: 'Text Node',
        description: 'Text node',
        version: '1.0.0',
        config: {
          slashCommands: [],
          canHaveChildren: true
        }
      };

      registry.register(plugin);

      expect(registry.canHaveChildren('text')).toBe(true);
    });

    it('should return true by default when canHaveChildren is not specified', () => {
      const plugin: PluginDefinition = {
        id: 'default-node',
        name: 'Default Node',
        description: 'Node without canHaveChildren specified',
        version: '1.0.0',
        config: {
          slashCommands: []
        }
      };

      registry.register(plugin);

      expect(registry.canHaveChildren('default-node')).toBe(true);
    });

    it('should return true for unknown/unregistered plugins', () => {
      expect(registry.canHaveChildren('unknown-plugin')).toBe(true);
    });

    it('should return true for disabled plugins', () => {
      const plugin: PluginDefinition = {
        id: 'disabled-plugin',
        name: 'Disabled Plugin',
        description: 'A disabled plugin',
        version: '1.0.0',
        config: {
          slashCommands: [],
          canHaveChildren: false
        }
      };

      registry.register(plugin);
      registry.setEnabled('disabled-plugin', false);

      expect(registry.canHaveChildren('disabled-plugin')).toBe(true);
    });
  });

  describe('Node Updater Management (Issue #709)', () => {
    it('should return updater for plugin with registered updater', () => {
      const plugin: PluginDefinition = {
        id: 'task',
        name: 'Task Node',
        description: 'Task node with type-specific updater',
        version: '1.0.0',
        config: { slashCommands: [] },
        updater: MockNodeUpdater
      };

      registry.register(plugin);

      const updater = registry.getNodeUpdater('task');
      expect(updater).toBe(MockNodeUpdater);
    });

    it('should return null for plugin without updater', () => {
      const plugin: PluginDefinition = {
        id: 'text',
        name: 'Text Node',
        description: 'Text node without type-specific updater',
        version: '1.0.0',
        config: { slashCommands: [] }
      };

      registry.register(plugin);

      const updater = registry.getNodeUpdater('text');
      expect(updater).toBeNull();
    });

    it('should return null for non-existent plugin', () => {
      const updater = registry.getNodeUpdater('non-existent');
      expect(updater).toBeNull();
    });

    it('should return null for disabled plugin updater', () => {
      const plugin: PluginDefinition = {
        id: 'disabled-task',
        name: 'Disabled Task',
        description: 'Task that will be disabled',
        version: '1.0.0',
        config: { slashCommands: [] },
        updater: MockNodeUpdater
      };

      registry.register(plugin);
      registry.setEnabled('disabled-task', false);

      const updater = registry.getNodeUpdater('disabled-task');
      expect(updater).toBeNull();
    });

    it('should cache updater lookups', () => {
      const plugin: PluginDefinition = {
        id: 'cached-task',
        name: 'Cached Task',
        description: 'Task for cache testing',
        version: '1.0.0',
        config: { slashCommands: [] },
        updater: MockNodeUpdater
      };

      registry.register(plugin);

      // First call
      const updater1 = registry.getNodeUpdater('cached-task');
      // Second call should use cache
      const updater2 = registry.getNodeUpdater('cached-task');

      expect(updater1).toBe(updater2);
      expect(updater1).toBe(MockNodeUpdater);
    });

    it('should use negative caching for plugins without updater', () => {
      const plugin: PluginDefinition = {
        id: 'no-updater',
        name: 'No Updater',
        description: 'Plugin without updater',
        version: '1.0.0',
        config: { slashCommands: [] }
      };

      registry.register(plugin);

      // First call
      const updater1 = registry.getNodeUpdater('no-updater');
      // Second call should use negative cache
      const updater2 = registry.getNodeUpdater('no-updater');

      expect(updater1).toBeNull();
      expect(updater2).toBeNull();
    });

    it('should check if updater exists with hasNodeUpdater', () => {
      const pluginWithUpdater: PluginDefinition = {
        id: 'with-updater',
        name: 'With Updater',
        description: 'Plugin with updater',
        version: '1.0.0',
        config: { slashCommands: [] },
        updater: MockNodeUpdater
      };

      const pluginWithoutUpdater: PluginDefinition = {
        id: 'without-updater',
        name: 'Without Updater',
        description: 'Plugin without updater',
        version: '1.0.0',
        config: { slashCommands: [] }
      };

      registry.register(pluginWithUpdater);
      registry.register(pluginWithoutUpdater);

      expect(registry.hasNodeUpdater('with-updater')).toBe(true);
      expect(registry.hasNodeUpdater('without-updater')).toBe(false);
      expect(registry.hasNodeUpdater('non-existent')).toBe(false);
    });

    it('should return false for disabled plugin in hasNodeUpdater', () => {
      const plugin: PluginDefinition = {
        id: 'disabled-has-updater',
        name: 'Disabled Has Updater',
        description: 'Plugin that will be disabled',
        version: '1.0.0',
        config: { slashCommands: [] },
        updater: MockNodeUpdater
      };

      registry.register(plugin);
      expect(registry.hasNodeUpdater('disabled-has-updater')).toBe(true);

      registry.setEnabled('disabled-has-updater', false);
      expect(registry.hasNodeUpdater('disabled-has-updater')).toBe(false);
    });

    it('should clear updater cache when plugin is disabled', () => {
      const plugin: PluginDefinition = {
        id: 'cache-clear-test',
        name: 'Cache Clear Test',
        description: 'Test cache clearing on disable',
        version: '1.0.0',
        config: { slashCommands: [] },
        updater: MockNodeUpdater
      };

      registry.register(plugin);

      // Populate cache
      const updater1 = registry.getNodeUpdater('cache-clear-test');
      expect(updater1).toBe(MockNodeUpdater);

      // Disable plugin (should clear cache)
      registry.setEnabled('cache-clear-test', false);

      // Now should return null
      const updater2 = registry.getNodeUpdater('cache-clear-test');
      expect(updater2).toBeNull();
    });
  });

  describe('Query Node Plugin', () => {
    it('should register query plugin correctly', () => {
      const plugin: PluginDefinition = {
        id: 'query',
        name: 'Query Node',
        description: 'Saved query definition',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'query',
              name: 'Query',
              description: 'Create a saved query',
              contentTemplate: '',
              nodeType: 'query'
            }
          ],
          canHaveChildren: false, // Query nodes are leaf nodes
          canBeChild: true
        },
        node: {
          lazyLoad: vi.fn().mockResolvedValue({ default: MockViewerComponent })
        },
        reference: {
          component: MockReferenceComponent,
          priority: 1
        }
      };

      registry.register(plugin);

      expect(registry.hasPlugin('query')).toBe(true);
      expect(registry.canHaveChildren('query')).toBe(false);
      expect(registry.getAllSlashCommands()).toHaveLength(1); // Has /query command
      expect(registry.getReferenceComponent('query')).toBe(MockReferenceComponent);

      const queryCommand = registry.getAllSlashCommands()[0];
      expect(queryCommand.id).toBe('query');
      expect(queryCommand.nodeType).toBe('query');
    });
  });
});
