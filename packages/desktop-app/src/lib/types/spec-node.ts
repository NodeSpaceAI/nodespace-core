/**
 * Spec node helpers.
 *
 * `SpecNode` and `SpecNodeUpdate` are generated from Rust's
 * `nodespace-types` (`./generated`): the spec schema's core fields travel
 * at the top level, and `properties` carries only extension fields
 * (`custom:…`).
 */

import type { Node } from './node';
import type { SpecNode } from './generated';
import { isExactly } from './core-node-types';

export type { SpecNode, SpecNodeUpdate, SpecStatus } from './generated';

export function isSpecNode(node: Node | SpecNode): node is SpecNode {
  return isExactly(node.nodeType, 'spec');
}

/**
 * Convert a node received over any transport to a `SpecNode`. The backend
 * (`node_to_typed_value`) already promotes the core fields to the top level
 * for every transport, so this only narrows the type and fills the status
 * default.
 */
export function nodeToSpecNode(node: Node): SpecNode {
  const spec = node as Node & Partial<SpecNode>;
  return {
    ...spec,
    nodeType: 'spec',
    properties: node.properties ?? {},
    specStatus: spec.specStatus ?? 'draft'
  };
}
