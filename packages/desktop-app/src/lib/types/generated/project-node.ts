// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';
import type { Priority } from './priority';
import type { ProjectStatus } from './project-status';

/**
 * Wire shape for project nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for a node whose type is exactly
 * `project`. The project schema's core fields (`status`, `priority`,
 * `start_date`, `end_date`) are promoted to the top level; they map directly
 * to the TypeScript `ProjectNode` interface. A project carries no source
 * state: that is a `software-project`'s (ADR-098).
 *
 * `status` is the project's own vocabulary; `priority` is the scale `task`
 * shares. Both are user-extensible, and the service layer validates a write
 * against the schema's declared values.
 */
export type ProjectNode = {
  status: ProjectStatus;
  priority?: Priority;
  startDate?: string;
  endDate?: string;
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
