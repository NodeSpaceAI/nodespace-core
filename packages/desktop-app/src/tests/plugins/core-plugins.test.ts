/**
 * Core Plugins Integration Tests
 *
 * Tests the core plugin definitions and their integration with the unified
 * plugin registry system. Ensures all built-in node types work correctly.
 *
 * Tests follow the official NodeSpace testing guide patterns.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { PluginRegistry } from '$lib/plugins/plugin-registry';
import {
  textNodePlugin,
  headerNodePlugin,
  taskNodePlugin,
  aiChatNodePlugin,
  dateNodePlugin,
  userNodePlugin,
  documentNodePlugin,
  corePlugins,
  registerCorePlugins,
  registerExternalPlugin
} from '$lib/plugins/core-plugins';
import type { NodeViewerComponent, NodeReferenceComponent } from '$lib/plugins/types';

describe('Core Plugins Integration', () => {
  let registry: PluginRegistry;

  beforeEach(() => {
    registry = new PluginRegistry();
  });

  afterEach(() => {
    registry.clear();
  });

  describe('Individual Core Plugin Definitions', () => {
    it('should have valid textNodePlugin definition', () => {
      expect(textNodePlugin.id).toBe('text');
      expect(textNodePlugin.name).toBe('Text Node');
      expect(textNodePlugin.version).toBe('1.0.0');
      expect(textNodePlugin.config.slashCommands).toHaveLength(1); // just text command
      // Text nodes use BaseNodeViewer (default) - no custom viewer needed
      expect(textNodePlugin.viewer).toBeUndefined();
      expect(textNodePlugin.node).toBeDefined(); // Has node component instead
      expect(textNodePlugin.reference).toBeDefined();

      const textCommand = textNodePlugin.config.slashCommands[0];
      expect(textCommand.id).toBe('text');
    });

    it('should have valid headerNodePlugin definition', () => {
      expect(headerNodePlugin.id).toBe('header');
      expect(headerNodePlugin.name).toBe('Header Node');
      expect(headerNodePlugin.version).toBe('1.0.0');
      expect(headerNodePlugin.config.slashCommands).toHaveLength(3); // h1, h2, h3
      expect(headerNodePlugin.node).toBeDefined(); // Header uses 'node' not 'viewer'
      expect(headerNodePlugin.reference).toBeDefined();

      // Check header commands
      const commands = headerNodePlugin.config.slashCommands;
      expect(commands.find((cmd) => cmd.id === 'header1')).toBeDefined();
      expect(commands.find((cmd) => cmd.id === 'header2')).toBeDefined();
      expect(commands.find((cmd) => cmd.id === 'header3')).toBeDefined();
      expect(commands.find((cmd) => cmd.shortcut === '#')).toBeDefined();
    });

    it('should have valid taskNodePlugin definition', () => {
      expect(taskNodePlugin.id).toBe('task');
      expect(taskNodePlugin.name).toBe('Task Node');
      expect(taskNodePlugin.config.slashCommands).toHaveLength(1);
      // Task nodes have TaskNodeViewer for task-specific UI (Issue #715)
      expect(taskNodePlugin.viewer).toBeDefined();
      expect(taskNodePlugin.node).toBeDefined(); // Has node component instead
      expect(taskNodePlugin.reference).toBeDefined();

      const taskCommand = taskNodePlugin.config.slashCommands[0];
      expect(taskCommand.shortcut).toBe('[ ]');
      expect(taskCommand.contentTemplate).toBe('');
    });

    it('should have valid aiChatNodePlugin definition', () => {
      expect(aiChatNodePlugin.id).toBe('ai-chat');
      expect(aiChatNodePlugin.name).toBe('AI Chat Node');
      expect(aiChatNodePlugin.config.slashCommands).toHaveLength(1);
      // AI chat nodes use BaseNodeViewer (default) - no custom viewer needed yet
      expect(aiChatNodePlugin.viewer).toBeUndefined();
      // No node component yet either - will be created in future
      expect(aiChatNodePlugin.node).toBeUndefined();
      expect(aiChatNodePlugin.reference).toBeDefined();

      const chatCommand = aiChatNodePlugin.config.slashCommands[0];
      expect(chatCommand.shortcut).toBe('⌘ + k');
    });

    it('should have valid dateNodePlugin definition', () => {
      expect(dateNodePlugin.id).toBe('date');
      expect(dateNodePlugin.name).toBe('Date Node');
      expect(dateNodePlugin.config.slashCommands).toHaveLength(0);
      expect(dateNodePlugin.node).toBeDefined(); // DateNode is a node component, not a viewer
      expect(dateNodePlugin.reference).toBeDefined();
      // Date nodes do not have slash commands - they exist implicitly for all dates
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
      expect(corePlugins).toHaveLength(9); // text, header, task, ai-chat, date, code-block, quote-block, ordered-list, query
      expect(corePlugins).toContain(textNodePlugin);
      expect(corePlugins).toContain(headerNodePlugin);
      expect(corePlugins).toContain(taskNodePlugin);
      expect(corePlugins).toContain(aiChatNodePlugin);
      expect(corePlugins).toContain(dateNodePlugin);
      // Note: userNodePlugin and documentNodePlugin are defined but not yet registered
      // They will be added when the user/document reference system is implemented
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

      expect(registry.getAllPlugins()).toHaveLength(9); // text, header, task, ai-chat, date, code-block, quote-block, ordered-list, query

      // Verify each core plugin is registered
      for (const plugin of corePlugins) {
        expect(registry.hasPlugin(plugin.id)).toBe(true);
        expect(registry.isEnabled(plugin.id)).toBe(true);
      }
    });

    it('should register with correct statistics', () => {
      registerCorePlugins(registry);

      // Verify registration statistics through the registry API
      // Note: Logger output is intentionally silenced during tests
      const stats = registry.getStats();
      expect(stats.pluginsCount).toBe(9); // text, header, task, ai-chat, date, code-block, quote-block, ordered-list, query
      expect(stats.slashCommandsCount).toBe(10); // text: 1, header: 3, task: 1, ai-chat: 1, code-block: 1, quote-block: 1, ordered-list: 1, query: 1
      expect(stats.viewersCount).toBe(2); // date and task have custom viewers (TaskNodeViewer added in Issue #715)
      expect(stats.referencesCount).toBe(9); // all plugins have references
    });

    it('should provide correct slash command count', () => {
      registerCorePlugins(registry);

      const stats = registry.getStats();

      // text: 1, header: 3, task: 1, ai-chat: 1, code-block: 1, quote-block: 1, ordered-list: 1, query: 1, date: 0 = 10 total
      expect(stats.slashCommandsCount).toBe(10);
    });

    it('should provide all slash commands with proper inheritance', () => {
      registerCorePlugins(registry);

      const commands = registry.getAllSlashCommands();

      expect(commands).toHaveLength(10); // text, header1-3, task, ai-chat, code, quote, ordered-list, query

      // Verify text node commands from BasicNodeTypeRegistry work
      const textCommands = commands.filter((cmd) =>
        ['text', 'header1', 'header2', 'header3'].includes(cmd.id)
      );
      expect(textCommands).toHaveLength(4);

      // Verify other core commands
      expect(commands.find((cmd) => cmd.id === 'task')).toBeDefined();
      expect(commands.find((cmd) => cmd.id === 'ai-chat')).toBeDefined();
      // Date nodes do not have slash commands - they are created through other mechanisms
    });
  });

  describe('Plugin Component Resolution', () => {
    beforeEach(() => {
      registerCorePlugins(registry);
    });

    it('should resolve viewer components for plugins with viewers', async () => {
      // date and task have custom viewers (TaskNodeViewer added in Issue #715)
      const viewerPlugins = ['date', 'task'];

      for (const pluginId of viewerPlugins) {
        expect(registry.hasViewer(pluginId)).toBe(true);

        const viewer = await registry.getViewer(pluginId);
        expect(viewer).toBeDefined();
      }

      // These plugins intentionally have no custom viewer - they use BaseNodeViewer
      const noViewerPlugins = ['text', 'ai-chat'];
      for (const pluginId of noViewerPlugins) {
        expect(registry.hasViewer(pluginId)).toBe(false);

        const viewer = await registry.getViewer(pluginId);
        expect(viewer).toBeNull();
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
        'ai-chat' // ai-chat plugin
        // date plugin has no slash commands
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

      // Verify the plugin was registered successfully
      // Note: Logger output is intentionally silenced during tests
      expect(registry.hasPlugin('whiteboard')).toBe(true);
      expect(registry.isEnabled('whiteboard')).toBe(true);

      // Verify slash command is available
      const command = registry.findSlashCommand('whiteboard');
      expect(command).toBeDefined();
      expect(command?.name).toBe('WhiteBoard');
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
      expect(registry.getAllSlashCommands()).toHaveLength(11); // 10 core + 1 external
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

      // Header commands should still be available (separate plugin)
      expect(commands.find((cmd) => cmd.id === 'header1')).toBeDefined();
    });

    it('should re-enable disabled plugins', () => {
      registry.setEnabled('task', false);
      expect(registry.findSlashCommand('task')).toBeNull();

      registry.setEnabled('task', true);
      expect(registry.findSlashCommand('task')).toBeDefined();
    });

    it('should handle registry clearing', () => {
      expect(registry.getAllPlugins()).toHaveLength(9); // text, header, task, ai-chat, date, code-block, quote-block, ordered-list, query

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

      // Architecture changed: text, task, ai-chat now use BaseNodeViewer (default)
      // date and task have custom viewers (TaskNodeViewer added in Issue #715)
      const customViewerTypes = ['date', 'task'];

      for (const viewerType of customViewerTypes) {
        expect(registry.hasViewer(viewerType)).toBe(true);
      }

      // These types use BaseNodeViewer fallback (no custom viewer registered)
      const baseViewerTypes = ['text', 'ai-chat'];
      for (const viewerType of baseViewerTypes) {
        expect(registry.hasViewer(viewerType)).toBe(false);
      }
    });

    it('should maintain all functionality from old NODE_REFERENCE_COMPONENTS', () => {
      registerCorePlugins(registry);

      // Verify currently implemented reference types have references
      // Note: 'user' and 'document' are not yet implemented - will be added when reference system is built
      const expectedReferenceTypes = ['text', 'task', 'date', 'ai-chat', 'header', 'code-block', 'quote-block', 'ordered-list'];

      for (const referenceType of expectedReferenceTypes) {
        expect(registry.hasReferenceComponent(referenceType)).toBe(true);
      }
    });
  });
});
