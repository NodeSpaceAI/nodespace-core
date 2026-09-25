/**
 * Type-Safe Person Node Interface
 *
 * Flat structure matching the Rust `PersonNode` wire shape
 * (`packages/nodespace-types/src/person.rs`): the person schema's core fields
 * travel at the top level. `properties` carries only extension fields
 * (`custom:…`), never `first_name`/`last_name`/`email`.
 */

import type { Node } from './node';

export interface PersonNode {
  id: string;
  nodeType: 'person';
  content: string;
  title?: string | null;
  version: number;
  createdAt: string;
  modifiedAt: string;
  /** Extension fields only — core fields are the typed fields below. */
  properties?: Record<string, unknown>;

  firstName?: string;
  lastName?: string;
  email?: string;
}

/**
 * Partial update for a person's core fields. Mirrors the Rust
 * `PersonNodeUpdate`: absent = no change, `null` = clear, a string = set.
 */
export interface PersonNodeUpdate {
  firstName?: string | null;
  lastName?: string | null;
  email?: string | null;
}

export function isPersonNode(node: Node | PersonNode): node is PersonNode {
  return node.nodeType === 'person';
}

/**
 * Convert a node received over any transport to a `PersonNode`. The backend
 * (`node_to_typed_value`) already promotes the core fields to the top level
 * for every transport, so this only narrows the type and keeps the fields
 * every node carries.
 */
export function nodeToPersonNode(node: Node): PersonNode {
  return {
    ...(node as unknown as PersonNode),
    nodeType: 'person',
    properties: node.properties ?? {}
  };
}
