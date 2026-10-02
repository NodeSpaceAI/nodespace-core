// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The typed fields of a `skill` node: the retrieval and dispatch config
 * stored in its properties. The skill's name is the node's `content`.
 *
 * This is the only place a skill field is read from or written to the
 * properties bag. `skill` has a registered core schema, so the store hoists
 * its fields under `properties.skill.*`; a node built in memory, a seed
 * template, or a flat update patch carries them at the top level instead.
 * [`SkillFields::from_properties`] reads both, preferring the `skill` bucket
 * per field. Every hand-rolled reader that guessed only one of the two
 * shapes read that field as empty.
 *
 * The skill's guidance body is its child subtree, not a property, so it is
 * not part of this model.
 *
 * Serializes with camelCase keys: these are the fields the wire [`SkillNode`]
 * promotes to the top level. Storage keeps the schema's snake_case names
 * ([`Self::properties`]).
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
