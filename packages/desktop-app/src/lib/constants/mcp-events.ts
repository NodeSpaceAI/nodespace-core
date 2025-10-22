/**
 * MCP Server Event Constants
 *
 * Centralized event type definitions for MCP server events emitted from the Rust backend.
 * These events are used for real-time UI synchronization when operations occur via MCP.
 *
 * @module constants/mcp-events
 */

/**
 * MCP server event types
 * @constant
 */
export const MCP_EVENTS = {
  /** Emitted when a new node is created via MCP (payload includes full Node data) */
  NODE_CREATED: 'node-created',
  /** Emitted when a node is updated via MCP (payload includes node_id, frontend fetches full node) */
  NODE_UPDATED: 'node-updated',
  /** Emitted when a node is deleted via MCP (payload includes node_id) */
  NODE_DELETED: 'node-deleted'
} as const;

/**
 * Type for MCP event names
 */
export type McpEventType = (typeof MCP_EVENTS)[keyof typeof MCP_EVENTS];
