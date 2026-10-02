// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Partial update for a skill's core fields, received from the frontend.
 *
 * `description` and `tool_whitelist` have no clear path (the schema requires
 * them); the other fields are tri-state: absent leaves the field unchanged,
 * `null` clears it, and a value sets it. A list is replaced whole. The
 * skill's name is `content`, an envelope field, and its guidance is its
 * child subtree; both are written through the generic node operations.
 */
export type SkillNodeUpdate = {
  description?: string;
  exclusion?: string | null;
  toolWhitelist?: Array<string>;
  maxIterations?: number | null;
  nodeTypes?: Array<string> | null;
};
