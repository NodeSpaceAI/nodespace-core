// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Params of an `update_node` action. A play never writes `lifecycle_status`:
 * that is governance, not automation state (ADR-087).
 */
export type UpdateNodeParams = {
  node_id: string;
  content?: string;
  properties?: Record<string, unknown>;
  /**
   * Retype the node.
   */
  node_type?: string;
};
