/**
 * Issue node helpers.
 *
 * An issue is a task that is development work: one change, delivered by one
 * pull request. `IssueNode`, `IssueNodeUpdate` and `IssueStatus` are generated
 * from Rust's `nodespace-types` (`./generated`): a task's fields, the issue's
 * `pullRequest` and `commits`, and a status vocabulary that adds
 * `changes_requested` and `ready_to_merge` to a task's.
 */

import type { Node } from './node';
import type { IssueNode } from './generated';
import { isExactly } from './core-node-types';

export type { IssueNode, IssueNodeUpdate, IssueStatus } from './generated';

export function isIssueNode(node: Node | IssueNode): node is IssueNode {
  return isExactly(node.nodeType, 'issue');
}

/**
 * Convert a node received over any transport to an `IssueNode`. The backend
 * (`node_to_typed_value`) already promotes the core fields to the top level
 * for every transport, so this only narrows the type and fills the status
 * default.
 */
export function nodeToIssueNode(node: Node): IssueNode {
  const issue = node as Node & Partial<IssueNode>;
  // Every typed key is written, set or not, so a merge over an older copy of
  // the node clears a field the backend no longer carries.
  return {
    ...issue,
    nodeType: 'issue',
    properties: node.properties ?? {},
    status: issue.status ?? 'open',
    priority: issue.priority,
    dueDate: issue.dueDate,
    startedAt: issue.startedAt,
    completedAt: issue.completedAt,
    pullRequest: issue.pullRequest,
    commits: issue.commits
  };
}
