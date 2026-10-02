// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The typed fields of a `skill` node: its retrieval and dispatch config. The
 * skill's name is the node's `content`, and its guidance is its child
 * subtree; neither is part of this shape.
 */
export type SkillFields = {
  /**
   * What the skill is for. Drives the skill's embedding for retrieval.
   */
  description: string;
  /**
   * What the skill is *not* for, scored against the query to penalize
   * verb-only overlaps. `None` when absent or blank.
   */
  exclusion?: string;
  /**
   * Tools a turn that selects this skill may call.
   */
  toolWhitelist: Array<string>;
  /**
   * ReAct iteration budget for the skill.
   */
  maxIterations: number;
  /**
   * Schema ids this skill is scoped to. Empty means unscoped.
   */
  nodeTypes: Array<string>;
};
