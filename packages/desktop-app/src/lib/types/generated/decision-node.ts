// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { DecisionStatus } from './decision-status';
import type { NodeReference } from './node-reference';

/**
 * Wire shape for decision nodes sent to the frontend.
 *
 * A decision records what was decided and why. Its `content` is its title
 * and its body is its children, so its status is its only field.
 */
export type DecisionNode = {
  decisionStatus: DecisionStatus;
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
};
