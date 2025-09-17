/**
 * Basic Node Type Registry Types
 *
 * Minimal interfaces for future node type registry system.
 * This does not replace any existing functionality - it's purely additive.
 */

export interface BasicNodeTypeDefinition {
  id: string;
  displayName: string;
  description: string;
}

export interface BasicSlashCommandDefinition {
  id: string;
  name: string;
  description: string;
  nodeTypeId: string;
  shortcut?: string;
  contentTemplate: string;
}

export interface BasicNodeTypeConfig {
  slashCommands: BasicSlashCommandDefinition[];
  canHaveChildren?: boolean;
  canBeChild?: boolean;
}

export interface FullNodeTypeDefinition extends BasicNodeTypeDefinition {
  config: BasicNodeTypeConfig;
}
