/**
 * SSE Event Types for Browser Mode Real-Time Sync
 *
 * These types define the structure of Server-Sent Events received from
 * the dev-proxy's `/api/events` endpoint. They mirror the Rust SseEvent
 * enum in dev-proxy.rs.
 *
 * Issue #724: Events send only node_id (not full payload) for efficiency.
 * Frontend fetches full node data via get_node() API if needed.
 */

/**
 * Base interface for all SSE events
 */
export interface SseEventBase {
  type: string;
}

/**
 * Event sent when a new node is created (ID only)
 */
export interface NodeCreatedEvent extends SseEventBase {
  type: 'nodeCreated';
  nodeId: string;
  clientId?: string;
}

/**
 * Event sent when an existing node is updated (ID only)
 */
export interface NodeUpdatedEvent extends SseEventBase {
  type: 'nodeUpdated';
  nodeId: string;
  clientId?: string;
}

/**
 * Event sent when a node is deleted
 */
export interface NodeDeletedEvent extends SseEventBase {
  type: 'nodeDeleted';
  nodeId: string;
  clientId?: string;
}

// ============================================================================
// Unified Relationship Events (Issue #811)
// All relationship types (has_child, member_of, mentions, custom) use these events.
// ============================================================================

/**
 * Event sent when any relationship is created (unified format)
 *
 * Supports all relationship types: has_child, member_of, mentions, custom
 */
export interface RelationshipCreatedEvent extends SseEventBase {
  type: 'relationshipCreated';
  id: string;
  fromId: string;
  toId: string;
  relationshipType: string;
  properties: Record<string, unknown>;
  clientId?: string;
}

/**
 * Event sent when any relationship is updated (unified format)
 */
export interface RelationshipUpdatedEvent extends SseEventBase {
  type: 'relationshipUpdated';
  id: string;
  fromId: string;
  toId: string;
  relationshipType: string;
  properties: Record<string, unknown>;
  clientId?: string;
}

/**
 * Event sent when any relationship is deleted (unified format)
 */
export interface RelationshipDeletedEvent extends SseEventBase {
  type: 'relationshipDeleted';
  id: string;
  fromId: string;
  toId: string;
  relationshipType: string;
  clientId?: string;
}

/**
 * Union type of all SSE events
 */
export type SseEvent =
  | NodeCreatedEvent
  | NodeUpdatedEvent
  | NodeDeletedEvent
  | RelationshipCreatedEvent
  | RelationshipUpdatedEvent
  | RelationshipDeletedEvent;
