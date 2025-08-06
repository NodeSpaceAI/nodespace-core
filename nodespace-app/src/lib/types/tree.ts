/**
 * Tree Node Data Types
 * 
 * Shared type definitions for hierarchical tree structures.
 */

// Tree node interface for hierarchical data
export interface TreeNodeData {
  id: string;
  title: string;
  content: string;
  nodeType: 'text' | 'task' | 'ai-chat' | 'entity' | 'query';
  depth: number;
  parentId: string | null;
  children: TreeNodeData[];
  expanded: boolean;
  hasChildren: boolean;
}