/**
 * Shared types for the viewer's per-row rendering (NodeRow) surface.
 *
 * A ViewerRenderNode is a core Node augmented with the viewer-local UI state used
 * to render a single row. The event-detail types mirror base-node.svelte's
 * dispatcher; NodeRow consumes those `on:` events internally and re-exposes them to
 * the viewer as the callback props declared here.
 */

import type { Node } from '$lib/types';

/** A node augmented with viewer-local render state for a single row. */
export interface ViewerRenderNode extends Node {
  depth: number;
  children: string[];
  expanded: boolean;
  autoFocus: boolean;
  inheritHeaderLevel: number;
  isPlaceholder: boolean;
}

export interface CreateNewNodeDetail {
  afterNodeId: string;
  nodeType: string;
  currentContent?: string;
  newContent?: string;
  originalContent?: string;
  inheritHeaderLevel?: number;
  cursorAtBeginning?: boolean;
  insertAtBeginning?: boolean;
  focusOriginalNode?: boolean;
  newNodeCursorPosition?: number;
}

export interface NavigateArrowDetail {
  nodeId: string;
  direction: 'up' | 'down';
  pixelOffset: number;
}

export interface ContentChangedDetail {
  content: string;
  cursorPosition?: number;
}

export interface NodeTypeChangedDetail {
  nodeType: string;
  cleanedContent?: string;
  cursorPosition?: number;
}

export interface SlashCommandSelectedDetail {
  command: string;
  nodeType: string;
  cursorPosition?: number;
}

export interface IconClickDetail {
  nodeId: string;
  nodeType: string;
  currentState?: string;
}

export interface TaskStateChangedDetail {
  nodeId: string;
  state: string;
}

export interface CombineWithPreviousDetail {
  nodeId: string;
  currentContent: string;
}

export interface DeleteNodeDetail {
  nodeId: string;
}

/** Callback props NodeRow forwards node events to. */
export interface NodeRowCallbacks {
  onCreateNewNode: (_detail: CreateNewNodeDetail) => void;
  onIndentNode: (_detail: { nodeId: string }) => void;
  onOutdentNode: (_detail: { nodeId: string }) => void;
  onNavigateArrow: (_detail: NavigateArrowDetail) => void;
  onContentChanged: (_node: ViewerRenderNode, _detail: ContentChangedDetail) => void;
  onNodeTypeChanged: (_node: ViewerRenderNode, _detail: NodeTypeChangedDetail) => void;
  onSlashCommandSelected: (_node: ViewerRenderNode, _detail: SlashCommandSelectedDetail) => void;
  onIconClick?: (_detail: IconClickDetail) => void;
  onTaskStateChanged: (_node: ViewerRenderNode, _detail: TaskStateChangedDetail) => void;
  onCombineWithPrevious: (_detail: CombineWithPreviousDetail) => void;
  onDeleteNode: (_detail: DeleteNodeDetail) => void;
  onToggleExpanded: (_nodeId: string) => void;
}
