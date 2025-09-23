/**
 * Core Plugins Integration Tests
 *
 * Tests the core plugin definitions and their integration with the unified
 * plugin registry system. Ensures all built-in node types work correctly.
 *
 * Tests follow the official NodeSpace testing guide patterns.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { PluginRegistry } from '$lib/plugins/pluginRegistry';
import {
  textNodePlugin,
  taskNodePlugin,
  aiChatNodePlugin,
  dateNodePlugin,
  userNodePlugin,
  documentNodePlugin,
  corePlugins,
  registerCorePlugins,
  registerExternalPlugin
} from '$lib/plugins/corePlugins';
import type { NodeViewerComponent, NodeReferenceComponent } from '$lib/plugins/types';

describe('Core Plugins Integration', () => {
  let registry: PluginRegistry;
  let consoleSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    registry = new PluginRegistry();
    consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
  });

  afterEach(() => {
    registry.clear();
    consoleSpy.mockRestore();
  });

  describe('Individual Core Plugin Definitions', () => {
    it('should have valid textNodePlugin definition', () => {
      expect(textNodePlugin.id).toBe('text');
      expect(textNodePlugin.name).toBe('Text Node');
      expect(textNodePlugin.version).toBe('1.0.0');
      expect(textNodePlugin.config.slashCommands).toHaveLength(4); // text, h1, h2, h3
      expect(textNodePlugin.viewer).toBeDefined();
      expect(textNodePlugin.reference).toBeDefined();

      // Check header commands are included from recent BasicNodeTypeRegistry work
      const commands = textNodePlugin.config.slashCommands;
      expect(commands.find((cmd) => cmd.id === 'header1')).toBeDefined();
      expect(commands.find((cmd) => cmd.id === 'header2')).toBeDefined();
      expect(commands.find((cmd) => cmd.id === 'header3')).toBeDefined();
      expect(commands.find((cmd) => cmd.shortcut === '#')).toBeDefined();
    });

    it('should have valid taskNodePlugin definition', () => {
      expect(taskNodePlugin.id).toBe('task');
      expect(taskNodePlugin.name).toBe('Task Node');
      expect(taskNodePlugin.config.slashCommands).toHaveLength(1);
      expect(taskNodePlugin.viewer).toBeDefined();
      expect(taskNodePlugin.reference).toBeDefined();

      const taskCommand = taskNodePlugin.config.slashCommands[0];
      expect(taskCommand.shortcut).toBe('[ ]');
      expect(taskCommand.contentTemplate).toBe('');
    });

    it('should have valid aiChatNodePlugin definition', () => {
      expect(aiChatNodePlugin.id).toBe('ai-chat');
      expect(aiChatNodePlugin.name).toBe('AI Chat Node');
      expect(aiChatNodePlugin.config.slashCommands).toHaveLength(1);
      expect(aiChatNodePlugin.viewer).toBeDefined();
      expect(aiChatNodePlugin.reference).toBeDefined();

      const chatCommand = aiChatNodePlugin.config.slashCommands[0];
      expect(chatCommand.shortcut).toBe('⌘ + k');
    });

    it('should have valid dateNodePlugin definition', () => {
      expect(dateNodePlugin.id).toBe('date');
      expect(dateNodePlugin.name).toBe('Date Node');
      expect(dateNodePlugin.config.slashCommands).toHaveLength(1);
      expect(dateNodePlugin.viewer).toBeDefined();
      expect(dateNodePlugin.reference).toBeDefined();

      const dateCommand = dateNodePlugin.config.slashCommands[0];
      expect(dateCommand.contentTemplate).toMatch(/^\d{4}-\d{2}-\d{2}$/); // YYYY-MM-DD format
    });

    it('should have valid reference-only plugins', () => {
      // User plugin - reference only
      expect(userNodePlugin.id).toBe('user');
      expect(userNodePlugin.name).toBe('User Reference');
      expect(userNodePlugin.config.slashCommands).toHaveLength(0);
      expect(userNodePlugin.viewer).toBeUndefined();
      expect(userNodePlugin.reference).toBeDefined();

      // Document plugin - reference only
      expect(documentNodePlugin.id).toBe('document');
      expect(documentNodePlugin.name).toBe('Document Reference');
      expect(documentNodePlugin.config.slashCommands).toHaveLength(0);
      expect(documentNodePlugin.viewer).toBeUndefined();
      expect(documentNodePlugin.reference).toBeDefined();
    });
  });

  describe('Core Plugins Collection', () => {
    it('should export all core plugins in corePlugins array', () => {
      expect(corePlugins).toHaveLength(6);
      expect(corePlugins).toContain(textNodePlugin);
      expect(corePlugins).toContain(taskNodePlugin);
      expect(corePlugins).toContain(aiChatNodePlugin);
      expect(corePlugins).toContain(dateNodePlugin);
      expect(corePlugins).toContain(userNodePlugin);
      expect(corePlugins).toContain(documentNodePlugin);
    });

    it('should have unique plugin IDs', () => {
      const ids = corePlugins.map((plugin) => plugin.id);
      const uniqueIds = new Set(ids);

      expect(ids.length).toBe(uniqueIds.size);
    });
  });

  describe('Core Plugin Registration', () => {
    it('should register all core plugins successfully', () => {
      registerCorePlugins(registry);

      expect(registry.getAllPlugins()).toHaveLength(6);

      // Verify each core plugin is registered
      for (const plugin of corePlugins) {
        expect(registry.hasPlugin(plugin.id)).toBe(true);
        expect(registry.isEnabled(plugin.id)).toBe(true);
      }
    });

    it('should log registration statistics', () => {
      registerCorePlugins(registry);

      expect(consoleSpy).toHaveBeenCalledWith(
        '[UnifiedPluginRegistry] Core plugins registered:',
        expect.objectContaining({
          plugins: 6,
          slashCommands: expect.any(Number),
          viewers: 4, // text, task, ai-chat, date
          references: 6 // all plugins have references
        })
      );
    });

    it('should provide correct slash command count', () => {
      registerCorePlugins(registry);

      const stats = registry.getStats();

      // text: 4 commands, task: 1, ai-chat: 1, date: 1, user: 0, document: 0 = 7 total
      expect(stats.slashCommandsCount).toBe(7);
    });

    it('should provide all slash commands with proper inheritance', () => {
      registerCorePlugins(registry);

      const commands = registry.getAllSlashCommands();

      expect(commands).toHaveLength(7);

      // Verify text node commands from BasicNodeTypeRegistry work
      const textCommands = commands.filter((cmd) =>
        ['text', 'header1', 'header2', 'header3'].includes(cmd.id)
      );
      expect(textCommands).toHaveLength(4);

      // Verify other core commands
      expect(commands.find((cmd) => cmd.id === 'task')).toBeDefined();
      expect(commands.find((cmd) => cmd.id === 'ai-chat')).toBeDefined();
      expect(commands.find((cmd) => cmd.id === 'date')).toBeDefined();
    });
  });

  describe('Plugin Component Resolution', () => {
    beforeEach(() => {
      registerCorePlugins(registry);
    });

    it('should resolve viewer components for plugins with viewers', async () => {
      // Test plugins with viewers
      const viewerPlugins = ['text', 'task', 'ai-chat', 'date'];

      for (const pluginId of viewerPlugins) {
        expect(registry.hasViewer(pluginId)).toBe(true);

        const viewer = await registry.getViewer(pluginId);
        expect(viewer).toBeDefined();
      }
    });

    it('should not resolve viewers for reference-only plugins', async () => {
      // Test plugins without viewers
      const referenceOnlyPlugins = ['user', 'document'];

      for (const pluginId of referenceOnlyPlugins) {
        expect(registry.hasViewer(pluginId)).toBe(false);

        const viewer = await registry.getViewer(pluginId);
        expect(viewer).toBeNull();
      }
    });

    it('should resolve reference components for all plugins', () => {
      for (const plugin of corePlugins) {
        expect(registry.hasReferenceComponent(plugin.id)).toBe(true);

        const reference = registry.getReferenceComponent(plugin.id);
        expect(reference).toBeDefined();
      }
    });
  });

  describe('Slash Command Integration', () => {
    beforeEach(() => {
      registerCorePlugins(registry);
    });

    it('should find all core slash commands', () => {
      const expectedCommands = [
        'text',
        'header1',
        'header2',
        'header3', // text plugin
        'task', // task plugin
        'ai-chat', // ai-chat plugin
        'date' // date plugin
      ];

      for (const commandId of expectedCommands) {
        const command = registry.findSlashCommand(commandId);
        expect(command).toBeDefined();
        expect(command?.id).toBe(commandId);
      }
    });

    it('should filter commands correctly', () => {
      // Test header command filtering
      const headerCommands = registry.filterSlashCommands('header');
      expect(headerCommands).toHaveLength(3);
      expect(headerCommands.every((cmd) => cmd.name.includes('Header'))).toBe(true);

      // Test task command filtering
      const taskCommands = registry.filterSlashCommands('task');
      expect(taskCommands).toHaveLength(1);
      expect(taskCommands[0].id).toBe('task');
    });

    it('should provide commands with proper shortcuts', () => {
      const headerCommands = registry.filterSlashCommands('header');

      expect(headerCommands.find((cmd) => cmd.shortcut === '#')).toBeDefined();
      expect(headerCommands.find((cmd) => cmd.shortcut === '##')).toBeDefined();
      expect(headerCommands.find((cmd) => cmd.shortcut === '###')).toBeDefined();

      const taskCommand = registry.findSlashCommand('task');
      expect(taskCommand?.shortcut).toBe('[ ]');

      const chatCommand = registry.findSlashCommand('ai-chat');
      expect(chatCommand?.shortcut).toBe('⌘ + k');
    });
  });

  describe('External Plugin Registration', () => {
    it('should register external plugin successfully', () => {
      const externalPlugin = {
        id: 'whiteboard',
        name: 'WhiteBoard Node',
        description: 'Interactive whiteboard with drawing tools',
        version: '1.0.0',
        config: {
          slashCommands: [
            {
              id: 'whiteboard',
              name: 'WhiteBoard',
              description: 'Create an interactive whiteboard',
              contentTemplate: ''
            }
          ],
          canHaveChildren: true,
          canBeChild: true
        },
        viewer: {
          lazyLoad: () => Promise.resolve({ default: vi.fn() as unknown as NodeViewerComponent })
        },
        reference: {
          component: vi.fn() as unknown as NodeReferenceComponent
        }
      };

      registerExternalPlugin(registry, externalPlugin);

      expect(registry.hasPlugin('whiteboard')).toBe(true);
      expect(consoleSpy).toHaveBeenCalledWith(
        '[UnifiedPluginRegistry] External plugin registered: WhiteBoard Node (whiteboard)'
      );
    });

    it('should work alongside core plugins', () => {
      registerCorePlugins(registry);

      const initialCount = registry.getAllPlugins().length;

      const externalPlugin = {
        id: 'custom-node',
        name: 'Custom Node',
        description: 'Custom external node type',
        version: '2.0.0',
        config: {
          slashCommands: [
            {
              id: 'custom',
              name: 'Custom',
              description: 'Create custom node',
              contentTemplate: 'custom:'
            }
          ]
        }
      };

      registerExternalPlugin(registry, externalPlugin);

      expect(registry.getAllPlugins()).toHaveLength(initialCount + 1);
      expect(registry.getAllSlashCommands()).toHaveLength(8); // 7 core + 1 external
    });
  });

  describe('Plugin State Management', () => {
    beforeEach(() => {
      registerCorePlugins(registry);
    });

    it('should disable individual core plugins', () => {
      expect(registry.isEnabled('text')).toBe(true);

      registry.setEnabled('text', false);

      expect(registry.isEnabled('text')).toBe(false);

      // Should reduce available commands
      const commands = registry.getAllSlashCommands();
      expect(commands.find((cmd) => cmd.id === 'text')).toBeUndefined();
      expect(commands.find((cmd) => cmd.id === 'header1')).toBeUndefined();
    });

    it('should re-enable disabled plugins', () => {
      registry.setEnabled('task', false);
      expect(registry.findSlashCommand('task')).toBeNull();

      registry.setEnabled('task', true);
      expect(registry.findSlashCommand('task')).toBeDefined();
    });

    it('should handle registry clearing', () => {
      expect(registry.getAllPlugins()).toHaveLength(6);

      registry.clear();

      expect(registry.getAllPlugins()).toHaveLength(0);
      expect(registry.getAllSlashCommands()).toHaveLength(0);
    });
  });

  describe('Backward Compatibility', () => {
    it('should maintain all functionality from old BasicNodeTypeRegistry', () => {
      registerCorePlugins(registry);

      // Verify all original BasicNodeTypeRegistry node types are present
      const expectedNodeTypes = ['text', 'task', 'ai-chat'];

      for (const nodeType of expectedNodeTypes) {
        expect(registry.hasPlugin(nodeType)).toBe(true);
        expect(registry.findSlashCommand(nodeType)).toBeDefined();
      }

      // Verify header commands from BasicNodeTypeRegistry are present
      const headerCommands = ['header1', 'header2', 'header3'];

      for (const headerCmd of headerCommands) {
        expect(registry.findSlashCommand(headerCmd)).toBeDefined();
      }
    });

    it('should maintain all functionality from old ViewerRegistry', () => {
      registerCorePlugins(registry);

      // Verify all original ViewerRegistry types have viewers
      const expectedViewerTypes = ['text', 'date', 'task', 'ai-chat'];

      for (const viewerType of expectedViewerTypes) {
        expect(registry.hasViewer(viewerType)).toBe(true);
      }
    });

    it('should maintain all functionality from old NODE_REFERENCE_COMPONENTS', () => {
      registerCorePlugins(registry);

      // Verify all original NODE_REFERENCE_COMPONENTS types have references
      const expectedReferenceTypes = ['text', 'task', 'user', 'date', 'document', 'ai-chat'];

      for (const referenceType of expectedReferenceTypes) {
        expect(registry.hasReferenceComponent(referenceType)).toBe(true);
      }
    });
  });
});
