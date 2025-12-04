/**
 * Keyboard Command Registry - Centralized keyboard event handling using Command Pattern
 *
 * Follows the SlashCommandService pattern for consistency and extensibility.
 * Provides a singleton registry for keyboard commands that can be extended by plugins.
 */

import type { TextareaController } from '$lib/design/components/textarea-controller';
import { DEFAULT_PANE_ID } from '$lib/stores/navigation';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('KeyboardCommandRegistry');

/**
 * Context provided to keyboard commands during execution
 * Contains all information needed to execute keyboard operations
 */
export interface KeyboardContext {
  /** The original keyboard event */
  event: KeyboardEvent;

  /** Reference to the TextareaController */
  controller: TextareaController;

  /** Current node ID */
  nodeId: string;

  /** Current node type */
  nodeType: string;

  /** Pane ID this node belongs to (for multi-pane support) */
  paneId: string;

  /** Current text content */
  content: string;

  /** Current cursor position (character offset) */
  cursorPosition: number;

  /** Current selection (if any) */
  selection: Selection | null;

  /** Whether multiline mode is enabled */
  allowMultiline: boolean;

  /** Additional metadata for extensibility */
  metadata: Record<string, unknown>;
}

/**
 * Base interface for keyboard commands
 * Each command encapsulates a specific keyboard operation
 */
export interface KeyboardCommand {
  /**
   * Unique identifier for this command
   */
  id: string;

  /**
   * Human-readable description for debugging and documentation
   */
  description: string;

  /**
   * Check if this command can execute in the current context
   * Used for context-sensitive commands
   */
  canExecute(context: KeyboardContext): boolean;

  /**
   * Execute the command with the given context
   * Returns true if the command handled the event, false otherwise
   */
  execute(context: KeyboardContext): Promise<boolean>;
}

/**
 * Key combination for registering commands
 * Supports modifiers (Ctrl, Alt, Shift, Meta/Cmd)
 */
export interface KeyCombination {
  key: string;
  ctrl?: boolean;
  alt?: boolean;
  shift?: boolean;
  meta?: boolean; // Cmd on Mac, Win on Windows
}

/**
 * Singleton registry for keyboard commands
 * Manages registration, lookup, and execution of keyboard commands
 */
export class KeyboardCommandRegistry {
  private static instance: KeyboardCommandRegistry | null = null;

  /** Map of key combinations to commands */
  private commands = new Map<string, KeyboardCommand>();

  /** Private constructor for singleton pattern */
  private constructor() {
    // Initialize with empty registry
  }

  /**
   * Get the singleton instance
   */
  public static getInstance(): KeyboardCommandRegistry {
    if (!KeyboardCommandRegistry.instance) {
      KeyboardCommandRegistry.instance = new KeyboardCommandRegistry();
    }
    return KeyboardCommandRegistry.instance;
  }

  /**
   * Register a keyboard command for a specific key combination
   * If command already exists, silently skip (idempotent operation)
   */
  public register(keyCombination: KeyCombination, command: KeyboardCommand): void {
    const key = this.serializeKeyCombination(keyCombination);

    // Idempotent: if command already registered, skip (supports hot reload & multiple registrations)
    if (this.commands.has(key)) {
      return;
    }

    this.commands.set(key, command);
  }

  /**
   * Unregister a keyboard command
   */
  public unregister(keyCombination: KeyCombination): void {
    const key = this.serializeKeyCombination(keyCombination);
    this.commands.delete(key);
  }

  /**
   * Find and execute a command for the given keyboard event
   * Returns true if a command handled the event, false otherwise
   */
  public async execute(
    event: KeyboardEvent,
    controller: TextareaController,
    additionalContext: Partial<KeyboardContext> = {}
  ): Promise<boolean> {
    const keyCombination = this.extractKeyCombination(event);
    const key = this.serializeKeyCombination(keyCombination);

    const command = this.commands.get(key);

    if (!command) {
      return false; // No command registered for this key
    }

    // Build the full context
    const context = this.buildContext(event, controller, additionalContext);

    // MULTI-PANE COORDINATION: Only execute if this pane is active
    // This prevents keyboard shortcuts from affecting inactive panes
    if (context.paneId && context.metadata.activePaneId) {
      if (context.paneId !== context.metadata.activePaneId) {
        return false; // Not the active pane, ignore the command
      }
    }

    // Check if command can execute in current context
    if (!command.canExecute(context)) {
      return false;
    }

    try {
      // Execute the command
      const handled = await command.execute(context);

      if (handled) {
        log.debug(`Command executed: ${command.id} (${command.description})`);
      }

      return handled;
    } catch (error) {
      log.error(`Error executing command ${command.id}:`, error);
      return false;
    }
  }

  /**
   * Get all registered commands (for debugging/documentation)
   */
  public getCommands(): Map<string, KeyboardCommand> {
    return new Map(this.commands);
  }

  /**
   * Clear all registered commands (primarily for testing)
   */
  public clearAll(): void {
    this.commands.clear();
  }

  /**
   * Extract key combination from keyboard event
   */
  private extractKeyCombination(event: KeyboardEvent): KeyCombination {
    return {
      key: event.key,
      ctrl: event.ctrlKey,
      alt: event.altKey,
      shift: event.shiftKey,
      meta: event.metaKey
    };
  }

  /**
   * Serialize key combination to a unique string for Map lookup
   * Format: "Ctrl+Shift+Alt+Meta+Key"
   */
  private serializeKeyCombination(combo: KeyCombination): string {
    const parts: string[] = [];

    if (combo.ctrl) parts.push('Ctrl');
    if (combo.alt) parts.push('Alt');
    if (combo.shift) parts.push('Shift');
    if (combo.meta) parts.push('Meta');

    parts.push(combo.key);

    return parts.join('+');
  }

  /**
   * Build keyboard context from event and controller
   */
  private buildContext(
    event: KeyboardEvent,
    controller: TextareaController,
    additionalContext: Partial<KeyboardContext>
  ): KeyboardContext {
    const selection = window.getSelection();

    return {
      event,
      controller,
      nodeId: additionalContext.nodeId || '',
      nodeType: additionalContext.nodeType || 'text',
      paneId: additionalContext.paneId || DEFAULT_PANE_ID,
      content: additionalContext.content || '',
      cursorPosition: additionalContext.cursorPosition || 0,
      selection,
      allowMultiline: additionalContext.allowMultiline ?? false,
      metadata: additionalContext.metadata || {}
    };
  }
}
