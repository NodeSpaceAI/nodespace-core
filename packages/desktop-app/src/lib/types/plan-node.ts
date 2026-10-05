/**
 * Plan node helpers.
 *
 * `PlanNode` and `PlanNodeUpdate` are generated from Rust's
 * `nodespace-types` (`./generated`): the plan schema's core fields travel
 * at the top level, and `properties` carries only extension fields
 * (`custom:…`).
 */

import type { Node } from './node';
import type { PlanNode } from './generated';
import { isExactly } from './core-node-types';

export type { PlanNode, PlanNodeUpdate, PlanStatus } from './generated';

export function isPlanNode(node: Node | PlanNode): node is PlanNode {
  return isExactly(node.nodeType, 'plan');
}

/**
 * Convert a node received over any transport to a `PlanNode`. The backend
 * (`node_to_typed_value`) already promotes the core fields to the top level
 * for every transport, so this only narrows the type and fills the status
 * default.
 */
export function nodeToPlanNode(node: Node): PlanNode {
  const plan = node as Node & Partial<PlanNode>;
  return {
    ...plan,
    nodeType: 'plan',
    properties: node.properties ?? {},
    planStatus: plan.planStatus ?? 'draft'
  };
}
