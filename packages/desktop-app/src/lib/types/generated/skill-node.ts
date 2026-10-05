// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';

/**
 * Wire shape for skill nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for a `skill` node: the skill schema's
 * fields are promoted to the top level (camelCase, see [`SkillFields`]) and
 * `properties` keeps only extension fields. The skill's name is the
 * envelope's `content`.
 */
export type SkillNode = {
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
