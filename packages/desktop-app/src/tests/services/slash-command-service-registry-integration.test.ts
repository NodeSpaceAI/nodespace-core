/**
 * SlashCommandService Registry Integration Tests
 *
 * Tests the integration between SlashCommandService and the unified
 * plugin registry system to ensure both systems work together correctly.
 */

import { expect, describe, it, beforeEach } from 'vitest';
import { SlashCommandService } from '../../lib/services/slash-command-service.js';
import { pluginRegistry } from '$lib/plugins/index';
import { registerCorePlugins } from '$lib/plugins/core-plugins';

describe('SlashCommandService Registry Integration', () => {
  let service: SlashCommandService;

  beforeEach(() => {
    service = SlashCommandService.getInstance();
    // Initialize the unified plugin registry with core plugins
    pluginRegistry.clear(); // Clear previous state
    registerCorePlugins(pluginRegistry);
  });

  describe('Basic Functionality (Registry Enabled)', () => {
    it('should return registry commands when registry integration enabled', () => {
      const commands = service.getCommands();

      expect(commands).toHaveLength(8); // 8 core plugin commands (text, header1-3, task, ai-chat, code, quote)

      // Commands are sorted by priority (higher first), then by name
      const commandIds = commands.map((cmd) => cmd.id);
      expect(commandIds).toContain('text');
      expect(commandIds).toContain('header1');
      expect(commandIds).toContain('header2');
      expect(commandIds).toContain('header3');
      expect(commandIds).toContain('task');
      expect(commandIds).toContain('ai-chat');
      expect(commandIds).toContain('code');
    });

    it('should filter registry commands correctly', () => {
      const filtered = service.filterCommands('header');

      expect(filtered).toHaveLength(3);
      expect(filtered.every((cmd) => cmd.name.toLowerCase().includes('header'))).toBe(true);
    });

    it('should find registry commands by ID', () => {
      const command = service.findCommand('task');

      expect(command).toBeDefined();
      expect(command?.id).toBe('task');
      expect(command?.nodeType).toBe('task');
    });
  });

  describe('Registry Integration (Enabled)', () => {
    it('should include registry commands when integration enabled', () => {
      const commands = service.getCommands();

      // Should have at least the hardcoded commands
      expect(commands.length).toBeGreaterThanOrEqual(6);

      // Plugin registry should be initialized with plugins
      expect(pluginRegistry.getAllPlugins().length).toBeGreaterThan(0);

      // Should contain both hardcoded and potentially registry commands
      const commandIds = commands.map((cmd) => cmd.id);
      expect(commandIds).toContain('text');
      expect(commandIds).toContain('task');
      expect(commandIds).toContain('ai-chat');
    });

    it('should avoid duplicate commands when merging registry', () => {
      const commands = service.getCommands();
      const commandIds = commands.map((cmd) => cmd.id);
      const uniqueIds = new Set(commandIds);

      // No duplicate IDs
      expect(commandIds.length).toBe(uniqueIds.size);
    });

    it('should filter commands from both sources', () => {
      const filtered = service.filterCommands('text');

      expect(filtered.length).toBeGreaterThan(0);
      expect(filtered.some((cmd) => cmd.name.toLowerCase().includes('text'))).toBe(true);
    });

    it('should find commands from both hardcoded and registry sources', () => {
      // Test finding a hardcoded command
      const hardcodedCommand = service.findCommand('task');
      expect(hardcodedCommand).toBeDefined();

      // Test finding registry command (if it exists and is different)
      const textCommand = service.findCommand('text');
      expect(textCommand).toBeDefined();
    });
  });

  describe('Command Execution', () => {
    it('should execute registry commands correctly', () => {
      const taskCommand = service.findCommand('task')!;
      const result = service.executeCommand(taskCommand);

      expect(result).toEqual({
        content: '', // Task plugin has empty content template
        nodeType: 'task',
        headerLevel: undefined
      });
    });

    it('should execute registry commands when available', () => {
      // Find a command (prefer registry if available, fallback to hardcoded)
      const textCommand = service.findCommand('text')!;
      const result = service.executeCommand(textCommand);

      expect(result).toBeDefined();
      expect(result.nodeType).toBe('text');
      expect(typeof result.content).toBe('string');
    });

    it('should handle header commands with proper levels', () => {
      const headerCommand = service.findCommand('header1')!;
      const result = service.executeCommand(headerCommand);

      expect(result).toEqual({
        content: '# ',
        nodeType: 'text',
        headerLevel: 1
      });
    });
  });

  describe('Registry State', () => {
    it('should properly check registry state', () => {
      expect(pluginRegistry.getAllPlugins().length).toBeGreaterThan(0);

      const stats = pluginRegistry.getStats();
      expect(stats.pluginsCount).toBeGreaterThan(0);
      expect(stats.slashCommandsCount).toBeGreaterThan(0);
      expect(Array.isArray(stats.plugins)).toBe(true);
    });

    it('should provide registry commands', () => {
      const registryCommands = pluginRegistry.getAllSlashCommands();

      expect(Array.isArray(registryCommands)).toBe(true);
      expect(registryCommands.length).toBeGreaterThan(0);

      // Each command should have required properties
      registryCommands.forEach((cmd) => {
        expect(cmd).toHaveProperty('id');
        expect(cmd).toHaveProperty('name');
        expect(cmd).toHaveProperty('description');
        expect(cmd).toHaveProperty('contentTemplate');
      });
    });
  });

  describe('Backwards Compatibility', () => {
    it('should maintain registry-based behavior with all core commands', () => {
      const commands = service.getCommands();
      const filtered = service.filterCommands('task');
      const found = service.findCommand('task');

      // Registry-based behavior includes all core commands
      expect(commands).toHaveLength(8); // All 8 core slash commands (text, header1-3, task, ai-chat, code, quote)
      expect(filtered.length).toBe(1);
      expect(found?.id).toBe('task');
    });

    it('should find all hardcoded commands correctly', () => {
      // Test all hardcoded commands can be found
      const testCases = [
        { id: 'text', nodeType: 'text', headerLevel: 0 },
        { id: 'header1', nodeType: 'text', headerLevel: 1 },
        { id: 'header2', nodeType: 'text', headerLevel: 2 },
        { id: 'header3', nodeType: 'text', headerLevel: 3 },
        { id: 'task', nodeType: 'task' },
        { id: 'ai-chat', nodeType: 'ai-chat' }
      ];

      testCases.forEach(({ id, nodeType, headerLevel }) => {
        const command = service.findCommand(id)!;
        expect(command).toBeDefined();
        expect(command.id).toBe(id);
        expect(command.nodeType).toBe(nodeType);
        if (headerLevel !== undefined) {
          expect(command.headerLevel).toBe(headerLevel);
        }
      });
    });
  });
});
