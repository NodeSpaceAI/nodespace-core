// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
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
