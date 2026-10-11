// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { IssueStatus } from './issue-status';
import type { LinkValue } from './link-value';
import type { NodeReference } from './node-reference';
import type { Priority } from './priority';

/**
 * Wire shape for issue nodes sent to the frontend: a task's fields and the
 * issue's own (ADR-098).
 *
 * Produced by `node_to_typed_value` for a node whose type is exactly
 * `issue`. Fields map directly to the TypeScript `IssueNode` interface.
 */
export type IssueNode = {
  status: IssueStatus;
  priority?: Priority;
  dueDate?: string;
  startedAt?: string;
  completedAt?: string;
  /**
   * The pull request that delivers the issue.
   */
  pullRequest?: LinkValue;
  /**
   * The commits that deliver the issue.
   */
  commits?: Array<LinkValue>;
  id: string;
  nodeType: string;
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  properties: Record<string, unknown>;
  mentions?: Array<string>;
  mentionedIn?: Array<NodeReference>;
  title?: string | null;
  lifecycleStatus: string;
};
