// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';

/**
 * Wire shape for person nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for `node_type == "person"`. The person
 * schema's core fields (`first_name`, `last_name`, `email`) are promoted to
 * the top level; they map directly to the TypeScript `PersonNode` interface.
 */
export type PersonNode = {
  firstName?: string;
  lastName?: string;
  email?: string;
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
