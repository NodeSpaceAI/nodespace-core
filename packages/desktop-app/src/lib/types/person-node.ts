/**
 * Person node helpers.
 *
 * `PersonNode` and `PersonNodeUpdate` are generated from Rust's
 * `nodespace-types` (`./generated`): the person schema's core fields travel at
 * the top level, and `properties` carries only extension fields (`custom:…`).
 */

import type { Node } from './node';
import type { PersonNode } from './generated';
import { isExactly } from './core-node-types';

export type { PersonNode, PersonNodeUpdate } from './generated';

export function isPersonNode(node: Node | PersonNode): node is PersonNode {
  return isExactly(node.nodeType, 'person');
}

/**
 * Convert a node received over any transport to a `PersonNode`. The backend
 * (`node_to_typed_value`) already promotes the core fields to the top level
 * for every transport, so this only narrows the type and keeps the fields
 * every node carries.
 */
export function nodeToPersonNode(node: Node): PersonNode {
  return {
    ...(node as PersonNode),
    nodeType: 'person',
    properties: node.properties ?? {}
  };
}
