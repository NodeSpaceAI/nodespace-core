/**
 * Basic Node Type Definitions
 *
 * These mirror the existing hardcoded node types in SlashCommandService.
 * They're purely for experimentation and future migration.
 */

import type { FullNodeTypeDefinition } from './types.js';

export const textNodeType: FullNodeTypeDefinition = {
  id: 'text',
  displayName: 'Text',
  description: 'Create a text node with optional header formatting',
  config: {
    slashCommands: [
      {
        id: 'text',
        name: 'Text',
        description: 'Create a text node',
        nodeTypeId: 'text',
        contentTemplate: ''
      },
      {
        id: 'header1',
        name: 'Header 1',
        description: 'Create a large header',
        nodeTypeId: 'text',
        shortcut: '#',
        contentTemplate: '# '
      },
      {
        id: 'header2',
        name: 'Header 2',
        description: 'Create a medium header',
        nodeTypeId: 'text',
        shortcut: '##',
        contentTemplate: '## '
      },
      {
        id: 'header3',
        name: 'Header 3',
        description: 'Create a small header',
        nodeTypeId: 'text',
        shortcut: '###',
        contentTemplate: '### '
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  }
};

export const taskNodeType: FullNodeTypeDefinition = {
  id: 'task',
  displayName: 'Task',
  description: 'Create a task with checkbox and state management',
  config: {
    slashCommands: [
      {
        id: 'task',
        name: 'Task',
        description: 'Create a task with checkbox',
        nodeTypeId: 'task',
        shortcut: '[ ]',
        contentTemplate: '- [ ] '
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  }
};

export const aiChatNodeType: FullNodeTypeDefinition = {
  id: 'ai-chat',
  displayName: 'AI Chat',
  description: 'Start an AI conversation',
  config: {
    slashCommands: [
      {
        id: 'ai-chat',
        name: 'AI Chat',
        description: 'Start an AI conversation',
        nodeTypeId: 'ai-chat',
        shortcut: '⌘ + k',
        contentTemplate: ''
      }
    ],
    canHaveChildren: true,
    canBeChild: true
  }
};
