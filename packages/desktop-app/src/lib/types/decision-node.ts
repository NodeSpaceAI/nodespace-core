/**
 * Decision node helpers.
 *
 * `DecisionNode` and `DecisionNodeUpdate` are generated from Rust's
 * `nodespace-types` (`./generated`): the decision schema's core fields travel
 * at the top level, and `properties` carries only extension fields
 * (`custom:…`).
 */

import type { Node } from './node';
import type { DecisionNode } from './generated';
import { isExactly } from './core-node-types';

export type { DecisionNode, DecisionNodeUpdate, DecisionStatus } from './generated';

export function isDecisionNode(node: Node | DecisionNode): node is DecisionNode {
  return isExactly(node.nodeType, 'decision');
}

/**
 * Convert a node received over any transport to a `DecisionNode`. The backend
 * (`node_to_typed_value`) already promotes the core fields to the top level
 * for every transport, so this only narrows the type and fills the status
 * default.
 */
export function nodeToDecisionNode(node: Node): DecisionNode {
  const decision = node as Node & Partial<DecisionNode>;
  return {
    ...decision,
    nodeType: 'decision',
    properties: node.properties ?? {},
    decisionStatus: decision.decisionStatus ?? 'proposed'
  };
}
