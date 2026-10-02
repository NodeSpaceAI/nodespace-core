// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Params of a `create_node` action. Every string may be a `{binding}`
 * template; `properties` is open because the created type's schema decides
 * its keys.
 */
export type CreateNodeParams = {
  node_type: string;
  /**
   * The schema version of `node_type` the rule was written against. When
   * given, it must match the installed schema's.
   */
  version?: number;
  content?: string;
  properties?: Record<string, unknown>;
};
