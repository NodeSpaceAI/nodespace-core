// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';

/**
 * Wire shape for collection nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for a `collection` node. The collection
 * schema's one field, `description`, is promoted to the top level; the
 * collection's name is the envelope's `content`.
 */
export type CollectionNode = {
  /**
   * What the collection is for.
   */
  description?: string;
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
