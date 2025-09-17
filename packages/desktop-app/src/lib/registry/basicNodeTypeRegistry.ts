/**
 * Basic Node Type Registry
 *
 * Minimal implementation that runs alongside existing systems.
 * Does NOT replace SlashCommandService or any existing functionality.
 * This is purely for experimentation and future migration.
 */

import type { FullNodeTypeDefinition, BasicSlashCommandDefinition } from './types.js';

export class BasicNodeTypeRegistry {
  private nodeTypes = new Map<string, FullNodeTypeDefinition>();

  /**
   * Register a node type definition
   */
  register(definition: FullNodeTypeDefinition): void {
    this.nodeTypes.set(definition.id, definition);
  }

  /**
   * Get a node type definition by id
   */
  getNodeType(id: string): FullNodeTypeDefinition | null {
    return this.nodeTypes.get(id) || null;
  }

  /**
   * Get all registered node types
   */
  getAllNodeTypes(): FullNodeTypeDefinition[] {
    return Array.from(this.nodeTypes.values());
  }

  /**
   * Get all slash commands from all registered node types
   */
  getAllSlashCommands(): BasicSlashCommandDefinition[] {
    const commands: BasicSlashCommandDefinition[] = [];

    for (const nodeType of this.nodeTypes.values()) {
      commands.push(...nodeType.config.slashCommands);
    }

    return commands;
  }

  /**
   * Find a slash command by id
   */
  findSlashCommand(commandId: string): BasicSlashCommandDefinition | null {
    for (const nodeType of this.nodeTypes.values()) {
      const command = nodeType.config.slashCommands.find((cmd) => cmd.id === commandId);
      if (command) {
        return command;
      }
    }
    return null;
  }

  /**
   * Get slash commands filtered by query
   */
  filterSlashCommands(query: string): BasicSlashCommandDefinition[] {
    const allCommands = this.getAllSlashCommands();

    if (!query.trim()) {
      return allCommands;
    }

    const lowerQuery = query.toLowerCase();
    return allCommands.filter(
      (command) =>
        command.name.toLowerCase().includes(lowerQuery) ||
        command.description.toLowerCase().includes(lowerQuery) ||
        command.shortcut?.toLowerCase().includes(lowerQuery)
    );
  }

  /**
   * Check if any node types are registered
   */
  hasNodeTypes(): boolean {
    return this.nodeTypes.size > 0;
  }

  /**
   * Get registry statistics for debugging
   */
  getStats() {
    return {
      nodeTypesCount: this.nodeTypes.size,
      totalSlashCommands: this.getAllSlashCommands().length,
      nodeTypes: Array.from(this.nodeTypes.keys())
    };
  }
}

// Create a global instance for experimentation
export const basicNodeTypeRegistry = new BasicNodeTypeRegistry();
