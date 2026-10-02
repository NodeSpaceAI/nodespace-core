// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';

/**
 * Wire shape for the database-settings singleton sent to the frontend.
 *
 * Produced by `node_to_typed_value` for a `database-settings` node. Its one
 * field, `required_extensions`, is promoted to the top level.
 */
export type DatabaseSettingsNode = {
  /**
   * Ids of the extensions a reader needs in order to read this database.
   * Empty when the settings node stores none.
   */
  requiredExtensions: Array<string>;
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
