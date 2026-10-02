/**
 * The generic node.
 *
 * The wire shapes (`NodeEnvelope`, `NodeReference`, `NodeUpdate`) are generated
 * from Rust's `nodespace-types` (`./generated`, ADR-086 §8). This module adds
 * the frontend's own types and helpers around them.
 */

import type { NodeEnvelope, RelationshipDirection } from './generated';

export type { NodeEnvelope, NodeReference, NodeUpdate, RelationshipDirection } from './generated';

/**
 * Represents a relationship with another node, including direction
 *
 * Used for returning relationships in subtree queries where we need to know
 * both the related node and the direction of the relationship.
 *
 * For outgoing relationships (direction = 'out'):
 * - `id` is the target node ID
 * - The current node points TO this node
 *
 * For incoming relationships (direction = 'in'):
 * - `id` is the source node ID
 * - This node points TO the current node
 */
export interface NodeRelationship {
  /** The related node's ID */
  id: string;
  /** The related node's title (for display) */
  title: string | null;
  /** Direction of the relationship relative to the node this is attached to */
  direction: RelationshipDirection;
  /** Type of relationship (e.g., "mentions", "has_child", "member_of") */
  relationshipType: string;
}

/**
 * Node - the generic node: the envelope, with every field of its type inside
 * `properties`. A primitive type has no fields, so this is also its whole
 * wire shape.
 *
 * All services and components use this type for a node of any type; a type
 * with its own fields has a generated typed shape that includes the envelope.
 */
export type Node = NodeEnvelope;

/**
 * NodeUIState - Separate UI state storage
 *
 * Stored in parallel Map in ReactiveNodeService.
 * Keeps UI concerns separate from data model.
 *
 * Why separate?
 * - Data (Node) is persisted and synced
 * - UI state is ephemeral and local-only
 * - Clean separation of concerns
 */
export interface NodeUIState {
  /** Node ID this state belongs to */
  nodeId: string;

  /** Hierarchy depth (0 = root, 1 = child of root, etc.) */
  depth: number;

  /** Whether node's children are visible */
  expanded: boolean;

  /** Whether this node should receive focus */
  autoFocus: boolean;

  /** Inherited header level for rendering */
  inheritHeaderLevel: number;

  /** Whether this is a placeholder node (not yet persisted) */
  isPlaceholder: boolean;
}

/**
 * Type guard to check if an object is a Node
 */
export function isNode(obj: unknown): obj is Node {
  if (typeof obj !== 'object' || obj === null) return false;

  const node = obj as Record<string, unknown>;

  return (
    typeof node.id === 'string' &&
    typeof node.nodeType === 'string' &&
    typeof node.content === 'string' &&
    typeof node.createdAt === 'string' &&
    typeof node.modifiedAt === 'string' &&
    typeof node.properties === 'object' &&
    node.properties !== null
  );
}

/**
 * Create default UI state for a node
 */
export function createDefaultUIState(
  nodeId: string,
  overrides?: Partial<NodeUIState>
): NodeUIState {
  return {
    nodeId,
    depth: 0,
    expanded: false,
    autoFocus: false,
    inheritHeaderLevel: 0,
    isPlaceholder: false,
    ...overrides
  };
}
