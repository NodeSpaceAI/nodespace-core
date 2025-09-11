/**
 * Base interface that all node viewers should implement
 * This ensures consistent event handling and prop structure
 *
 * NOTE: For component props, use NodeViewerProps from $lib/types/nodeViewers.js
 * This interface is for documentation and implementation guidance only.
 */

import type { NodeViewerEventDetails, NodeViewerProps } from '$lib/types/nodeViewers.js';

export interface BaseNodeViewerInterface {
  // Props - use NodeViewerProps in actual components
  props: NodeViewerProps;

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
