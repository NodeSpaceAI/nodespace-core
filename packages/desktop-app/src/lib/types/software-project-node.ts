/**
 * Software-project node helpers.
 *
 * A software project is a project with source code. `SoftwareProjectNode` and
 * `SoftwareProjectNodeUpdate` are generated from Rust's `nodespace-types`
 * (`./generated`): a project's fields, plus `repository` and `checkoutPath`,
 * the folder on this machine that holds its checkout.
 */

import type { Node } from './node';
import type { SoftwareProjectNode } from './generated';
import { isExactly } from './core-node-types';

export type { SoftwareProjectNode, SoftwareProjectNodeUpdate } from './generated';

export function isSoftwareProjectNode(
  node: Node | SoftwareProjectNode
): node is SoftwareProjectNode {
  return isExactly(node.nodeType, 'software-project');
}

/**
 * Convert a node received over any transport to a `SoftwareProjectNode`. The
 * backend (`node_to_typed_value`) already promotes the core fields to the top
 * level for every transport, so this only narrows the type and fills the
 * status default.
 */
export function nodeToSoftwareProjectNode(node: Node): SoftwareProjectNode {
  const project = node as Node & Partial<SoftwareProjectNode>;
  // Every typed key is written, set or not, so a merge over an older copy of
  // the node clears a field the backend no longer carries.
  return {
    ...project,
    nodeType: 'software-project',
    properties: node.properties ?? {},
    status: project.status ?? 'planning',
    priority: project.priority,
    startDate: project.startDate,
    endDate: project.endDate,
    repository: project.repository,
    checkoutPath: project.checkoutPath
  };
}
