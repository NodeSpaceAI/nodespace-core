/**
 * Slash Command Service - Handles slash command detection and execution
 *
 * Now integrates with the plugin registry for extensible slash commands.
 * External plugins can register their own slash commands dynamically.
 */

import type { NodeType } from '$lib/design/icons';
import { pluginRegistry } from '$lib/plugins/index';

export interface SlashCommand {
  id: string;
  name: string;
  description: string;
  nodeType: string;
  icon: NodeType;
  shortcut?: string;
  headerLevel?: number;
}

export interface SlashCommandContext {
  trigger: '/';
  query: string;
  startPosition: number;
  endPosition: number;
  element: HTMLElement;
  isValid: boolean;
  metadata: Record<string, unknown>;
}

export class SlashCommandService {
  private static instance: SlashCommandService | null = null;

  public static getInstance(): SlashCommandService {
    if (!SlashCommandService.instance) {
      SlashCommandService.instance = new SlashCommandService();
    }
    return SlashCommandService.instance;
  }

  /**
   * Convert plugin SlashCommandDefinitions to legacy SlashCommands
   * TODO: Eventually migrate to use SlashCommandDefinition directly
   */
  private convertPluginCommandsToSlashCommands(
    pluginCommands: import('$lib/plugins/types').SlashCommandDefinition[]
  ): SlashCommand[] {
    return pluginCommands.map((cmd) => ({
      id: cmd.id,
      name: cmd.name,
      description: cmd.description,
      nodeType: this.inferNodeTypeFromCommand(cmd),
      icon: this.inferIconFromCommand(cmd),
      shortcut: cmd.shortcut,
      headerLevel: this.inferHeaderLevelFromCommand(cmd)
    }));
  }

  /**
   * Infer node type from command (based on plugin ID or command ID)
   */
  private inferNodeTypeFromCommand(
    cmd: import('$lib/plugins/types').SlashCommandDefinition
  ): string {
    // FIRST: Use explicitly defined nodeType if available
    if (cmd.nodeType) {
      return cmd.nodeType;
    }

    // For header commands, they create text nodes
    if (cmd.id.startsWith('header')) {
      return 'text';
    }

    // For other commands, try to find the plugin that defines this command
    const plugins = pluginRegistry.getEnabledPlugins();
    for (const plugin of plugins) {
      if (plugin.config.slashCommands.some((c) => c.id === cmd.id)) {
        return plugin.id; // Use plugin ID as node type
      }
    }

    // Fallback to text
    return 'text';
  }

  /**
   * Infer icon type from command
   */
  private inferIconFromCommand(cmd: import('$lib/plugins/types').SlashCommandDefinition): NodeType {
    const nodeType = this.inferNodeTypeFromCommand(cmd);

    // Map node types to icons
    switch (nodeType) {
      case 'task':
        return 'task';
      case 'ai-chat':
        return 'ai_chat';
      case 'text':
      default:
        return 'text';
    }
  }

  /**
   * Infer header level from command
   */
  private inferHeaderLevelFromCommand(
    cmd: import('$lib/plugins/types').SlashCommandDefinition
  ): number | undefined {
    if (cmd.id === 'header1') return 1;
    if (cmd.id === 'header2') return 2;
    if (cmd.id === 'header3') return 3;
    if (cmd.id === 'text') return 0;
    return undefined;
  }

  /**
   * Get all available commands from registered plugins
   */
  public getCommands(): SlashCommand[] {
    const pluginCommands = pluginRegistry.getAllSlashCommands();
    return this.convertPluginCommandsToSlashCommands(pluginCommands);
  }

  /**
   * Filter commands based on query string
   */
  public filterCommands(query: string): SlashCommand[] {
    const allCommands = this.getCommands();

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
   * Find command by ID
   */
  public findCommand(id: string): SlashCommand | null {
    const allCommands = this.getCommands();
    return allCommands.find((cmd) => cmd.id === id) || null;
  }

  /**
   * Execute a command - returns the content that should replace the trigger
   */
  public executeCommand(command: SlashCommand): {
    content: string;
    nodeType: string;
    headerLevel?: number;
    desiredCursorPosition?: number;
  } {
    // Find the original plugin command definition
    const pluginCommand = pluginRegistry.findSlashCommand(command.id);

    if (pluginCommand) {
      return {
        content: pluginCommand.contentTemplate || '',
        nodeType: command.nodeType,
        headerLevel: command.headerLevel,
        desiredCursorPosition: pluginCommand.desiredCursorPosition
      };
    }

    // Fallback for unknown commands
    return { content: '', nodeType: 'text', headerLevel: 0 };
  }
}
