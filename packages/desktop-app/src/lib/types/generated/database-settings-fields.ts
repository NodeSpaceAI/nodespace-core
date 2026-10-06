// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { CaptureContent } from './capture-content';
import type { ProviderConfig } from './provider-config';

/**
 * The `database-settings` schema's fields, decoded from the node's
 * properties (ADR-095).
 *
 * The only reader of a stored settings node: storage keys are the schema's
 * snake_case field names, hoisted by the store under
 * `properties.database-settings.*` and left there when another build retypes
 * the singleton to a subtype (ADR-083 §2). [`Self::from_properties`] reads
 * that bucket, or the flat shape a node built in memory carries. On the wire
 * the same fields travel camelCase at the top level of a
 * [`DatabaseSettingsNode`].
 */
export type DatabaseSettingsFields = {
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
