// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { LinkValue } from './link-value';
import type { NodeReference } from './node-reference';
import type { Priority } from './priority';
import type { TaskStatus } from './task-status';

/**
 * Wire shape for task nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for `node_type == "task"`. Fields map
 * directly to the TypeScript `TaskNode` interface.
 */
export type TaskNode = {
  status: TaskStatus;
  priority?: Priority;
  dueDate?: string;
  startedAt?: string;
  completedAt?: string;
  /**
   * The pull request that delivered the task.
   */
  pullRequest?: LinkValue;
  /**
   * The commits that delivered the task.
   */
  commits?: Array<LinkValue>;
  /**
   * Whether the task must link an approved spec before it is started.
   * `false` is the light lane for chores and small fixes (ADR-097 §6).
   * Absent reads as `true`, the schema default.
   */
  requiresSpec: boolean;
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
