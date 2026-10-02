// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';
import type { RuleDefinition } from './rule-definition';

/**
 * Wire shape for play nodes sent to the frontend.
 *
 * Produced by `node_to_typed_value` for a play: the play schema's fields are
 * promoted to the top level and `properties` keeps only extension fields.
 */
export type PlayNode = {
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
  rules: Array<RuleDefinition>;
  /**
   * What the play automates, in one line.
   */
  description?: string;
};
