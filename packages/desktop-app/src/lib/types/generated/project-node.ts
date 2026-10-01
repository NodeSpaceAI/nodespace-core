// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';
import type { Priority } from './priority';

/**
 * Wire shape for project nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for `node_type == "project"`. The project
 * schema's core fields (`status`, `priority`, `start_date`, `end_date`) are
 * promoted to the top level; they map directly to the TypeScript
 * `ProjectNode` interface.
 *
 * `status` stays a string: it is user-extensible (`user_values`), and the
 * schema, not this struct, owns the vocabulary — the service layer validates
 * writes against it. `priority` is the scale `task` shares.
 */
export type ProjectNode = {
  status: string;
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
