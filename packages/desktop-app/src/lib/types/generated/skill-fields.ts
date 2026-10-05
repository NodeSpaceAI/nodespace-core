// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The typed fields of a `skill` node: its retrieval and dispatch config. The
 * skill's name is the node's `content`, its guidance is its child subtree,
 * and the schemas it is about are its [`SKILL_APPLIES_TO`] edges; none of
 * those is part of this shape.
 */
export type SkillFields = {
  /**
   * The requests the skill should handle, worded the way someone would
   * ask. Embedded with the skill's name for retrieval.
   */
  useFor: string;
  /**
   * Requests that sound similar but belong to another skill, scored
   * against the query to penalize verb-only overlaps. `None` when absent or blank.
   */
  notFor?: string;
  /**
   * Tools a turn that selects this skill may call.
   */
  toolWhitelist: Array<string>;
  /**
   * ReAct iteration budget for the skill.
   */
  maxIterations: number;
};
