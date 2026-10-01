/**
 * The generic node.
 *
 * The wire shapes (`NodeEnvelope`, `NodeReference`, `NodeUpdate`) are generated
 * from Rust's `nodespace-types` (`./generated`, ADR-086 §8). This module adds
 * the frontend's own types and helpers around them.
 */

import type { NodeEnvelope, RelationshipDirection } from './generated';
import { isExactly } from './core-node-types';

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

// ============================================================================
// Collection Node Types
// ============================================================================

/**
 * CollectionNode - A specialized node type for organizing other nodes
 *
 * Collections provide a flexible, hierarchical organizational structure similar
 * to tags but with additional features:
 * - Hierarchical nesting (collections can contain sub-collections)
 * - Many-to-many membership (nodes can belong to multiple collections)
 * - DAG structure (directed acyclic graph - not strictly tree)
 * - Path-based navigation (e.g., "hr:policy:vacation")
 *
 * ## Path Syntax
 *
 * Collections use colon-separated paths for intuitive navigation:
 * - "hr" → Top-level HR collection
 * - "hr:policy" → Policy sub-collection under HR
 * - "hr:policy:vacation" → Vacation policy under HR policy
 *
 * ## Usage Example
 *
 * ```typescript
 * // Create a collection
 * const collection = await createNode({
 *   nodeType: 'collection',
 *   content: 'HR Policies',
 *   properties: { description: 'Human resources policy documents' }
 * });
 *
 * // Add a document to the collection
 * await collectionService.addNodeToCollectionPath(docId, 'hr:policy');
 *
 * // Query all members of a collection
 * const members = await queryNodes({ collection: 'hr:policy' });
 * ```
 *
 * ## Difference from Parent-Child Hierarchy
 *
 * | Feature | Parent-Child | Collections |
 * |---------|-------------|-------------|
 * | Cardinality | Node has 1 parent | Node has N collections |
 * | Structure | Tree | DAG (directed acyclic graph) |
 * | Use case | Document structure | Cross-cutting organization |
 * | Path syntax | N/A | colon-separated (hr:policy) |
 *
 */
export interface CollectionNode extends NodeEnvelope {
  /** Always 'collection' for collection nodes */
  nodeType: 'collection';

  /** Collection-specific properties */
  properties: {
    /** Optional description of the collection's purpose */
    description?: string;

    /** Allow additional plugin/custom properties */
    [key: string]: unknown;
  };
}

/**
 * Type guard to check if a node is a CollectionNode
 */
export function isCollectionNode(node: Node): node is CollectionNode {
  return isExactly(node.nodeType, 'collection');
}

/**
 * Collection membership info - extended data about a node's collection memberships
 *
 * Provides full collection details for UI display.
 */
export interface CollectionMembership {
  /** The collection node this membership refers to */
  collection: CollectionNode;

  /** When the membership was created */
  addedAt: string;
}

/**
 * Collection path segment - used when parsing collection paths
 *
 * Each segment represents one level in the path hierarchy.
 */
export interface CollectionPathSegment {
  /** The segment name (e.g., "policy" in "hr:policy") */
  name: string;

  /** The resolved collection node ID, if known */
  collectionId?: string;
}

/**
 * Parse a collection path into segments
 *
 * @param path - Collection path like "hr:policy:vacation"
 * @returns Array of path segments
 *
 * @example
 * ```typescript
 * const segments = parseCollectionPath('hr:policy:vacation');
 * // Returns: [{ name: 'hr' }, { name: 'policy' }, { name: 'vacation' }]
 * ```
 */
export function parseCollectionPath(path: string): CollectionPathSegment[] {
  if (!path || path.trim() === '') {
    return [];
  }

  return path
    .split(':')
    .filter((segment) => segment.trim() !== '')
    .map((name) => ({ name: name.trim() }));
}

/**
 * Format collection path segments back into a path string
 *
 * @param segments - Array of path segments
 * @returns Colon-separated path string
 */
export function formatCollectionPath(segments: CollectionPathSegment[]): string {
  return segments.map((s) => s.name).join(':');
}

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
