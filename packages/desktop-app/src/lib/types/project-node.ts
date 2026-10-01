/**
 * Project node helpers.
 *
 * `ProjectNode` and `ProjectNodeUpdate` are generated from Rust's
 * `nodespace-types` (`./generated`): the project schema's core fields travel
 * at the top level, and `properties` carries only extension fields
 * (`custom:…`).
 */

import type { Node } from './node';
import type { ProjectNode } from './generated';
import { isExactly } from './core-node-types';

export type { ProjectNode, ProjectNodeUpdate, ProjectStatus } from './generated';

export function isProjectNode(node: Node | ProjectNode): node is ProjectNode {
  return isExactly(node.nodeType, 'project');
}

/**
 * Convert a node received over any transport to a `ProjectNode`. The backend
 * (`node_to_typed_value`) already promotes the core fields to the top level
 * for every transport, so this only narrows the type and fills the status
 * default.
 */
export function nodeToProjectNode(node: Node): ProjectNode {
  const project = node as Node & Partial<ProjectNode>;
  return {
    ...project,
    nodeType: 'project',
    properties: node.properties ?? {},
    status: project.status ?? 'planning'
  };
}
