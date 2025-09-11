/**
 * Type definitions for the node viewer plugin system
 */

import type { ComponentType } from 'svelte';

export interface NodeViewerProps {
  nodeId: string;
  autoFocus?: boolean;
  content: string;
  nodeType: string;
  inheritHeaderLevel?: number;
  children?: unknown[];
}

export interface NodeViewerEventDetails {
  createNewNode: {
    afterNodeId: string;
    nodeType: string;
    currentContent?: string;
    newContent?: string;
    inheritHeaderLevel?: number;
    cursorAtBeginning?: boolean;
  };
  contentChanged: { content: string };
  indentNode: { nodeId: string };
  outdentNode: { nodeId: string };
  navigateArrow: { 
    nodeId: string; 
    direction: 'up' | 'down'; 
    columnHint: number;
  };
  combineWithPrevious: { 
    nodeId: string; 
    currentContent: string;
  };
  deleteNode: { nodeId: string };
}

export type NodeViewerComponent = ComponentType;

export interface ViewerRegistration {
  component?: NodeViewerComponent;
  lazyLoad?: () => Promise<{ default: NodeViewerComponent }>;
  priority: number; // Higher priority overrides default viewers
}

export interface BaseNodeViewer {
  // Required props
  nodeId: string;
  content: string;
  nodeType: string;
  
  // Optional props
  autoFocus?: boolean;
  inheritHeaderLevel?: number;
  children?: unknown[];
}