/**
 * Base interface that all node viewers should implement
 * This ensures consistent event handling and prop structure
 */

import type { NodeViewerEventDetails } from '$lib/types/node-viewers.js';

export interface BaseNodeViewerInterface {
  // Required props
  nodeId: string;
  content: string;
  nodeType: string;
  
  // Optional props
  autoFocus?: boolean;
  inheritHeaderLevel?: number;
  children?: unknown[];
  
  // Required event dispatchers (must forward these events)
  // All viewers must dispatch these events for BaseNodeViewer to function
  createNewNode: (detail: NodeViewerEventDetails['createNewNode']) => void;
  contentChanged: (detail: NodeViewerEventDetails['contentChanged']) => void;
  indentNode: (detail: NodeViewerEventDetails['indentNode']) => void;
  outdentNode: (detail: NodeViewerEventDetails['outdentNode']) => void;
  navigateArrow: (detail: NodeViewerEventDetails['navigateArrow']) => void;
  combineWithPrevious: (detail: NodeViewerEventDetails['combineWithPrevious']) => void;
  deleteNode: (detail: NodeViewerEventDetails['deleteNode']) => void;
}

/**
 * Helper type for viewer component props
 * Used in Svelte components with $props() rune
 */
export interface ViewerComponentProps {
  nodeId: string;
  autoFocus?: boolean;
  content?: string;
  nodeType?: string;
  inheritHeaderLevel?: number;
  children?: unknown[];
}