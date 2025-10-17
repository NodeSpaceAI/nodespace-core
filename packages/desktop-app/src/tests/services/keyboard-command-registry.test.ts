/**
 * Tests for KeyboardCommandRegistry
 *
 * Validates command registration, lookup, execution, and context building
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  KeyboardCommandRegistry,
  type KeyboardCommand,
  type KeyboardContext
} from '$lib/services/keyboard-command-registry';
import type { TextareaController } from '$lib/design/components/textarea-controller';

describe('KeyboardCommandRegistry', () => {
  let registry: KeyboardCommandRegistry;

  beforeEach(() => {
    registry = KeyboardCommandRegistry.getInstance();
    registry.clearAll();
  });

  describe('Singleton Pattern', () => {
    it('should return the same instance', () => {
      const instance1 = KeyboardCommandRegistry.getInstance();
      const instance2 = KeyboardCommandRegistry.getInstance();

      expect(instance1).toBe(instance2);
    });
  });

  describe('Command Registration', () => {
    it('should register a command for a simple key', () => {
      const mockCommand: KeyboardCommand = {
        id: 'test-enter',
        description: 'Test Enter command',
        canExecute: () => true,
        execute: async () => true
      };

      registry.register({ key: 'Enter' }, mockCommand);

      const commands = registry.getCommands();
      expect(commands.size).toBe(1);
      expect(commands.get('Enter')).toBe(mockCommand);
    });

    it('should register a command for a key with modifiers', () => {
      const mockCommand: KeyboardCommand = {
        id: 'test-ctrl-b',
        description: 'Test Ctrl+B command',
        canExecute: () => true,
        execute: async () => true
      };

      registry.register({ key: 'b', ctrl: true }, mockCommand);

      const commands = registry.getCommands();
      expect(commands.size).toBe(1);
      expect(commands.get('Ctrl+b')).toBe(mockCommand);
    });

    it('should register multiple modifiers in correct order', () => {
      const mockCommand: KeyboardCommand = {
        id: 'test-complex',
        description: 'Test complex command',
        canExecute: () => true,
        execute: async () => true
      };

      registry.register({ key: 'A', ctrl: true, shift: true, alt: true, meta: true }, mockCommand);

      const commands = registry.getCommands();
      // Order should be: Ctrl+Alt+Shift+Meta+Key
      expect(commands.get('Ctrl+Alt+Shift+Meta+A')).toBe(mockCommand);
    });

    it('should be idempotent when registering the same key combination multiple times', () => {
      const command1: KeyboardCommand = {
        id: 'test-1',
        description: 'First command',
        canExecute: () => true,
        execute: async () => true
      };

      const command2: KeyboardCommand = {
        id: 'test-2',
        description: 'Second command',
        canExecute: () => true,
        execute: async () => true
      };

      // Register first command
      registry.register({ key: 'Enter' }, command1);

      // Attempt to register second command with same key - should be silently skipped
      registry.register({ key: 'Enter' }, command2);

      // Verify the first command is still registered (idempotent behavior)
      const commands = registry.getCommands();
      const result = commands.get('Enter');
      expect(result).toBe(command1);
      expect(result?.id).toBe('test-1');
    });
  });

  describe('Command Unregistration', () => {
    it('should unregister a command', () => {
      const mockCommand: KeyboardCommand = {
        id: 'test-tab',
        description: 'Test Tab command',
        canExecute: () => true,
        execute: async () => true
      };

      registry.register({ key: 'Tab' }, mockCommand);
      expect(registry.getCommands().size).toBe(1);

      registry.unregister({ key: 'Tab' });
      expect(registry.getCommands().size).toBe(0);
    });

    it('should unregister a command with modifiers', () => {
      const mockCommand: KeyboardCommand = {
        id: 'test-shift-tab',
        description: 'Test Shift+Tab command',
        canExecute: () => true,
        execute: async () => true
      };

      registry.register({ key: 'Tab', shift: true }, mockCommand);
      expect(registry.getCommands().size).toBe(1);

      registry.unregister({ key: 'Tab', shift: true });
      expect(registry.getCommands().size).toBe(0);
    });
  });

  describe('Command Execution', () => {
    it('should execute a registered command when canExecute returns true', async () => {
      const executeSpy = vi.fn(async () => true);

      const mockCommand: KeyboardCommand = {
        id: 'test-enter',
        description: 'Test Enter command',
        canExecute: () => true,
        execute: executeSpy
      };

      registry.register({ key: 'Enter' }, mockCommand);

      const mockEvent = new KeyboardEvent('keydown', { key: 'Enter' });
      const mockController = {} as TextareaController;

      const result = await registry.execute(mockEvent, mockController);

      expect(result).toBe(true);
      expect(executeSpy).toHaveBeenCalledTimes(1);
    });

    it('should not execute a command when canExecute returns false', async () => {
      const executeSpy = vi.fn(async () => true);

      const mockCommand: KeyboardCommand = {
        id: 'test-enter',
        description: 'Test Enter command',
        canExecute: () => false,
        execute: executeSpy
      };

      registry.register({ key: 'Enter' }, mockCommand);

      const mockEvent = new KeyboardEvent('keydown', { key: 'Enter' });
      const mockController = {} as TextareaController;

      const result = await registry.execute(mockEvent, mockController);

      expect(result).toBe(false);
      expect(executeSpy).not.toHaveBeenCalled();
    });

    it('should return false when no command is registered', async () => {
      const mockEvent = new KeyboardEvent('keydown', { key: 'Enter' });
      const mockController = {} as TextareaController;

      const result = await registry.execute(mockEvent, mockController);

      expect(result).toBe(false);
    });

    it('should match commands with modifiers correctly', async () => {
      const executeSpy = vi.fn(async () => true);

      const mockCommand: KeyboardCommand = {
        id: 'test-ctrl-b',
        description: 'Test Ctrl+B command',
        canExecute: () => true,
        execute: executeSpy
      };

      registry.register({ key: 'b', ctrl: true }, mockCommand);

      // Event without Ctrl - should not execute
      const event1 = new KeyboardEvent('keydown', { key: 'b' });
      const mockController = {} as TextareaController;

      const result1 = await registry.execute(event1, mockController);
      expect(result1).toBe(false);
      expect(executeSpy).not.toHaveBeenCalled();

      // Event with Ctrl - should execute
      const event2 = new KeyboardEvent('keydown', { key: 'b', ctrlKey: true });
      const result2 = await registry.execute(event2, mockController);
      expect(result2).toBe(true);
      expect(executeSpy).toHaveBeenCalledTimes(1);
    });

    it('should handle command execution errors gracefully', async () => {
      const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

      const mockCommand: KeyboardCommand = {
        id: 'test-error',
        description: 'Test error command',
        canExecute: () => true,
        execute: async () => {
          throw new Error('Command failed');
        }
      };

      registry.register({ key: 'Enter' }, mockCommand);

      const mockEvent = new KeyboardEvent('keydown', { key: 'Enter' });
      const mockController = {} as TextareaController;

      const result = await registry.execute(mockEvent, mockController);

      expect(result).toBe(false);
      expect(consoleSpy).toHaveBeenCalledWith(
        expect.stringContaining('Error executing command'),
        expect.any(Error)
      );

      consoleSpy.mockRestore();
    });
  });

  describe('Context Building', () => {
    it('should build context with event and controller', async () => {
      let capturedContext: KeyboardContext | null = null as KeyboardContext | null;

      const mockCommand: KeyboardCommand = {
        id: 'test-context',
        description: 'Test context building',
        canExecute: () => true,
        execute: async (context) => {
          capturedContext = context;
          return true;
        }
      };

      registry.register({ key: 'Enter' }, mockCommand);

      const mockEvent = new KeyboardEvent('keydown', { key: 'Enter' });
      const mockController = {} as TextareaController;

      await registry.execute(mockEvent, mockController, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'test content',
        cursorPosition: 5,
        allowMultiline: true
      });

      expect(capturedContext).not.toBeNull();
      expect(capturedContext?.event).toBe(mockEvent);
      expect(capturedContext?.controller).toBe(mockController);
      expect(capturedContext?.nodeId).toBe('test-node');
      expect(capturedContext?.nodeType).toBe('text');
      expect(capturedContext?.content).toBe('test content');
      expect(capturedContext?.cursorPosition).toBe(5);
      expect(capturedContext?.allowMultiline).toBe(true);
    });

    it('should provide default values for missing context', async () => {
      let capturedContext: KeyboardContext | null = null as KeyboardContext | null;

      const mockCommand: KeyboardCommand = {
        id: 'test-defaults',
        description: 'Test default context',
        canExecute: () => true,
        execute: async (context) => {
          capturedContext = context;
          return true;
        }
      };

      registry.register({ key: 'Enter' }, mockCommand);

      const mockEvent = new KeyboardEvent('keydown', { key: 'Enter' });
      const mockController = {} as TextareaController;

      await registry.execute(mockEvent, mockController);

      expect(capturedContext).not.toBeNull();
      expect(capturedContext?.nodeId).toBe('');
      expect(capturedContext?.nodeType).toBe('text');
      expect(capturedContext?.content).toBe('');
      expect(capturedContext?.cursorPosition).toBe(0);
      expect(capturedContext?.allowMultiline).toBe(false);
      expect(capturedContext?.metadata).toEqual({});
    });
  });

  describe('ClearAll', () => {
    it('should clear all registered commands', () => {
      const mockCommand1: KeyboardCommand = {
        id: 'test-1',
        description: 'Test 1',
        canExecute: () => true,
        execute: async () => true
      };

      const mockCommand2: KeyboardCommand = {
        id: 'test-2',
        description: 'Test 2',
        canExecute: () => true,
        execute: async () => true
      };

      registry.register({ key: 'Enter' }, mockCommand1);
      registry.register({ key: 'Tab' }, mockCommand2);

      expect(registry.getCommands().size).toBe(2);

      registry.clearAll();

      expect(registry.getCommands().size).toBe(0);
    });
  });
});
