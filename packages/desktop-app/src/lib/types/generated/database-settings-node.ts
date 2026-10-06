// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { CaptureContent } from './capture-content';
import type { NodeReference } from './node-reference';
import type { ProviderConfig } from './provider-config';

/**
 * Wire shape for the database-settings singleton sent to the frontend.
 *
 * Produced by `node_to_typed_value` for a `database-settings` node: the
 * schema's fields are promoted to the top level (camelCase, see
 * [`DatabaseSettingsFields`]).
 */
export type DatabaseSettingsNode = {
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
  /**
   * Ids of the extensions a reader needs in order to read this database.
   */
  requiredExtensions: Array<string>;
  /**
   * Whether a finished terminal session is saved to its chat node.
   */
  captureEnabled: boolean;
  /**
   * How much of a captured session is saved.
   */
  captureContent: CaptureContent;
  /**
   * The OpenAI-compatible providers this database has configured.
   */
  providers: Array<ProviderConfig>;
};
