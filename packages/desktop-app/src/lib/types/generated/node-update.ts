// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

export type NodeUpdate = {
  nodeType?: string;
  content?: string;
  properties?: Record<string, unknown>;
  title?: string | null;
  lifecycleStatus?: string;
  /**
   * Property keys to remove, applied after `properties` is merged. Bare
   * field names. A key the node does not hold is left as it is.
   */
  unsetProperties?: Array<string>;
};
