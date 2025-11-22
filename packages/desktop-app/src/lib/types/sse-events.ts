/**
 * SSE Event Types for Browser Mode Real-Time Sync
 *
 * These types define the structure of Server-Sent Events received from
 * the dev-proxy's `/api/events` endpoint. They mirror the Rust SseEvent
 * enum in dev-proxy.rs.
 */

import type { Node } from '$lib/types';

/**
 * Base interface for all SSE events
 */
export interface SseEventBase {
  type: string;
}

/**
 * Event sent when a new node is created
 */
export interface NodeCreatedEvent extends SseEventBase {
  type: 'nodeCreated';
  nodeId: string;
  nodeData: Node;
}

/**
 * Event sent when an existing node is updated
 */
export interface NodeUpdatedEvent extends SseEventBase {
  type: 'nodeUpdated';
  nodeId: string;
  nodeData: Node;
}

/**
 * Event sent when a node is deleted
 */
export interface NodeDeletedEvent extends SseEventBase {
  type: 'nodeDeleted';
  nodeId: string;
}

/**
 * Event sent when a parent-child edge is created
 */
export interface EdgeCreatedEvent extends SseEventBase {
  type: 'edgeCreated';
  parentId: string;
  childId: string;
}

/**
 * Event sent when a parent-child edge is deleted
 */
export interface EdgeDeletedEvent extends SseEventBase {
  type: 'edgeDeleted';
  parentId: string;
  childId: string;
}

/**
 * Union type of all SSE events
 */
export type SseEvent =
  | NodeCreatedEvent
  | NodeUpdatedEvent
  | NodeDeletedEvent
  | EdgeCreatedEvent
  | EdgeDeletedEvent;
