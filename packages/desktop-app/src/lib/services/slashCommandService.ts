/**
 * Slash Command Service - Handles slash command detection and execution
 * 
 * Provides command definitions, filtering, and execution logic for the 
 * slash command system (/) that allows quick node type creation.
 */

import type { NodeType } from '$lib/design/icons';

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
  
  private commands: SlashCommand[] = [
    {
      id: 'text',
      name: 'Text',
      description: 'Create a text node',
      nodeType: 'text',
      icon: 'text',
      headerLevel: 0
    },
    {
      id: 'header1',
      name: 'Header 1',
      description: 'Create a large header',
      nodeType: 'text',
      icon: 'text', // Will be styled as h1
      shortcut: '#',
      headerLevel: 1
    },
    {
      id: 'header2',
      name: 'Header 2',
      description: 'Create a medium header',
      nodeType: 'text',
      icon: 'text', // Will be styled as h2
      shortcut: '##',
      headerLevel: 2
    },
    {
      id: 'header3',
      name: 'Header 3',
      description: 'Create a small header',
      nodeType: 'text',
      icon: 'text', // Will be styled as h3
      shortcut: '###',
      headerLevel: 3
    },
    {
      id: 'task',
      name: 'Task',
      description: 'Create a task with checkbox',
      nodeType: 'task',
      icon: 'task',
      shortcut: '[ ]'
    },
    {
      id: 'ai-chat',
      name: 'AI Chat',
      description: 'Start an AI conversation',
      nodeType: 'ai-chat',
      icon: 'ai_chat'
    }
  ];

  public static getInstance(): SlashCommandService {
    if (!SlashCommandService.instance) {
      SlashCommandService.instance = new SlashCommandService();
    }
    return SlashCommandService.instance;
  }

  /**
   * Get all available commands
   */
  public getCommands(): SlashCommand[] {
    return [...this.commands];
  }

  /**
   * Filter commands based on query string
   */
  public filterCommands(query: string): SlashCommand[] {
    if (!query.trim()) {
      return this.commands;
    }

    const lowerQuery = query.toLowerCase();
    return this.commands.filter(command => 
      command.name.toLowerCase().includes(lowerQuery) ||
      command.description.toLowerCase().includes(lowerQuery) ||
      command.shortcut?.toLowerCase().includes(lowerQuery)
    );
  }

  /**
   * Find command by ID
   */
  public findCommand(id: string): SlashCommand | null {
    return this.commands.find(cmd => cmd.id === id) || null;
  }

  /**
   * Execute a command - returns the content that should replace the trigger
   */
  public executeCommand(command: SlashCommand): {
    content: string;
    nodeType: string;
    headerLevel?: number;
  } {
    switch (command.id) {
      case 'text':
        return { content: '', nodeType: 'text', headerLevel: 0 };
      
      case 'header1':
        return { content: '# ', nodeType: 'text', headerLevel: 1 };
      
      case 'header2':
        return { content: '## ', nodeType: 'text', headerLevel: 2 };
      
      case 'header3':
        return { content: '### ', nodeType: 'text', headerLevel: 3 };
      
      case 'task':
        return { content: '- [ ] ', nodeType: 'task' };
      
      case 'ai-chat':
        return { content: '', nodeType: 'ai-chat' };
      
      default:
        return { content: '', nodeType: 'text', headerLevel: 0 };
    }
  }
}